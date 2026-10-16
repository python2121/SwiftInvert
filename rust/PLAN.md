# SwiftInvert → Rust + egui + wgpu: the port plan

Status: **living document** — the orchestrating session and every agent update the
"Progress" table at the bottom as work lands. Written 2026-10-15.

## 0. Decisions already made (don't relitigate)

| Decision | Choice | Why |
|---|---|---|
| Language / UI / GPU | Rust 2024, egui 0.36 (eframe), wgpu 30 compute with **WGSL** | One codebase for macOS / Linux / Windows (and WebGPU later); wgpu abstracts Metal / Vulkan / DX12. |
| Where the code lives | `rust/` inside this repo, Cargo workspace, branch `rust-port` | Shares `Tests/Fixtures` by relative path; Swift/Qt apps keep shipping until parity; `main` untouched. |
| UI donor | `storytold/lightcraft` (Lightroom clone, MIT/Apache) and the shared `craft-ui` kit — NOT wordcraft's ribbon | Same shape as us: filmstrip, loupe, right-hand slider panel, crop tool, histogram, export. |
| Not a lightcraft plugin | Standalone app that consumes their crates | They have no plugin system; their planned SDK is export/metadata-shaped; **licence**: SwiftInvert is GPL-3 (NegPy port), lightcraft forbids GPL code. GPL may consume MIT/Apache, not the reverse. |
| First RAW backend | `lightcraft-raw` (pure Rust) behind a `RawBackend` trait; LibRaw FFI as cargo feature `libraw` | Host (SteamOS) has no LibRaw headers / clang. The parity fixtures start at the linear image so the suite is decode-independent. LibRaw stays the byte-exact-vs-NegPy reference where it exists (Mac, distrobox). |
| Colour management | `craft-cms` (pure Rust ICC v2/v4, BPC, relative colorimetric, 3-D LUT) | Closes the long-standing "littleCMS conversion path" gap; no C. |
| Math precision | analysis/derive in `f64`, images in `f32` | Mirrors Swift (`Double` params, `Float` pixels); fixture gates unchanged. |
| Naming | crates `negkit`, `negraw`, `neggpu`, `negsession`, `negui`; binaries `negcli`, `swiftinvert` | App rename is the user's call later; binary name kept so docs stay true. |
| Commits | Orchestrator commits at wave boundaries on `rust-port`; agents never commit | Reversible; `main` is pristine. |

Reference implementation for every formula: the Swift sources in `../Sources/NegativeKit`
(+ CLAUDE.md §"The image pipeline"). NegPy itself is not on this machine; **the fixtures are the
oracle**. When Swift and CLAUDE.md disagree, Swift + the fixture test wins.

## 1. Architecture and layering

```
L0  (std, serde)                     — no crate of our own
L1  negkit      pure math            — port of NegativeKit. Deps: serde, serde_json, thiserror, rayon (feature "rayon", default on, off for wasm). NO io, NO gpu, NO ui.
L2  negraw      RAW → RgbImage       — RawBackend trait; backends: lcraw (default, lightcraft-raw git), libraw (feature, FFI); embedded thumbnail JPEG bytes.
L3  neggpu      wgpu compute         — WGSL twin of negkit::reference. GpuContext (headless | from existing device), RenderPipeline (render / render_display / render_display_into), uniform packing, histogram.
L4  negsession  engine               — ImageSession cache tower, settings/sidecars/profiles/history models, Renderer trait (Cpu | Gpu), export (TIFF16/JPEG + ICC via craft-cms), library scan. No egui.
L5  negui       egui front end       — eframe app: library tree, loupe, controls table, histogram, tools, dialogs, control channel. Reads negsession only.
apps/negcli     headless CLI         — decode / thumb / render / bench / meter / parity-dump (L4 user).
apps/swiftinvert desktop binary      — eframe shell + platform glue (window wgpu setup shared with neggpu).
```

