# CLAUDE.md

Guidance for Claude Code when working in this repository.

> **Keep this file current.** When a change alters something documented here —
> pipeline stage order, a data contract, an analysis default, the sidecar
> format, or the build/test commands — update the relevant section in the same
> change. After adding an adjustment control, re-check the "Adding a new
> adjustment control" checklist.

SwiftInvert is a desktop film-negative conversion app written in **Rust +
egui (eframe) + wgpu**, a deliberate ~10% rewrite of the Python app **NegPy**
(`~/Documents/code/NegPy`), which remains the numerical reference
implementation. Scope: library grid, C-41 conversion with NegPy-quality auto
metering, exposure/color controls, histogram, darkroom read-outs, export.
Camera RAW only; no B&W/E6, papers, crosstalk, retouch, or toning.

**History.** The app was first written in Swift + SwiftUI + Metal (plus a
Qt/Vulkan shell for Linux); the Rust port (branch `rust-port`, 2026-10-15/16)
reached parity and became the product. On **2026-10-28** the Swift and Qt trees
were removed and the Cargo workspace moved from `rust/` to the repository root.
The last commit carrying the Swift sources is **`e20fcc2`**: `PLAN.md`,
`docs/*.md` and many module headers cite Swift files by `file:line`, and every
such citation still resolves with `git show e20fcc2:Sources/…`. The name stays
"SwiftInvert" (binary `swiftinvert`); renaming is the user's call.

## Commands

```bash
./ci.sh                                   # the quality gate: cargo fmt --check, clippy -D warnings, cargo test --workspace
NEG_RAW_SAMPLE=/path/file.CR3 ./ci.sh     # …plus the real-RAW smoke tests (skipped and announced otherwise)
make test                                 # = ./ci.sh;  make build / run / release / app / install (see the Makefile)
cargo run --release -p swiftinvert [folder]                 # the app from source
cargo run --release -p negcli -- bench /path/file.CR3       # CPU vs GPU bench table
cargo run --release -p negcli -- render <raw> [--sidecar P | --stock] [--set key=value]... -o out.png
cargo run --release -p swiftinvert -- --selftest out.png <folder> --crop --dialogs --teststrip --zones --latency --export <dir>
NEG_GPU=off …                             # force the CPU reference renderer
./build.sh                                # macOS: package dist/SwiftInvert.app (= make app); make install → /Applications
packaging/linux/install-desktop.sh        # Linux: user-scope .desktop entry + icon

# Regenerate parity fixtures (after deliberate NegPy kernel/constant changes):
cd ~/Documents/code/NegPy && uv run python ~/Documents/code/SwiftInvert/scripts/dump_fixtures.py
```

- `--selftest` is the headless way to verify UI changes: it renders a real
  negative offscreen (egui_kittest) and writes screenshots; it never writes
  sidecars. It rides the default-on `selftest` cargo feature of
  `apps/swiftinvert` (`--no-default-features` leaves the harness out).
- GPU tests skip with a printed reason when no adapter is visible. On this
  Mac they run on Metal. `NEG_GPU_BACKEND=vulkan|metal|dx12|off` overrides
  the backend choice.
- Sample negatives on this machine: `~/Pictures/Negative Test 1/*.CR3`.
- Toolchain: Rust 1.95+ (edition 2024). No C toolchain is needed by default;
  `--features libraw` on `negraw` compiles a C shim against `<libraw/libraw.h>`
  (Homebrew `libraw` on the Mac) and is the byte-exact-vs-NegPy decode
  reference, not the product path.
- Other env knobs: `SWIFTINVERT_NO_NATIVE_MENU=1` (macOS: in-window egui menu
  instead of the system menu bar), `SWIFTINVERT_FONT` / `SWIFTINVERT_FONT_GRADE`
  (UI font override).

## Repository map

