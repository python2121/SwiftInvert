# CLAUDE.md

Guidance for Claude Code when working in this repository.

> **Keep this file current.** When a change alters something documented here —
> pipeline stage order, the uniform contract, an analysis default, the sidecar
> format, or the build/test commands — update the relevant section in the same
> change. After adding an adjustment control, re-check the "Adding a new
> adjustment control" checklist and the uniform layout table.

SwiftInvert is a native macOS film-negative conversion app (SwiftUI + Metal),
a deliberate ~10% rewrite of the Python app **NegPy**
(`~/Documents/code/NegPy`), which remains the numerical reference
implementation. Scope: library grid, C-41 conversion with NegPy-quality auto
metering, exposure/color controls, histogram, export. Camera RAW only; no
B&W/E6, papers, crosstalk, retouch, or toning.

## Commands

```bash
swift build                 # debug build
swift run -c release SwiftInvert   # the app (release: debug decode is ~10x slower)
make test                   # Swift Testing suite — MUST use make, not bare `swift test`
swift build -c release && .build/release/negcli   # headless CLI (decode/thumb/render/bench/meter)
make app                    # package self-contained dist/SwiftInvert.app (bundles Homebrew dylibs)
make install                # copy the bundle to /Applications

# Regenerate parity fixtures (after deliberate NegPy kernel/constant changes):
cd ~/Documents/code/NegPy && uv run python ~/Documents/code/SwiftInvert/scripts/dump_fixtures.py
```

**Toolchain constraints (this machine has Command Line Tools, no Xcode):**
- No XCTest and Testing.framework lives outside default search paths → tests
  use Swift Testing with the framework/rpath flags encoded in the `Makefile`.
  Bare `swift test` fails with "no such module 'Testing'".
- No build-time `metal` compiler → shaders are compiled **at runtime** from
  `Sources/MetalRenderKit/Shaders/NegPipeline.metal` (a bundled `.copy`
  resource) via `MTLDevice.makeLibrary(source:)`.
- LibRaw comes from Homebrew (`brew install libraw`), linked dynamically via
  the `CLibRaw` systemLibrary target (`pkgConfig: "libraw_r"`). LGPL-2.1.
- If the repo directory is ever moved/renamed: `rm -rf .build` (the module
  cache embeds absolute paths).

## Module graph

```
SwiftInvert (app)  ──►  MetalRenderKit ──► NegativeKit
       │                     │
       └──►  RawDecodeKit ───┴──► NegativeKit
                  │
                  └──► CLibRaw (Homebrew libraw_r)
negcli ──► RawDecodeKit + NegativeKit + MetalRenderKit
```

- **NegativeKit** — pure Swift math, no UI/Metal/IO. The port of NegPy's
  conversion kernel. Only Foundation/simd/Accelerate.
- **MetalRenderKit** — the GPU render chain + `ColorIO` (CGImage/color-space
  conversion, used by export).
- **RawDecodeKit** — LibRaw wrapper + embedded-thumbnail extraction.
- **SwiftInvert** — SwiftUI app (`AppModel`, `ImageSession`, views).
- **negcli** — headless driver; `bench` measures slider-latency and analysis;
  `meter` prints the darkroom read-outs (negative character + spot-densitometer
  probes) for a real RAW, which is how the read-out math gets exercised on
  actual scans (the fixtures can't cover a hover).

## The image pipeline

End-to-end: **decode → orient → analyze (CPU) → derive params (CPU, µs) →
render (GPU) → display/export**. Slider changes re-run ONLY derive+render
(~5 ms at preview size). Analysis re-runs only when its inputs change.

### 1. RAW decode (`RawDecodeKit/RawDecoder.swift`)

LibRaw parameters mirror NegPy's rawpy decode exactly (verified byte-identical
against rawpy on real files): `gamma=(1,1)`, `no_auto_bright`, `output_bps=16`,
`output_color=RAW` (sensor-native, **no camera color matrix**), unity WB
(`user_mul=(1,1,1,1)`, `use_camera_wb=0`), `user_flip=0`. Output is scene-linear
`RGBImage` (interleaved RGB float32 in [0,1], `u16/65535` via vDSP).