Rules (checked by review, later by an xtask):
- Nothing below L5 imports egui/eframe/winit/rfd. Nothing below L3 imports wgpu.
- `negkit` must build for `wasm32-unknown-unknown` with `--no-default-features` (no rayon, no std::time).
- Production code: `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo, clippy::unimplemented, clippy::unreachable)]` on every crate root; `unsafe_code = "forbid"` workspace-wide EXCEPT `negraw` with feature `libraw` (FFI isolated in one module with `// SAFETY:` per call).
- Input-derived numbers are hostile: sizes from files/sidecars/JSON are clamped and checked; indexing through `get()` where the index comes from data.

## 2. Sharing GPU code across platforms (and staying fast)

**One shader source.** `neggpu/src/wgsl/negpipeline.wgsl` holds all six kernels as `@compute`
entry points in ONE module: `normalize`, `print_curve`, `color_pop`, `histogram`, `encode_f`,
`encode_u8`. wgpu/naga compiles it to MSL / SPIR-V / HLSL at runtime; there is no per-platform
shader, no build-time compiler, no checked-in binaries (the SPIR-V-in-git discipline dies here).

**One uniform layout, pinned.** `neggpu/src/uniforms.rs` declares `#[repr(C)] bytemuck::Pod`
structs that are byte-identical to the existing std140 layout (`NormUniforms` 48 B,
`CurveUniforms` 304 B with `autoHighlight` at 268 and `maskScale` at 272, levels buffer
51 floats). `tests/layout.rs` pins every offset/stride. The WGSL struct is written by hand in the
same order; naga validates alignment at pipeline creation, the test pins Rust, parity pins both.