```
Cargo.toml / Cargo.lock      workspace (resolver 3; default members = crates/* + negcli)
ci.sh                        the quality gate
Makefile, build.sh           cargo wrappers + macOS .app packaging (packaging/macos/Info.plist)
crates/negkit                L1 pure math — port of NegativeKit (NegPy's kernel). No io/gpu/ui.
crates/negraw                L2 RAW → RgbImage. lcraw (lightcraft-raw, default) | libraw (FFI feature)
crates/neggpu                L3 wgpu compute: ONE WGSL module, uniform packing, histogram, readback
crates/negsession            L4 engine: cache tower, sidecars, profiles, history, export, library scan
crates/negui                 L5 egui shell: library, loupe, controls table, tools, dialogs, darkroom UI
apps/swiftinvert             the desktop binary (eframe + platform glue)
apps/negcli                  headless driver: decode / thumb / render / meter / bench / export
fixtures/                    NegPy-dumped parity fixtures (scripts/dump_fixtures.py)
assets/                      app icon (+ .icns), screenshot, bundled Inter font (ATTRIBUTION.md)
packaging/{macos,linux}      Info.plist; desktop-entry installer
docs/controls-spec.md        the sidebar/window/library spec the UI was ported from (cites Swift file:line)
docs/session-spec.md         the engine semantics spec: caching keys, invariants, state machines, formats
PLAN.md                      decisions (§0 — don't relitigate), architecture, data contracts, test map, progress
HANDOFF.md                   state at the end of the port, known gaps, review protocol
UPSTREAM.md                  NegPy review log (last reviewed commit; port/skip decisions)
TODO.md                      feature candidates
scripts/                     dump_fixtures.py (NegPy → fixtures), generate_icon.swift (icon artwork; standalone CoreGraphics script)
.claude/skills/negpy-review  the upstream-review skill
```

Layering rules (PLAN §1): nothing below `negui` imports egui/eframe/winit/rfd;
nothing below `neggpu` imports wgpu; `negkit` builds for wasm32 with
`--no-default-features`. Every crate root denies `unwrap`/`expect`/`panic`/
`todo`/`unreachable` in production code (tests may); `unsafe_code` is forbidden
workspace-wide except `negraw`'s FFI module under the `libraw` feature.
Input-derived numbers (file sizes, sidecar values, JSON) are hostile: clamp,
check, index through `get()`.

## The image pipeline

End-to-end: **decode → orient → analyze (CPU) → derive params (CPU, µs) →
render (GPU) → display/export**. Slider changes re-run ONLY derive+render.
Analysis re-runs only when its keyed inputs change. The math is specified
formula-by-formula in PLAN §3–§4 and the module headers (each cites the NegPy
commit it mirrors); what follows is the map and the invariants.

### 1. RAW decode (`negraw`)