- **Preview path**: `half_size=1` + linear demosaic (`user_qual=0`) — except
  X-Trans sensors (`idata.filters == 9`), where half_size aliases the 6×6 CFA;
  they decode full and downsample. Preview is capped at 1536 px long edge —
unless the HQ toggle (canvas control bar, `AppModel.hqPreview`,
session-only) is on: then the display render runs on the cached
full-resolution decode (`ImageSession.hqSourceTexture`; analysis stays on
the proxy, matching export), freed by the first non-HQ render.
- **Full path** (export): LibRaw default (best) demosaic, full resolution.
- EXIF orientation baked in Swift afterwards (`applyingFlip`, dcraw codes:
  3=180°, 5=90°CCW, 6=90°CW). C struct's flexible array member must be read
  via byte offset (`MemoryLayout.offset(of: \.data)`) — `withUnsafePointer(to:
  pointee.data)` copies to a misaligned temporary and traps.
- **User orientation** (`settings.rotation` 0/90/180/270 CW +
  `flipHorizontal`) is baked into pixels right after decode
  (`RGBImage.oriented`), so analysis, display-space rects, rendering and
  export all share one coordinate space with zero mapping code.

### 2. Analysis (`NegativeKit`, CPU, on the ≤1536px preview)

**Fine-rotation invariant:** analysis runs on the orientation-only image
(90° steps + flip — `ImageSession.meterPreview`), never the fine-rotated
one: the inscribed auto-crop changes with the straighten angle, and
re-metering it would drift the conversion as the user rotates (it also
made the 0° straighten base's look differ from the committed re-bake).
Rendering/export still bake the angle; `exportRender` shares `prepare()`,
so preview and export agree.

Two stages, both offset-independent since the 2125a34 port
(`ExposureKernel.prepare` ≈150 ms once per image/crop; `finalize` ≈25 ms,
cached with it — white/black-point drags re-run NO analysis at all; the
offsets fold into `finalBounds` at derive time only):

**`prepare(linearImage:cropRect:analysisRect:analysisBuffer:)` →
`Prepared`** (offset-independent, cached per rect state):
1. **Region priority** (NegPy `resolve_analysis_region`): a freehand
   `analysisRect` wins and disables the buffer inset; else the output
   `cropRect` scopes metering (borders outside the crop can't skew the
   inversion) with the buffer applied inside; else the centered buffer alone.
   Default buffer **0.10** = middle 80% of the frame (NegPy ships 0.05 —
   deliberate divergence). Degenerate rects (<2 px) are ignored.
2. **Prefilter** (`Prefilter.prefilterLogGrid`): `log10(clip(x, 1e-6, 1.0))`
   via vForce (buffer holds negative density −D since values ≤1), then a
   **block-median grid**: block side `b = ceil(maxDim/1024)`; per-cell,
   per-channel median kills dust/speculars and makes stats
   resolution-invariant. The preview path always hits `b=2`, which has a
   closed-form median-of-4 fast path.
3. **Bounds** (`BoundsAnalysis.analyze`, NegPy `analyze_log_exposure_bounds`):
   two independent percentile axes on the grid, one sort per channel
   (vDSP): the **luma axis** (base clip 0.01%) sets the mean center+span
   (dynamic range); the **colour axis** (base clip 1.0%) sets per-channel
   cast offsets relative to the median channel. Recombined:
   `floor[ch] = mean(luma floors) + (colour floor[ch] − median(colour floors))`
   (same for ceils) → `LogNegativeBounds` (floors < ceils).
4. **Meters** (all on the same grid):
   - `anchor`: P50 of grid luma normalized by the **base** bounds, pulled 20%
     from `assumedAnchor` 0.46, clamped ±0.12 — the auto-exposure key.
   - `texturalRange`: |P90 − P10| of raw log luma — drives Auto Contrast.
   - `shadowRefs`: P98 per channel — the cast-removal fallback tie.

**`finalize(prepared)` → `ExposureAnalysis`**: the **neutral axis**
(NegPy `measure_neutral_axis_from_log`) measured against the PRE-trim base
bounds — NegPy 2125a34: the film's cast is a source property, so creative
WP/BP trims must not perturb it (their GPU always measured pre-trim; the
CPU side we'd ported was the bug): three normalized-luma bands (highlight 0.10–0.30,
mid 0.40–0.60, shadow 0.72–0.92); per band, keep the lowest-chroma 30%
(≥64 px, median chroma ≤0.35 cap) and take per-channel **raw-log medians**;
`confidence = 1 − midChroma/cap` gates Auto cast removal. Returns nil mid or
shadow → no neutral axis (shadow-ref fallback used downstream).

`ImageSession` caches: decode (per image) → oriented preview (per
orientation) → `Prepared` + `ExposureAnalysis` (per rects) → GPU source
textures (per crop). The "Analyzing…" indicator shows only when `prepare`
will run.

`AppModel.sessionLRU` retains the **2** most recent sessions (active + the one
navigated from), so arrowing back to a frame doesn't rebuild the whole tower.
Retention alone is safe *because* every tier is keyed — a returning session
re-validates against the current settings and cannot serve stale pixels. Only
the on-screen frame may hold the HQ tier (`releaseHQ()` strips it from the
others); the proxy caches are tens of MB, the HQ decode is hundreds.

### 3. Parameter derivation (`ExposureKernel.deriveRenderParams`, µs per tick)

Turns `(ExposureSettings, ExposureAnalysis)` into `RenderParams` — the full
GPU uniform payload. Order matters:

1. `finalBounds = baseBounds + (wp, bp)` offsets (floors+wp, ceils+bp).
2. `dMin = paperDmin ? 0.06 : 0`; `anchor = autoExposure ? metered : nil`.
3. `strength = confidence × slider` — confidence scaling is always on
   (NegPy 0.36 removed the auto toggle; slider 0–2, >1 overcorrects past the
   neutral axis; kernel clamps bound any value).
4. **`perChannelCurveParams`** (`CurveLogic`, the C-41 gray-balance heart) —
   NegPy-exact. Green is the reference (its pivot rides the anchor). Modes:
   - *Neutral-axis* (default): R/B fitted to green's axis — quadratic through
     3 green-matched points when a highlight ref exists (divided-differences
     solve; curvature clamped to ±0.45·slope), else a 2-point line; midtone
     deviations clamped ±0.2.
   - *Shadow-ref fallback*: one-point slope tie at P98 (clamp ±0.1).
   - *Base*: single shared linear curve.
   Inputs: `baseSlope = gradeToSlope(grade, effectiveGradeRange(...))` —
   grade is ISO R 50–180 (115 ≈ grade 2), `k = 2.9·range/(R/100)` clamped
   [2,10]; Auto Contrast damps the floor-ceil/textural ratio toward 2.0.
   `lumRange` comes from the **pre-offset** base bounds (NegPy quirk).
   `computePivot` solves so the reference tone prints at density 0.74.
5. **Overall contrast** folds *exactly* into the core: `v→v+k(v−v*)` ⇒
   slopes,curvatures ×(1+k), pivot += k·v*/s′ (anchor invariant). k =
   slider×0.5, slider −1…+2.
6. **`cmyOffsets`** (pre-curve, normalized space): WB filtration
   (`slider×0.2 / channelRange`) with **Temp** folded along the Planckian
   direction (yellow + magenta×0.0029/0.0057) and **Tint** on magenta, plus
   **Exposure** stops (`stops × −log10(2)/channelRange` — the dodge/burn EV
   domain; + = brighter).
7. Grade-coupled toe/shoulder knees; band CMY (`colorShadows/Mids/Highs`
   ×0.2 into density units); tone controls passthrough (shadowContrast's
   negative side remapped so slider −3 lands on the monotone floor −0.8,
   also hard-clamped in the kernels); `preSaturation`, `vibrance`,
   `saturation` passthrough.

### 4. GPU render (`MetalRenderKit`, rgba32float, 8×8 threadgroups)

One command buffer, passes in order (`RenderPipeline.render` /
`renderDisplay`):

1. **`normalizeLog`** — `log10(max(c,1e-6))` (no upper clamp, matches NegPy
   GPU) → per-channel stretch `(log − floor)/(ceil − floor)` (offsets already
   folded into `finalBounds`; uniform wp/bp fields are legacy-zero).
2. **`printCurve`** — the asymmetric H&D print curve, per pixel:
   a. *Pre-saturation*: `c → mean + k(c − mean)` (density-deviation gain;
      default slider 1.15, kernel identity at 1.0).
   b. Per channel: `val += cmyOffsets` → quadratic core
      `v = slope(val − pivot) + curv·val²` → midtone paper-S
      `v += 0.15·0.6·tanh((v − v*)/0.6)` → **tone controls** (masks
      `wS = σ(3.5(v−1.40))`, `wH = σ(3.5(0.30−v))` on the incoming v,
      parallel form: shadows/highlights lifts + anchor-pivoted contrasts) →
      **3-band color** (same masks, `wM = max(1−wS−wH,0)`; NegPy's 2-band
      regional CMY generalized) → shoulder softplus toward `d_min_eff`
      (paper white) → toe softplus toward `d_max_eff` (paper black).
   c. `t = 10^−D`; **True Black** (BPC, optional): `t → (t−b)/(1−b)` with
      `b = 10^−dMax` referenced to the *physical* d_max so toe lifts survive
      (negative toe raises the clip point); clamp [0,1] → **linear
      reflectance** out.
3. **`colorPop`** (dispatched ONLY when a color-pop control is off-default:
   vibrance/saturation/redSaturation ≠ 1 or redHue ≠ 0) — CIELAB
   (ProPhoto primaries, D50; matrices duplicated in MSL and
   `LabColor.swift` — keep in sync): the **Color Mixer** first
   (`LabColor.applyColorMixer`, SwiftInvert-only, no NegPy equivalent —
   chroma-gated hue-targeted R/Y/G/B bands: per-band raised-cosine hue
   windows × a shared chroma ramp that zeroes at the neutral axis, so
   whites/grays/faint casts never move; all weights read the ORIGINAL hue
   and compose jointly, so overlapping feathers are order-independent;
   constants tuned on real scans (blues at Lab hue ~235, gate 6→16 —
   colorimetric-primary values starve real content, whose colorful pixels
   sit at chroma 12-30): `bandCentersDeg`/`bandHalfWidthsDeg`/`bandChromaGate*`/
   `bandMaxHueShiftDeg` mirrored as MSL literals; UI: segmented band
   picker + gradient tracks, `ColorMixerSection.swift`), then
   vibrance (muted-chroma boost, /60 range) then saturation (a*,b* scale).
   Separate pass on purpose: inlining
   the Lab code into printCurve cost ~3 ms/frame in register pressure even
   when branched off. Writes into the (already consumed) `normalized`
   texture, which becomes the content texture.
4. **`histogram256`** — 4×256 atomic bins (R,G,B + Rec.709 luma) from the
   linear content, OETF-encoded in-shader so bins match display values.
5. **`outputEncode`** — working-space OETF (ProPhoto ROMM TRC: gamma 1.8,
   linear toe ×16 below 1/512). Display fast path writes the same kernel
   into an **rgba8unorm** texture (GPU quantization, 4× smaller readback,
   zero CPU conversion); export/tests use the float path.

**Threading/reuse contract**: `render` is serialized by an internal lock and
returns **read-back buffers, never live textures** — intermediate textures
are cached per size (≤4 MP; export sizes are not retained) and a later render
overwrites them. Violating this segfaulted the concurrent test runner once.
Uniform structs are mirrored byte-for-byte in `ShaderTypes.swift`;
`LayoutTests` pins strides (Norm 48, Curve 256) and key offsets — update both
sides plus the asserts together.

### 5. Color management

Working space is **linear ProPhoto → ROMM-encoded** at the end of the chain.
- **Display**: the encoded output becomes a CGImage tagged
  `CGColorSpace.rommrgb`; ColorSync converts to the monitor — verified
  byte-identical to NegPy's littleCMS display transform (relative
  colorimetric + BPC) on reference colors (`ColorIOTests` pins the oracle
  values, generated with NegPy's bundled ICC profiles).
- **Export** (`Exporter` + `ColorIO`): default **sRGB** (NegPy's default;
  wide-gamut files look washed out in profile-ignorant viewers) via a 16-bit
  ROMM CGImage drawn into an sRGB CGContext (one quantization). ProPhoto
  remains selectable. JPEG 8-bit (default q=0.92) or TIFF 16-bit, optional
  long-edge resize, destination next-to-source or a chosen folder.

## Parity with NegPy

`Tests/Fixtures/` is dumped from NegPy's actual engine by
`scripts/dump_fixtures.py` (run in NegPy's uv env). The suite (`make test`)
verifies every stage boundary:
- closed-form oracles (percentile semantics, grade/pivot/softplus/OETF math),
- `synthetic64` (NegPy's golden image, dumped as .bin): prefiltered grid
  @1e-5, bounds/meters/curve-params @1e-4, full CPU chain @1e-4,
- `synthetic_grid` (1600×1066, regenerated bit-exactly in Swift from an
  integer-hash formula): exercises b=2 block-median + the quadratic
  cast-removal path (slope/pivot tolerance 2.5e-4: Accelerate-vs-libm ulp
  amplification),
- `ramp257` print-curve shapes, `lab_color` (vibrance/saturation vs NegPy's
  CIELAB ops),
- GPU vs fixtures and GPU vs CPU reference at NegPy's own gates (mean<0.01,
  max<0.04); the rgba8 display path within 1.5/255 of the float path.

Beyond parity, the suite covers the seams the fixtures reach only
transitively, plus the app layer:
- `SidecarCodecTests` + `HistoryLabelTests` are **drift-catchers**: each pins
  `ExposureSettings`' stored-property count (45) and exercises every field —
  adding a settings field fails both until the decoder, `HistoryLabels`, and
  the tests' mutation lists all get their line (see the control checklist).
- `ImagePipelineSeamTests`: the prepare/finalize cache split (a reused
  `Prepared` must equal fresh analysis; offsets may only move the neutral
  axis) and `RGBImage.downsampled` (dims/identity/mean preservation).
- **`SwiftInvertTests`** — the app-target suite. SwiftPM tests the `@main`
  executable directly (`@testable import SwiftInvert` — works since Swift
  5.5, verified under the Makefile's CLT flags): `SidecarStore` file behavior
  (legacy `.negswift.json` fallback + delete-on-save),
  `ExportOptions.destinationURL`, `DensitometerState` probe mapping,
  `ImageConversion` shapes. Views/AppModel/ImageSession stay UI-verified —
  and pointer paths need a HUMAN pointer: synthetic mouse events never reach
  the unbundled binary (no TCC grant, won't take focus), so verify hover
  logic headlessly (`negcli meter` pattern) and hand off the gesture.

**Upstream review log: see `UPSTREAM.md`** — it records the last NegPy
commit reviewed (the baseline for "what changed upstream?" requests) and
the port/skip decisions per review. Update it after every upstream review.
For "what changed in NegPy?" requests, run the **`/negpy-review` skill**
(`.claude/skills/negpy-review/`) — it fetches upstream, triages the diff
around the inversion pipeline, and maintains UPSTREAM.md.

Neutral-axis semantics are synced with NegPy `2125a34` (pre-trim bounds;
fixtures unaffected — all dump configs use zero offsets).
Kernel constants are synced with **NegPy 0.38** (`6b841a1`: Auto Grade retune
`auto_grade_target` 0.55 / `auto_grade_strength` 0.3, defaults `paper_dmin`
off + `true_black` on — plus the 0.36 set: `toe_height` 0.90 with the
`toe_grade_strength` rescale, True Black, always-confidence cast removal);
fixtures were re-dumped from that revision (the manifest records `paper_dmin`
and `true_black` per config; the parity harnesses read both, so default flips
on either side can't silently skew parity). NegPy's per-layer R/G/B trims,
Split Grade and Zone Density (their convergent take on our tone controls)
are NOT ported — our tone controls + 3-band grading cover the achromatic
cases; per-channel crossover trims are a candidate future feature.

**Deliberate divergences from NegPy** (fixture tests pin the NegPy-neutral
values where needed):
- `preSaturation` default **1.15** (NegPy has no equivalent; parity tests set 1.0),
- `redHue` default **+0.5** (Color Mixer, SwiftInvert-only: C-41 reds skew
  magenta out of the box, +0.5 = 15° toward orange; parity tests pin 0),
- default analysis buffer **0.10** vs NegPy 0.05 (tests pass 0.05 explicitly),
- **Film-base sampling** (`settings.filmBaseSample`, SwiftInvert-only): a
  sampled rebate patch (per-channel log medians, `ExposureKernel.
  measureFilmBase`) replaces the colour-axis CEILING offsets in
  `BoundsAnalysis.analyze` — ground truth for the orange mask instead of the
  gray-world percentile estimate. Ceiling-only on purpose (the mask lives in
  the unexposed couplers → strongest at the thin end; base offsets at the
  floors would overcorrect highlights); the ceiling level stays luma-anchored
  (median channel pinned), so only color moves. nil = bit-identical to NegPy's
  path (fixtures unaffected). The VALUE is stored, not the rect: channel
  medians are orientation-invariant and paste across a roll,
- NegPy's default lab sharpen (0.25 since 8bc9678; was 0.5 earlier in 0.38) is not implemented,
- SwiftInvert-only controls: exposure stops, tone controls
  (shadows/highlights ± contrasts), overall contrast, temp/tint, 3-band
  color grading, pre-saturation — all identity-at-default so the NegPy
  fixtures still pass.

## App layer (`Sources/SwiftInvert`)

- **`AppModel`** (@MainActor @Observable): selection + `multiSelection`,
  settings (didSet → coalesced latest-wins render task + debounced 1 s
  sidecar save; `isRestoringSettings` suppresses saves on open), tool modes
  (crop/analysis-region draw), baseline hold-to-compare
  (`showingBaseline` renders stock settings with current geometry), export
  batch task with progress/cancel.
- **`ImageSession`** (actor): the per-image cache tower (see §2) + render.
- **Darkroom read-outs** (`Densitometry` in NegativeKit — pure measurement, no
  render path, so no parity surface): the **spot densitometer** (hover the
  canvas → D + zone in the control bar, with an 11-cell `ZoneStrip`) and the
  **Negative character** row under the Grade slider ("0.90 · normal"), which is
  the measured pre-offset density range vs `CurveLogic.defaultGradeRange`.
  Both mirror NegPy (`densitometer.py`, `stats._negative_row`), including its
  quirk of taking luma on the ENCODED triplet before decoding. The probe reads
  the **displayed rgba8 bitmap** (ROMM-encoded, so the bytes already are the
  working-space values) rather than a GPU metric — `DensitometerState` caches
  the provider bytes once per render and is a separate `@Observable` so pointer
  moves invalidate only the read-out label, never the canvas. NegPy's 120-bin
  `density_histogram` is deliberately NOT ported: it exists to feed their H&D
  chart, which we don't ship.
- **Edit history**: per-image undo/redo in AppModel (`historyEntries`/`historyIndex`,
  session-scoped per URL). Slider/handle drags commit on RELEASE (drag =
  preview: `setControlEditing` via the `controlEditingChanged` environment
  hook holds commits while any control is held); non-drag changes coalesce
  via a 0.7 s debounced commit;
  labels come from `historyLabel(from:to:)` (HistoryLabels.swift — add a line
  there for every new settings field) or `pendingHistoryLabel` for named
  actions (Rotate/Crop/Reset). New edits truncate the redo tail. Undo flushes
  any in-flight uncommitted change first. UI: HistoryPanel (⌘Z/⇧⌘Z,
  click-to-jump), below the collapsible Adjustments section.
- **Unified Crop & Straighten** (Lightroom model): `ToolMode.crop` renders
  the full UNROTATED frame (scheduleRender substitutes fineRotation 0,
  uncropped) fitted by its rotated bounding box; the image rotates behind an
  axis-aligned `CropBoxOverlay` (dim surround, thirds, corner handles,
  clamped move) whose math lives in `NegativeKit/CropGeometry` (rotated-space
  boxes, fitScale/constrain, content-preserving `remapCrop`; tested). The
  desired box is DetailView `@State` (`cropBox`, nil = follow committed);
  exit commits via `commitCrop()` (near-full box ⇒ crop cleared). Straighten
  commits outside the mode go through `AppModel.commitFineRotation`, which
  remaps a committed crop so its content doesn't drift with the inscribed
  auto-crop; `RenderOutput.frameSize` (orientation-only dims) is the
  coordinate base. The analysis tool keeps the old draw-a-rect
  `SelectionOverlay`.
- **Menu bar** (`SwiftInvertApp` `.commands`): File = Open Folder ⌘O /
  Export ⌘E / Show in Finder ⇧⌘R; Edit = Undo/Redo (replacing the system
  group — the ⌘Z shortcuts live HERE, not on HistoryPanel's buttons),
  Copy/Paste Adjustments ⇧⌘C/⇧⌘V (geometry never pasted), Reset All ⌥⌘R;
  View = Show Library ⇧⌘L / Show Grid Lines ⇧⌘G / HQ Preview ⇧⌘P
  (@AppStorage keys shared with the in-window controls); Image =
  Previous/Next Image ←/→ / Rotate Left/Right ⌘[/⌘] / Flip ⇧⌘H /
  Crop ⌘K / Crop for Analysis ⇧⌘K + clear items (tool toggles checkmark
  while active; Escape exits). Frame navigation also answers ↑/↓ (↑ =
  previous, ↓ = next) via the window's keyDown monitor in
  `SwiftInvertApp` — a menu item takes exactly one key equivalent, so the
  vertical pair can't live on the menu; the monitor mirrors the menu
  items' enablement (no files / export sheet up ⇒ pass through).
- **Sidecars**: `<basename>.swiftinvert.json` next to the source
  (`SidecarStore`); pre-rename `.negswift.json` read as fallback and removed
  on next save. Missing keys decode to defaults (custom `init(from:)` in
  `ExposureSettings` — every new field needs a line there).
- **UserDefaults**: `libraryFolder`, `canvasColor`, `exportOptions`
  (JSON blob); one-time migration from the pre-rename "NegSwift" domain.
- **SwiftUI pitfalls already hit** (don't regress):
  - Lazy grids + @Observable: cells must read observable state inside their
    own body — precomputed Bools leave stale cells (selection halo bug).
  - `.position()` expands a view's frame to its container: attach
    `contentShape`/gestures BEFORE positioning (histogram handle bug).
  - `NSEvent.modifierFlags` polling in tap handlers is unreliable — use
    `TapGesture().modifiers(.command)` etc.
  - The app runs unbundled via `swift run`: activation policy is set
    manually; UserDefaults/TCC key off the process name.

## Adding a new adjustment control (checklist)

1. `ExposureSettings`: field + default, **plus a line in the custom
   decoder** (sidecar back-compat), **plus `historyLabel`**
   (HistoryLabels.swift) — and `RenderParams` + its init if the kernel
   needs it. Two tripwires enforce this: `SidecarCodecTests` and
   `HistoryLabelTests` both pin the stored-property count (45) and mutate
   every field, so `make test` fails until all the lists have their line.
2. `deriveRenderParams`: map settings → params (fold into existing params
   where the algebra allows — see overall contrast/exposure — before adding
   uniforms).
3. Kernels, BOTH sides in the same change: `ReferenceCurve.swift` (CPU) and
   `NegPipeline.metal` (MSL), in identical order/parallel form. New uniforms:
   extend `CurveUniforms` in both files (16-byte alignment) + update
   `LayoutTests` strides/offsets. Guard non-default-off work behind uniform
   branches or a separate pass (occupancy!).
4. UI: `LabeledSlider`/`GradientSlider` (sets the reset-⨯ default), section
   in `ControlsSidebar`; `negcli render` flag if useful.
5. Tests: default-is-identity (fixtures must still pass), direction/region
   properties, monotonicity if it touches the tone curve, sidecar
   round-trip, GPU-parity case with the control active.
6. Run `make test`, `negcli bench` (watch for regressions), launch the app
   headlessly (render log line), commit with the reasoning.

## Constants sync points

- `K` (`ExposureConstants.swift`) is the single Swift source; the MSL
  duplicates: tone anchors/sharpness (`TONE_SHARPNESS`, `SHADOW_ANCHOR`,
  `HIGHLIGHT_ANCHOR`), Lab matrices/eps/kappa, ROMM OETF breakpoints.
  GPU/CPU parity tests catch drift but update them together.
- If NegPy's `EXPOSURE_CONSTANTS` change deliberately: update `K`, re-dump
  fixtures, re-run `make test`.