**Buffers, not textures, for the chain.** Interleaved RGB `f32` storage buffers = `RgbImage`'s own
layout, so upload and readback are memcpys (the Vulkan port's lesson). 1-D dispatch, workgroup 256.
Push constants are a native-only wgpu feature → the `{n, flags}` pair rides a 16-byte uniform
buffer instead (portable to WebGPU; one tiny write per dispatch).

**Memory discipline (from the 3.9 s/frame lesson).**
- Per-size intermediates (`normalized`, `curved`, `encoded`) cached in a map keyed by (w,h); ≤ 4 MP retained, export sizes dropped after use.
- Readback goes through ONE persistent `MAP_READ | COPY_DST` staging buffer per size; `map_async` + `device.poll(Wait)`; the mapped slice is copied straight into caller memory (`render_display_into(&mut [u8])` — the `si_render_into` zero-allocation contract) and unmapped.
- Histogram: `array<atomic<u32>, 1024>` cleared with `clear_buffer` each frame; read back with the display bytes in the same submission.
- Never allocate a big `vec![0; n]` the next line fully overwrites: `Vec::with_capacity` + `set_len`-free patterns (`extend_from_slice`, `resize` only when the fill IS the content), `bytemuck::cast_slice` for u8↔u32 views.
- No per-frame allocations in the slider path: uniforms are written with `queue.write_buffer` into persistent buffers; bind groups cached per (size, mask-on/off).

**Zero-copy display in the app.** `GpuContext::from_existing(device, queue)` lets the app share
eframe's wgpu device (`WgpuSetup::Existing`, what lightcraft does). `encode_u8` writes packed
RGBA8 into a storage buffer; the app `copy_buffer_to_texture`s it into an `rgba8unorm` texture
registered with egui-wgpu (`register_native_texture`) → the loupe paints it with no CPU readback at
all. Headless users (negcli, tests, Qt-style consumers) use the readback path. Both paths run the
identical kernels and submission.

**CPU fallback is the oracle, not a second implementation.** `negkit::reference` IS the CPU chain
(the Swift `ReferenceCurve` port). `negsession::Renderer` is a trait with `CpuRenderer`
(reference, rayon over rows) and `GpuRenderer` (neggpu). No adapter → CPU, same output within the
parity gates (mean < 0.01, max < 0.04 vs CPU; the rgba8 path within 1.5/255 of float).

**Backends per OS.** wgpu features: `metal, vulkan, dx12, wgsl` — no GL. Linux → Vulkan,
macOS → Metal, Windows → DX12 (lightcraft's #136 lesson: don't let wgpu enumerate drivers you
don't intend to use; `NEG_GPU_BACKEND=vulkan|metal|dx12|off` env override). `NEGCLI_VK_CPU=1`'s
successor: `NEG_GPU=off` forces the CPU renderer; tests that need a GPU call a `skip!`-style
helper and report the skip instead of failing silently.

**Performance budget** (targets = today's measured numbers on the NAVI33 Steam Machine; the
Rust port must match or beat them, `negcli bench` prints them):

| Path | Today | Target |
|---|---:|---:|
| proxy 1536 px render_display incl. histogram + sRGB, readback | 2.1 ms | ≤ 2.5 ms |
| medium tier (≈3000 px) | 14 ms | ≤ 15 ms |
| full 24 MP display | 55 ms | ≤ 60 ms |
| 24 MP export render (float) | 0.39 s | ≤ 0.45 s |
| `prepare` (analysis, 1536 proxy) | 17 ms | ≤ 20 ms |
| `finalize` | 7 ms | ≤ 8 ms |
| `derive_render_params` | µs | µs |
| CPU reference chain, 1536 px | 509 ms | ≤ 300 ms (rayon rows) |

## 3. Data contracts (names are law — tests and sidecars depend on them)

### 3.1 `RgbImage` (negkit::image)
`{ width: usize, height: usize, data: Vec<f32> }`, interleaved RGB, row-major, values nominally
[0,1] scene-linear. Constructors: `new_uninit_with(w,h, |slice| …)` (the big-buffer discipline),
`from_vec`. Ops (all pure, all tested against the Swift semantics): `oriented(rotation_cw: 0|90|180|270, flip_h)`,
`applying_flip(dcraw_code)` (3=180°, 5=90°CCW, 6=90°CW), `flipped_horizontally`,
`fine_rotated(degrees)` → inscribed rect (floor on this tier's grid), `inscribed_rect_size`,
`downsampled(max_long_edge)` = separable fractional-weight **area filter** (cv2 INTER_AREA; `area_taps`),
`cropped(NormalizedRect)` via `pixel_roi` (truncates).

### 3.2 `ExposureSettings` (negkit::settings) — the sidecar schema
Exactly the 54 stored properties of the Swift struct, same **camelCase JSON keys**, same defaults;
`#[serde(default)]` on every field (missing key = default: sidecar back-compat); retired keys
(`dyeSeparation`, …) are ignored on decode. `SIMD3<Double>` → `[f64; 3]` (JSON array),
`SIMD2<Double>` → `[f64; 2]`, `NormalizedRect` → `{x, y, width, height}` (check `NormalizedRect.swift`
Codable keys and `SidecarCodecTests` for the exact JSON before writing). Sidecar file:
`<basename>.swiftinvert.json` with `{ "version": N, "settings": {…} }` (legacy `.negswift.json` read
as fallback, removed on next save). Field list (defaults):

density 1.0 · grade 115 · wbCyan/wbMagenta/wbYellow 0 · autoExposure true · autoNormalizeContrast true ·
castRemovalStrength 0.5 · whitePointOffset/blackPointOffset 0 · toe 0 · toeWidth 2.5 · shoulder 0 ·
shoulderWidth 2.5 · paperDmin false · trueBlack true · exposureStops 0 · shadows 0 · shadowContrast 0 ·
darkShadows 0 · highlights 0 · highlightContrast 0 · overallContrast 0 · vibrance 1 · saturation 1 ·
skinProtection 0.5 · redHue 0 · redSaturation 1 · yellowHue 0 · yellowSaturation 1 · greenHue 0 ·
greenSaturation 1 · blueHue 0 · blueSaturation 1 · printSaturation 1 · separationDamping 0 ·
contrastMask 0 · maskSpacer 4 · hueTrim 0 · preSaturation 1.15 · temp 0 · tint 0 · colorShadows/colorMids/
colorHighs [0,0,0] · levelsRed/Green/Blue [] · rotation 0 · flipHorizontal false · fineRotation 0 ·
analysisRect null · analysisRectFineRotation 0 · cropRect null.

A `stored_property_count()` test pins 54 and a mutation test touches every field (the Swift
drift-catchers, `SidecarCodecTests` + `HistoryLabelTests`).

### 3.3 Analysis types (negkit::analysis)
`LogNegativeBounds { floors: [f64;3], ceils: [f64;3] }` (+ `applying_offsets(wp, bp)`);
`Prepared { grid: RgbImage, base_bounds, anchor, textural_range, shadow_point, highlight_point, shadow_refs: [f64;3] }`;
`ExposureAnalysis { base_bounds, anchor, textural_range, shadow_point: Option<f64>, highlight_point: Option<f64>, shadow_refs, neutral_mid/shadow/highlight: Option<[f64;3]>, neutral_confidence: Option<f64> }` — serde with the Swift keys (the fixtures' `analysis.json` is read through it).

### 3.4 `RenderParams` (negkit::derive) — the GPU payload
final_bounds, slopes/pivots/curvatures/cmy_offsets [f64;3], toe_eff, shoulder_eff, toe_width,
shoulder_width, d_min, v_star, shadows, shadow_contrast, dark_shadows, highlights,
highlight_contrast, auto_highlight, vibrance, saturation, skin_protection, band_hues [f64;4],
band_saturations [f64;4], pre_saturation, print_saturation, separation_damping, hue_trim (radians),
mask_val_scale [f64;3], true_black: bool, shadow_cmy/mid_cmy/highlight_cmy [f64;3],
levels_points: [Vec<[f64;2]>; 3] (pre-sanitized: sorted, 0.02 off the endpoints, monotone, ≤ 8).

### 3.5 Uniform packing (neggpu::uniforms) — mirrors `ShaderTypes.swift` exactly
`NormUniforms { floors: [f32;4], ceils: [f32;4], wp: f32, bp: f32, _pad: [f32;2] }` = 48 B.
`CurveUniforms`: pivots, slopes, curvatures, cmyOffsets, shadowCMY, midCMY, highlightCMY, dMinRGB
(8 × vec4 = 128 B), then 24 scalars in the Swift order (toe … printSaturation, offsets 128→224), bandHues, bandSaturations
(vec4 ×2), separationDamping, skinProtection, hueTrim, autoHighlight (268), maskScale (272), maskDims (288) → 304 B.
Scalar constants folded at pack time exactly as `UniformsBuilder.curveUniforms` does (K.toeShoulderStrength,
K.dMax, K.toeSharpnessBase, …). Levels buffer: `[c*16+0..8] inputs, [c*16+8..16] outputs, [48+c] count`.
Flags word: bit 0 = sRGB display/export encode (decode Adobe TRC → gamut matrix → piecewise sRGB OETF after the levels remap).

### 3.6 Decode contract (negraw) — LibRaw semantics expressed in lightcraft-raw terms
Output = sensor-native camera RGB, **unity white balance, no camera matrix**, black-subtracted,
white-normalized to [0,1] with **LibRaw's `maximum` = `2^bits − 1`** (NOT lightcraft's
highlight-linearity `white`: that read 1.44× brighter on an EOS R100, measured 2026-10-15), EXIF
orientation baked (`applying_flip` equivalent from `RawImage::orientation`), **LibRaw's image area
(no default crop)** — 6024×4020 on the R100 where the DNG-style crop gives 6000×4000.
- **Preview**: `develop_binned(k=2)` for Bayer (= LibRaw `half_size` + linear: R, B, mean of the two greens per 2×2); X-Trans: `develop_binned(k=3)` when `can_bin`, else full `demosaic(Method::XTrans)` then `downsampled`. Preview kept at native binned size; the Session downsamples to the 1536 proxy (medium tier = that buffer, under the 20 MP budget).
- **Full** (export / HQ): `develop(Method::Ahd)` for Bayer, X-Trans method for X-Trans. (LibRaw default is AHD too.)
- Highlight reconstruction OFF (clip at 1.0 like `no_auto_bright` + `adjust_maximum_thr=0`); no opcodes beyond what `normalized()` applies (document which OpcodeLists the backend applies; lens corrections are NOT wanted).
- Thumbnail: `embedded_preview(bytes)` JPEG bytes (the Qt/Mac library grid decodes them).
- `RawBackend` trait: `decode_preview(path) -> RgbImage`, `decode_full(path) -> RgbImage`, `thumbnail(path) -> Option<Vec<u8>>`, `probe(path) -> Option<RawInfoLite>`; a `libraw` feature implements the same trait over FFI (compiles only where `libraw_r` exists; `build.rs` uses pkg-config there).

### 3.7 Histogram contract
4 × 256 raw counts (R, G, B, Rec.709 luma) over the OETF-encoded, levels-remapped output; consumers
normalize to the peak bin and compress with a FIXED constant (HistogramView's shape-only rule).

## 4. Pipeline stages → Rust modules (source of truth in parentheses)

| Stage | negkit module | Swift source |
|---|---|---|
| Order statistics (radix sort funnel) | `stats`, `radix_sort` | Stats.swift, RadixSort.swift |
| Region priority + prefilter (log10 clip 1e-6, block-median grid b=ceil(maxDim/1024), median-of-4 fast path) | `prefilter`, `rect` | Prefilter.swift, NormalizedRect.swift |
| Bounds (luma axis 0.01 %, colour axis 1 %, same-pixel colour floors, float64) | `bounds` | BoundsAnalysis.swift |
| Meters (textured cells, anchor, texturalRange, shadow/highlight points, shadowRefs, two-pass neutral axis + confidence) | `meters` | Meters.swift |
| prepare / finalize / analyze | `kernel` | ExposureSettings.swift (`ExposureKernel`) |
| Constants `K`, WorkingOETF | `constants` | ExposureConstants.swift |
| Curve logic (grade↔slope, pivot, per-channel params, shadow reach, highlight hold, knees, filtration) | `curve_logic` | CurveLogic.swift |
| derive_render_params (+ sanitize_levels) | `derive` | ExposureSettings.swift |
| CPU reference chain (normalize, quadratic core w/ vertex hold, print curve, tone controls, 3-band, highlight hold, knees, print saturation, true black, encode, levels remap) | `reference` | ReferenceCurve.swift |
| Lab ops (rgb↔lab D65 Adobe, hue trim, colour mixer, vibrance, gamut-aware saturation, skin protection) | `lab` | LabColor.swift |
| Contrast mask plane (base-bounds normalized, Gaussian σ = spacer % × short side, zero-mean) + bilinear sample + val scale | `contrast_mask` | ContrastMask.swift |
| Crop geometry (rotated boxes, fitScale, constrain, remapCrop, displayAspect, contentWindow) | `crop_geometry` | CropGeometry.swift |
| Densitometry (encoded-triplet luma quirk, zones, character) | `densitometry` | Densitometry.swift |
| Zone grid / test strip / zone placement | `zone_grid`, `test_strip`, `zone_placement` | ZoneGrid.swift, TestStrip.swift, ZonePlacement.swift |

Every module header cites the NegPy commit the Swift cites. Deliberate divergences (CLAUDE.md
§"Deliberate divergences") are kept, not "fixed".

## 5. Fixtures and tests — the acceptance bar

Fixture root: `../../Tests/Fixtures` relative to `negkit/Cargo.toml` (`fixtures::path()` resolves
from `CARGO_MANIFEST_DIR`). `.bin` = float32 LE, shape from the manifest. Swift test → Rust test map
(same names, same tolerances; a Swift file's tests may be split across Rust files but none dropped):

| Swift test file | Rust location | Gate |
|---|---|---|
| ClosedFormTests | negkit/tests/closed_form.rs | exact oracles (1e-9…1e-6 as Swift) |
| RadixSortTests, AuditStatsAndLevelsTests | negkit/tests/stats.rs | exact |
| GridParityTests (synthetic64 prefiltered, synthetic_grid hash regen + prefiltered) | negkit/tests/grid_parity.rs | 1e-5 |
| AnalysisParityTests, AnalysisEquivalenceTests, AuditAnalysisEdgeTests, DensityChromaTests, AutoHelpersTests | negkit/tests/analysis.rs | 1e-4 (slope/pivot 2.5e-4 on synthetic_grid) |
| RampCurveTests, QuadraticCoreTests, TrueBlackTests, PreSaturationTests, ToneControlsTests, ColorGradingTests, LevelsRemapTests, AuditCurveFallbackTests | negkit/tests/curve.rs | per Swift |
| synthetic64 chain (normalized / curve_linear / output, 3 configs) | negkit/tests/chain.rs | 1e-4 |
| LabColorTests, HueTrimTests | negkit/tests/lab.rs | per Swift (lab_color fixtures) |
| ContrastMaskTests | negkit/tests/contrast_mask.rs | fixture gates |
| OrientationTests, PreprocessRectTests, AuditRectAndGeometryTests, ImagePipelineSeamTests | negkit/tests/geometry.rs (+ analysis.rs for the prepare-level halves) | per Swift |
| CropGeometryTests, DisplayAspectTests | negkit/tests/crop_geometry.rs | per Swift |
| DensitometryTests, ZoneGridTests, AuditZoneGridStrideTests, TestStripTests, ZonePlacementTests | negkit/tests/darkroom.rs | per Swift |
| SidecarCodecTests (+ HistoryLabelTests' field sweep) | negkit/tests/sidecar.rs | exact |
| LayoutTests / VulkanLayoutTests | neggpu/tests/layout.rs | exact offsets |
| GPUParityTests / VulkanParityTests (every control group; contrastMaskMatchesCPU incl. 3× plane) | neggpu/tests/parity.rs | mean<0.01 max<0.04; mask mean<1e-3 max<0.02; rgba8 within 1.5/255 |
| SwiftInvertTests (SidecarStore, ExportOptions.destinationURL, DensitometerState mapping) | negsession/tests | per Swift |

Also: `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `cargo test --workspace`
(GPU tests skip with a printed reason when no adapter), `cargo build -p negkit --target wasm32-unknown-unknown --no-default-features` (once the target is installed — not on this host; recorded as pending).

## 6. Work breakdown — waves, owners, acceptance

Each wave runs its agents in parallel; a wave closes when `cargo test --workspace` is green and the
orchestrator has reviewed and committed. Agents own the files listed and touch nothing else; shared
files (`lib.rs`, `Cargo.toml`, `types.rs`) are the orchestrator's — ask by reporting back, don't edit.

**Wave 0 — skeleton (orchestrator).** Workspace, crate manifests, `negkit` module stubs + shared
types (`image.rs` skeleton, `settings.rs` fields, `analysis` types, `RenderParams`), fixture helper,
`PLAN.md`, `.gitignore`, CI script `rust/ci.sh` (fmt, clippy, test). Builds green with stubs.

**Wave 1 — foundations (4 agents, negkit + negraw).**
- A `image` + `rect` + `stats` + `radix_sort` + `prefilter` (+ tests: stats.rs, grid_parity.rs, geometry.rs image/rect parts).
- C `constants` + `settings` (serde, sanitize_levels) + `curve_logic` + `derive` (+ tests: closed_form.rs, sidecar.rs, curve.rs derive-level parts).
- L `lab` (+ tests lab.rs).
- R `negraw` crate (lcraw backend, trait, thumbnail, libraw feature stub + build.rs; a smoke test on any RAW under `corpus/` if present, else skip).

**Wave 2 — analysis + reference chain (2 agents, negkit).**
- B `bounds` + `meters` + `kernel` (+ tests analysis.rs).
- D `reference` (+ tests chain.rs, curve.rs kernel parts, ramp, levels remap, tone controls).

**Wave 3 — GPU, darkroom extras, engine (3 agents).**
- G `neggpu` whole crate (context, WGSL, pipeline, uniforms, layout + parity tests, bench helper).
- M `contrast_mask`, `crop_geometry`, `densitometry`, `zone_grid`, `test_strip`, `zone_placement` (+ tests contrast_mask.rs, darkroom.rs, geometry.rs crop parts).
- S `negsession` (Session tower with keyed tiers, Renderer trait + CpuRenderer, SidecarStore, profiles, history model, export TIFF16/JPEG + craft-cms ICC, library scan) + `apps/negcli` (decode/thumb/render/meter/bench).

**Wave 4 — UI (2 agents, negui + app).**
- U1 shell: eframe app sharing the wgpu device, theme (craft-ui tokens, lightcraft look), library folder tree (depth ≤ 8, natural sort, RAW-less pruned, sticky folder), loupe (fit/zoom-to-cursor/pan/double-click 100 %, zero-copy texture), controls sidebar generated from a `ControlSpec` table mirroring `ControlsSidebar` (sections, ranges, defaults, reset-⨯, double-click reset, gradient tracks, disabled rules), histogram (shape-only rule), negative-character row, status bar, keyboard map.
- U2 tools + dialogs: crop & straighten (uncropped render, box = stored rect, Esc/Apply), analysis rect, rotate/flip, View Original hold, HQ off/auto/on badge, export dialog (sticky options, batch), history panel (undo/redo, drag commits on release, 0.7 s coalesce), Reset All, Copy/Paste adjustments, control channel + headless snapshot (`--selftest out.png`).

**Wave 5 — darkroom features in the UI (later):** test strip, densitometer + zone strip + zone
overlay, zone placement, interactive histogram window, contrast mask plane wiring, default profiles
editor. (Math already ported in M; this wave is presentation.)

**Reviews (different models, after waves 2, 3, 4):** opus — semantic review of each ported module
against its Swift source (formula-by-formula, constants, clamps, NaN paths); sonnet — code-quality /
simplification / never-crash sweep; findings fixed by the owning agent or the orchestrator; a
second opus pass on the fixes. Final: full CI, `negcli bench` numbers into the Progress table.

**Mac verification:** through the ClaudeShip hub jobs API on the "Macbook Air" swarm peer
(`docs/jobs.md`), after pushing `rust-port` to `origin` (github.com/python2121/SwiftInvert):
`git fetch && git checkout rust-port && cd rust && cargo test --workspace` on Metal. If Rust is
not installed there, report and stop — no installs on the Mac without the user.

## 7. Conventions for agents

- Read the Swift file you are porting IN FULL, and its tests. Port semantics, not syntax; keep the
  comments that cite NegPy commits and lessons (they are the project's memory).
- Doc comment on every pub item; module header with the Swift file it mirrors and the NegPy commit.
- `f64` for analysis/derive math; `f32` pixels; `as` casts only at the boundary and commented.
- No `unwrap`/`expect`/`panic`/indexing-from-data in non-test code (clippy denies it); tests may.
- Tests: Rust integration tests in `crates/<crate>/tests/*.rs`, named after the Swift test they port,
  with the Swift tolerance. Don't loosen a gate to pass — report it.
- Performance: no allocations in per-frame paths; `rayon` for per-row work in the CPU chain and
  analysis sorts (feature-gated); measure with `negcli bench` before claiming.
- Work only inside `rust/`. Never touch `Sources/`, `Tests/`, `qt/`, `Package.swift`, `.github`.
  Scratch files go to the session scratchpad, not the repo. No `cargo install`, no `sudo`, no
  global config changes, no git commits/pushes/branch changes. Don't message other sessions.
- Finish with a report: files written, tests added (count, pass/fail), gaps left, questions.

## 8. Platform matrix

| Platform | GPU backend | Build/tests run | Status |
|---|---|---|---|
| Linux (SteamOS host, RADV NAVI33) | Vulkan | here | primary |
| macOS (Macbook Air, Apple M2) | Metal | via ClaudeShip job after push | **verified 2026-10-16**: whole workspace builds, 282 CPU tests + 29 GPU parity tests pass; libraw feature links and matches lcraw at LSB level |
| Windows | DX12 | — | untested; wgpu-level only |
| Web (wasm32) | WebGPU, CPU fallback | negkit only at first | later |

## 9. Open questions for the user (not blocking)

1. App/product name once Rust is primary (binary is still `swiftinvert`).
2. Keep the LibRaw FFI backend long-term (byte-exact vs NegPy) or go pure Rust only?
3. egui will not look native on macOS; lightcraft adds its own title bar and menu glue. Acceptable?
4. When the Rust app reaches the Qt shell's feature set, retire `qt/` and `CoreBridge`?

## 10. Progress

| Wave | Item | Owner | State | Notes |
|---|---|---|---|---|
| 0 | skeleton, contracts, plan | orchestrator | done | af70305 |
| 0 | controls + session specs | spec agent | done | `rust/docs/*.md`, 8 Swift-vs-CLAUDE.md inconsistencies listed there |
| 1 | A image/rect/stats/radix/prefilter | agent A | done | 43 tests; prefiltered grids 0 / 4.8e-7 vs 1e-5 gate |
| 1 | C constants/settings/curve_logic/derive | agent C | done | 41 tests; curve params parity 3 configs @1e-4; lenient per-field sidecar decoder |
| 1 | L lab | agent L | done | 27 tests; lab_color fixtures ≤ 3e-7 vs 1e-4 |
| 1 | R negraw | agent R | done | lcraw default = LibRaw area + bit-depth white; **Mac parity vs LibRaw 0.22.1: per-sample max 6.3e-5, mean 2.7e-5, same dims** (EOS R100 CR3); `libraw` feature builds + tests on the Mac |
| 1 | review (Opus) | reviewer | done | 1 BUG (pixel_roi debug overflow) + NaN-handling risks, all fixed in 54340ee |
| 1 | Mac run | orchestrator | done | full default workspace builds, all tests pass on macOS (Rust 1.96) |
| 2 | B bounds/meters/kernel | agent B | done | 26 tests; analysis parity 3 configs + synthetic_grid; prepare 13.7 ms / finalize 4.5 ms after the prefilter rework |
| 2 | D reference chain | agent D | done | 42 tests; full chain ≤ 2.4e-7 vs 1e-4 (fixture AND live analysis); 1536 px render 54 ms (Swift 509) |
| 2 | reviews (Opus ×2: reference, analysis) | reviewers | done | no bugs; tone controls moved to the GPU's parallel form (Swift CPU was sequential); live-chain test added |
| 3 | M contrast mask / crop geometry / densitometry / zones / test strip / zone placement | agent M | done | 83 tests; plane 3e-7, masked chain 1.5e-7; negkit now 267 tests, fully ported |
| 3 | G neggpu | agent G | done | 34 tests; GPU vs CPU ~1e-7 mean on every control group (gates 0.01/0.04); rgba8 ≤ 0.5/255; histogram exact; **Vulkan (NAVI33): 1.68 ms proxy frame, 0.242 s 24 MP; Metal (Apple M2): 3.09 ms proxy frame, 0.126 s 24 MP — all 29 parity tests pass on both** from one WGSL module; >2^24-pixel masked render pinned (the GLSL's f32 gid split was wrong there) |
| 3 | reviews (Opus darkroom, Sonnet quality) | reviewers | done | no formula divergences; 1 GLSL bug found (f32 gid split past 2^24 px); 6 robustness fixes landed (2aaaa56, 6c40f72); deferred: private fields on RgbImage/Plane, #[must_use], blur allocations, fixtures module behind a feature |
| 3 | S negsession + negcli | agent S | done | 74 tests; cache tower with ImageSession's keys; sidecars/profiles/history/export (craft-cms ICC via image/tiff encoders); negcli decode/thumb/render/meter/bench/export; CPU proxy frame 91 ms, Session::open 285 ms |
| 3 | G host fixes (review 6) | agent G | done | 42 tests; error scopes (no aborts), LRU cap 4, unmap guards, `required_limits`, one-slot large display tier (24 MP display 59 → 34 ms cached), Rgba8Unorm-only texture path, `pack_unorm8` |
| 4 | I integration (negkit darkroom modules + GpuRenderer into negsession, test strip, zone placement) | agent I | in progress | |
| 4 | U1 egui shell | agent U1 | in progress | |