Output is scene-linear, sensor-native camera RGB (`RgbImage`: interleaved
f32, row-major, nominally [0,1]): **unity white balance, no camera matrix**,
black-subtracted, white-normalized with LibRaw's `maximum = 2^bits − 1`
(NOT lightcraft's highlight-linearity white — 1.44× brighter on an EOS R100),
LibRaw's image area (no default crop), EXIF orientation baked
(`applying_flip`, dcraw codes 3=180°, 5=90°CCW, 6=90°CW). Highlight
reconstruction off; no lens corrections.

- **Preview**: 2×2 binned linear develop for Bayer (= LibRaw `half_size`);
  X-Trans bins 3×3 when it can, else full demosaic + downsample. The preview
  is kept at native binned size and the Session downsamples it to the
  **1536 px proxy** (that native buffer IS the medium HQ tier, retained under
  a 20 MP budget — X-Trans bodies get no medium tier and fall through to full).
- **Full** (export / HQ on): AHD for Bayer, the X-Trans method for X-Trans.
- Backends implement `RawBackend` (`decode_preview`, `decode_full`,
  `thumbnail` = embedded JPEG bytes, `probe`). The parity fixtures start at the
  linear image, so the suite is decode-independent.
- **User orientation** (`rotation` 0/90/180/270 CW + `flipHorizontal`) is
  baked into pixels right after decode (`RgbImage::oriented`), so analysis,
  display-space rects, rendering and export share one coordinate space.

### 2. Analysis (`negkit`, CPU, on the ≤1536 px proxy)

**Fine-rotation invariant:** analysis runs on the orientation-only image
(90° steps + flip), never the fine-rotated one — the inscribed auto-crop
changes with the straighten angle and re-metering it would drift the
conversion as the user rotates. Rendering/export still bake the angle.
**Analysis ALWAYS runs on the proxy**, so HQ tier switches can never move the
conversion and export matches the screen (the what-you-see invariant).

Two stages, both offset-independent (white/black-point drags re-run NO
analysis; the offsets fold into `final_bounds` at derive time only).
Every order statistic funnels through `negkit::stats` (radix sort — 10× faster
than the library sorts it replaced); never `.sort()` pixel data.

**`kernel::prepare(linear, crop_rect, analysis_rect, buffer)` → `Prepared`**
(cached per rect state):
1. **Region priority** (NegPy `resolve_analysis_region`): a freehand
   `analysisRect` wins and disables the buffer inset; else the output
   `cropRect` scopes metering with the buffer applied inside; else the centred
   buffer alone. Default buffer **0.10** (NegPy ships 0.05 — deliberate).
   Degenerate rects (<2 px) are ignored.
2. **Prefilter** (`prefilter`): crop, `log10(clip(x, 1e-6, 1))`, then a
   **block-median grid** (block side `ceil(maxDim/1024)`; per-cell per-channel
   median kills dust/speculars and makes stats resolution-invariant; the
   proxy always hits b=2, closed-form median-of-4).
3. **Bounds** (`bounds`, NegPy `analyze_log_exposure_bounds`): the luma axis
   (clip 0.01%) sets centre+span; the colour axis (clip 1.0%) sets per-channel
   cast offsets relative to the median channel, with **same-pixel colour
   floors** (chroma-gated luma-extreme band, two-pass provisional-gamma
   refinement, float64 end to end; percentile fallback when the band has no
   trustworthy neutrals). → `LogNegativeBounds { floors, ceils }`.
4. **Meters** (`meters`, on the grid's **textured cells** only — Boyack &
   Juenger sector voting, all-cell fallback below 5%): `anchor` (auto-exposure
   key), `textural_range` (Auto Contrast), `shadow_point`/`highlight_point`
   (P99/P2 — Shadow Reach / Highlight Hold), `shadow_refs` (P98 per channel —
   cast-removal fallback).

**`kernel::finalize(prepared)` → `ExposureAnalysis`**: the **neutral axis**
measured against the PRE-trim base bounds (NegPy 2125a34: the film's cast is a
source property; creative WP/BP trims must not perturb it). Two-pass
(0.43): three luma bands, lowest-RMS-chroma 30% per band, loose cap 0.55 →
affine R/B→G correction → strict cap 0.29 → per-channel raw-log medians;
`confidence = tightness × sampleSize × agreement` gates Auto cast removal.

### 3. Parameter derivation (`negkit::derive::derive_render_params`, µs)

`(ExposureSettings, ExposureAnalysis)` → `RenderParams` (the full GPU uniform
payload; PLAN §3.4). Order matters: `final_bounds = base + (wp, bp)`;
`d_min = paperDmin ? 0.06 : 0`; `strength = confidence × slider`; per-channel
curve params (`curve_logic` — the C-41 gray-balance heart: green is the
reference, R/B fitted to its neutral axis, quadratic through 3 matched points
with a highlight ref else a 2-point line; shadow-ref fallback; base mode);
`base_slope` from grade (ISO R 50–180, 115 ≈ grade 2) or the Alkofer Auto
Grade on `textural_range`, raised by **Shadow Reach** until P99 prints ≥ 1.9 D;
pivot solved so the reference tone prints at 0.74 D; **overall contrast**
folds exactly into slopes/curvatures/pivot; **Highlight Hold** burn
(`auto_highlight`, derive-computed, never a settings field); `cmy_offsets`
(WB filtration + Temp along the Planckian direction + Tint on magenta +
Exposure stops in the EV domain); grade-coupled knees; band CMY; tone-control
passthrough (shadowContrast's negative side remapped to the −0.8 monotone
floor); `mask_val_scale` (Contrast Mask; `.zero` at gamma 0 gates the
feature); `levels_points` sanitized (sorted, 0.02 off the endpoints, monotone,
≤ 8 per channel).

The Contrast Mask PLANE is per-image data, not a param: `contrast_mask`
builds a Gaussian-blurred zero-mean normalized-val luma field from the PRINTED
frame (σ = maskSpacer% × grid short side), normalized against the BASE
bounds; the session caches it keyed on (orientation, rects, spacer, analysis
identity) and the kernels sample it bilinearly at half-pixel centres so proxy,
HQ and export renders sample one plane identically.

### 4. GPU render (`neggpu`) and its CPU twin (`negkit::reference`)

**One shader source**: `crates/neggpu/src/wgsl/negpipeline.wgsl` holds all six
kernels as `@compute` entry points — `normalize`, `print_curve`, `color_pop`,
`histogram`, `encode_f`, `encode_u8`. wgpu/naga compiles it to MSL/SPIR-V/HLSL
at runtime: no per-platform shader, no build-time compiler, no checked-in
binaries. `negkit::reference` is the CPU chain and **the oracle**: every GPU
is pinned to it (`neggpu/tests/parity.rs`: mean < 0.01, max < 0.04 per
control group; contrast mask mean < 1e-3, max < 0.02 incl. a 3× plane-size
case; rgba8 within 1.5/255 of float). `negsession::Renderer` is a trait —
`CpuRenderer` (reference, rayon rows) or `GpuRenderer`; no adapter → CPU.

Passes, in order: **normalize** (`log10(max(c,1e-6))`, per-channel stretch by
`final_bounds`) → **print_curve** (pre-saturation; `+cmy_offsets` (+ mask
sample × `mask_val_scale`); quadratic core with the **vertex hold** (NegPy
d4dc3e49); midtone paper-S; tone controls in the **PARALLEL form** (masks on
the incoming v — the GPUs were always parallel; the Swift CPU reference was
sequential, a divergence the port resolved in the GPU's favour); 3-band CMY;
Highlight Hold burn; shoulder/toe softplus knees; Print Saturation; `10^−D`;
True Black BPC referenced to the physical d_max → linear reflectance) →
**color_pop** (CIELAB, Adobe RGB (1998) primaries, D65: Hue Trim → Color Mixer
(chroma-gated R/Y/G/B bands) → vibrance → gamut-aware saturation → Skin
Protection) → **histogram** (4×256 atomic bins over the OETF-encoded,
levels-remapped output; RAW COUNTS — anything drawn from them must be a
function of SHAPE only: peak-normalize then a FIXED compression constant) →
**encode** (Adobe RGB TRC 563/256, then the per-channel piecewise-linear
levels remap; flags bit 0 = sRGB leg: decode TRC → gamut matrix → piecewise
sRGB OETF after the remap; `encode_u8` packs RGBA8 for display).

Host discipline (PLAN §2): interleaved-RGB storage buffers (upload/readback
are memcpys), 1-D dispatch workgroup 256, uniforms in persistent buffers
written per frame, per-size intermediates cached (≤ 4 MP retained, export
sizes dropped), ONE persistent staging buffer per size for readback
(`render_display_into(&mut [u8])` is the zero-allocation path), histogram
cleared with `clear_buffer`. The app shares eframe's wgpu device
(`GpuContext::from_existing`) and presents through a leased texture ring with
no CPU readback.

**Uniform layout is pinned**: `neggpu::uniforms` `#[repr(C)]` structs must stay
byte-identical to the WGSL structs (`NormUniforms` 48 B; `CurveUniforms` 304 B
with `autoHighlight` at 268, `maskScale` at 272; levels buffer 51 floats:
`[c*16+0..8]` inputs, `[c*16+8..16]` outputs, `[48+c]` counts).
`neggpu/tests/layout.rs` pins every offset — update both sides and the test
together.

**Big-buffer discipline**: never `vec![0; n]` a pixel buffer the next lines
fully overwrite (`RgbImage::new_uninit_with`, `Vec::with_capacity` +
`extend_from_slice`, `bytemuck::cast_slice` for views). No per-frame
allocations in the slider path.

### 5. Colour management

Working space is **linear Adobe RGB (1998) → Adobe-RGB-encoded** at the end
of the chain (NegPy b3490eb: ProPhoto's imaginary primaries inflated chroma
and skewed hues). Display and export conversion run through **`craft-cms`**
(pure-Rust ICC v2/v4, relative colorimetric + BPC) with NegPy's CC0
`sRGB-v4` / `AdobeCompat-v4` profiles; exports are tagged with the chosen
profile. Export (`negsession::export`): sRGB default, Adobe RGB selectable;
JPEG 8-bit (q 0.92) or TIFF-16; optional long-edge resize (`downsampled`, the
area filter); next-to-source or a chosen folder; temp-file + rename so a
failed encode never truncates the previous file.

### 6. The session (`negsession::session`) and the front end

The per-image cache tower, keyed at every tier so slider drags rebuild
nothing: decode (per image) → oriented preview (per orientation) →
`Prepared` + `ExposureAnalysis` (per rects) → GPU source per crop/tier →
mask plane (per orient/rects/spacer, only while the mask is on). Three
display tiers (`RenderTier`: proxy 1536 px · medium-if-free · full) resolved
by the HQ mode (off / **auto** (default; medium at ≥ 2× canvas zoom) / on);
the proxy stays on screen through any swap. Tier-swap stability: lay out
from the continuous `display_aspect`, place each bitmap at the window it
really covers (`content_window`, measured against ITS OWN frame), and
`downsampled` is a true area filter (cv2 INTER_AREA) so the proxy is never
warped. Tool previews ride an `uncropped` RENDER flag, never a settings
substitution — widening the render must not widen the METER (the crop tool
renders uncropped + fine rotation live; the analysis tool substitutes
fineRotation 0, a true substitution by the meter invariant).

What a front end must do itself (the `negsession` crate header has the
authoritative list): session LRU of 2 with `release_enhanced_tiers` on the
off-screen one; latest-wins render coalescing; sidecar saves debounced 1 s and
never while restoring a frame or in profile-editor mode; history per path
(commit on drag release or 700 ms after a non-drag tick); HQ up-swap
debounced 350 ms.

- **Sidecars**: `<basename>.swiftinvert.json` next to the source,
  `{version, settings}`; missing keys decode to defaults, a wrong-typed field
  decodes to ITS default (lenient per-field decoder — a deliberate divergence
  from the Swift all-or-nothing decode); retired keys (`dyeSeparation`, …)
  ignored; legacy `.negswift.json` read as fallback and removed on save.
  Atomic writes (unique temp + fsync + rename).
- **Settings**: `ExposureSettings` — exactly **54** stored camelCase fields
  (`STORED_KEYS`, `STORED_PROPERTY_COUNT`), defaults listed in PLAN §3.2.
- **Profiles** (`profiles`): "None" (stock) and "SwiftInvert Default" (the house
  look) reserved from code; user profiles + active choice persisted; applied
  wherever a frame is seen fresh. Stock `ExposureSettings::default()` stays
  NegPy-parity-neutral — fixtures, negcli and the parity suite never see profiles.
- **History** (`history`): labels from `history_label(a, b)` — every settings
  field needs a line there.
- **Prefs**: eframe persistence, split keys `swiftinvert.prefs` / `.export` /
  `.profiles` (`negui::app`).
- **UI** (`negui`): the sidebar is generated from `control_spec.rs`, a static
  table (field, label, section, range, default, format, display transform,
  disabled rule, gradient track) swept by the tests against `STORED_KEYS`, so
  a new field cannot ship without a row or an explicit "no UI" entry. macOS
  gets the system menu bar (`native_menu.rs`). Darkroom features: densitometer
  + zone strip (probe reads the displayed bytes, inverting the sRGB leg),
  zone overlay, test strip (5×5 real renders, Brightness across / Grade down,
  two-stage pick), zone placement (bisection against the 1-px green-reference
  model through the real kernel), interactive histogram (levels anchors by
  direct drag). Known gaps are listed in `HANDOFF.md`.
- **Verification limits**: synthetic pointer events never reach the app
  window, so hover/pointer paths are verified by `--selftest` screenshots
  and unit tests of the mapping math, and handed to a human for the gesture.

## Parity with NegPy

`fixtures/` is dumped from NegPy's actual engine by `scripts/dump_fixtures.py`
(run in NegPy's uv env; `negkit::fixtures` locates them from
`CARGO_MANIFEST_DIR`). The suite verifies every stage boundary:
- closed-form oracles (percentile semantics, grade/pivot/softplus/OETF math),
- `synthetic64` (NegPy's golden image): prefiltered grid @1e-5,
  bounds/meters/curve-params @1e-4, full CPU chain @1e-4,
- `synthetic_grid` (1600×1066, regenerated bit-exactly from an integer-hash
  formula): b=2 block-median + the quadratic cast-removal path (slope/pivot
  tolerance 2.5e-4),
- `ramp257` print-curve shapes, `lab_color` (vibrance/saturation vs NegPy's
  CIELAB ops), `contrast_mask` (plane / expansion / masked chain at zero offsets),
- GPU vs CPU reference at NegPy's own gates (above).
Test map (Swift test file → Rust file, same names and tolerances): PLAN §5.
Drift-catchers: `negkit/tests/sidecar.rs` pins the stored-property count (54)
and mutates every field; `negui/tests/shell.rs` sweeps the control table.

**Upstream review log: `UPSTREAM.md`** — the last NegPy commit reviewed and
the port/skip decisions per review. For "what changed in NegPy?" run the
**`/negpy-review` skill** (`.claude/skills/negpy-review/`).

Analysis semantics and kernel constants are synced with **NegPy 0.58.0**
(`dc8ac65f` tip, plus the `d4dc3e49` quadratic vertex hold from 0.62.0; the
8532dd92 print-tone + auto-helpers retune: toe sharpness 6.0 / paper midtone
gamma 0.05, textured-cell metering, Alkofer Auto Grade, Shadow Reach,
Highlight Hold; 127bcd7 two-pass estimator + same-pixel colour floors;
2125a34 pre-trim neutral axis; b3490eb Adobe RGB working space; the 0.38 set
with `paper_dmin` off + `true_black` on). ALL fixtures were dumped from
`dc8ac65f` (the manifest records `paper_dmin`/`true_black` per config;
`expo_dark` fires Highlight Hold at its 0.5 cap). NegPy's per-layer R/G/B
trims, Split Grade and Zone Density are NOT ported — our tone controls + 3-band
grading cover the achromatic cases (Zone Density's highlight term exists only
as Highlight Hold's actuator, with no user slider).

**Deliberate divergences from NegPy** (fixture tests pin the NegPy-neutral
values where needed):
- `preSaturation` default **1.15** (no NegPy equivalent; parity configs set 1.0),
- default analysis buffer **0.10** vs NegPy 0.05 (tests pass 0.05 explicitly),
- **Contrast Mask plane normalizes against the BASE bounds** (pre-WP/BP
  offset) so white/black-point drags stay analysis-free; identical at zero
  offsets (every fixture config). The mask slider is NOT display-inverted
  (right = + = squeeze = softer, matching our Grade's right = softer),
- the tone controls' PARALLEL form (above),
- NegPy's default lab sharpen (0.25) is not implemented,
- **vibrance is SwiftInvert-maintained** (upstream deleted theirs; the
  lab_color vibrance fixture is frozen at the 0369b10 dump — the reference
  formula lives in `dump_fixtures.py` and cannot be re-dumped from newer NegPy),
- Hue Trim is an ordinary sidecar setting (upstream makes it sticky across
  frames — bake it into a profile for the same effect),
- SwiftInvert-only controls: exposure stops, tone controls, overall contrast,
  temp/tint, 3-band colour grading, pre-saturation, Color Mixer, the
  interactive-histogram levels remap — all identity-at-default so the NegPy
  fixtures still pass.
- Zone placement's knee solve bisects the composite residual (upstream
  alternates passes); zone-pin sampling freezes normalized-log luma.

Decisions that differ from the retired Swift tree (PLAN §0, HANDOFF): the
decoder defaults to LibRaw's white level and image area; the CPU reference is
parallel-form; the port follows the Mac app wherever it differed from the Qt
bridge (tool preview tiers, meter angle keying, medium→full fallthrough,
unbounded history); sidecars are decoded leniently per field.

## Adding a new adjustment control (checklist)

1. `negkit::settings::ExposureSettings`: field + default + serde default +
   its entry in `STORED_KEYS` (bump the count) — the sidecar tests fail until
   the field list, the mutation sweep and `history_label` all have their line.
   Add to `RenderParams` (+ `derive`) if the kernel needs it.
2. `derive_render_params`: map settings → params; fold into existing params
   where the algebra allows (overall contrast / exposure are the models)
   before adding uniforms.
3. Kernels — **TWO mirrors in the same change, identical order and
   parallel form**: `negkit::reference` (CPU, the source of truth) and
   `negpipeline.wgsl` (all backends). New uniforms: extend `CurveUniforms`
   (16-byte alignment) in `neggpu::uniforms` AND the hand-written WGSL
   struct, then `tests/layout.rs`. Guard non-default-off work behind
   uniform branches (occupancy).
4. UI: a row in `negui::control_spec` (section, range, default, reset,
   disabled rule, track); a `negcli render --set key=value` already reaches
   any field by name.
5. Tests: default-is-identity (fixtures must still pass), direction/region
   properties, monotonicity if it touches the tone curve, sidecar
   round-trip, GPU-parity case with the control active.
6. `./ci.sh`, `negcli bench` (watch for regressions), `--selftest`, commit
   with the reasoning; update this file's pipeline section.

## Constants sync points

- `negkit::constants::K` is the single source for the analysis/derive
  constants; the WGSL duplicates the kernel-side literals (tone anchors and
  sharpness, Lab matrices/eps/kappa/white, the OETF exponent 256/563 and the
  Color Mixer band constants). GPU/CPU parity catches drift — update them
  together.
- If NegPy's `EXPOSURE_CONSTANTS` change deliberately: update `K`, re-dump
  fixtures, re-run `./ci.sh`, record it in `UPSTREAM.md`.
