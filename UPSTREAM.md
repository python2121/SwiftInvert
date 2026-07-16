# Upstream (NegPy) review log

SwiftInvert ports NegPy's negative-inversion pipeline; NegPy
(`~/Documents/code/NegPy`) keeps evolving. This file records **the last NegPy
commit we reviewed**, so "what changed since we last looked?" always has a
baseline. Keep it current: every upstream review ends by updating the marker
and appending a history entry.

## Last reviewed

```
commit:   4a669ed  ("fix: override export_fmt/export_color_space from session
          in all_saved scope (#527) (#534)")
reviewed: 2026-07-16
fixtures: Tests/Fixtures/ dumped from 6b841a1 (2026-07-15: Auto Grade constants
          + paper_dmin/true_black default flips ported; the dump manifest now
          records true_black per config and both parity harnesses read it).
```

## How to run a review

**Preferred: run the `/negpy-review` skill** (`.claude/skills/negpy-review/`)
— it automates the steps below, applies the port/skip judging rules, and
updates this file. The manual procedure, for reference:

1. `cd ~/Documents/code/NegPy && git fetch origin`
2. `git log --oneline <last-reviewed>..origin/main` — the unreviewed range.
3. Focus the diff on the pipeline we track:
   `git diff <last-reviewed> origin/main -- negpy/features/exposure/ negpy/features/process/ negpy/kernel/image/`
   (normalization.py, logic.py, models.py/EXPOSURE_CONSTANTS, the WGSL shaders).
   Also check whether the characterization goldens moved
   (`tests/test_scene_linear_relocation.py`, `tests/test_characteristic_curve.py`)
   — golden changes mean a deliberate default-look change upstream.
4. Decide per change: port / deliberately skip (record why) / not applicable
   (UI, camera capture, CPU-parallelism, stages we don't ship).
5. If constants or kernel semantics were ported: pull NegPy, re-dump fixtures
   (`uv run python ~/Documents/code/SwiftInvert/scripts/dump_fixtures.py` from
   the NegPy repo — check the dump script against upstream signature changes
   first), update `K` / kernels, `make test`.
6. Update the **Last reviewed** marker and append to the history below.

## Review history

### 2026-07-16 — through `4a669ed` (0.38.0 → unreleased, 9 commits)

**Kernel status: untouched.** Zero commits in the range touch
`features/exposure/`, `features/process/`, `kernel/image/`, or the
characterization goldens — the path-filtered log is empty, `git diff --stat`
over those trees returns nothing, and there are no renames in the range
(`--diff-filter=R` empty). A genuine null for the inversion pipeline: no
fixture re-dump, no constants drift, `dump_fixtures.py` signatures
unaffected. The one shared-kernel-adjacent hunk was checked by hand:
`lab.wgsl` changed only a stale comment ("Adobe RGB" → ProPhoto/D50 — the
code fix was `07e3f8f`, reviewed 2026-07-15), and the `lab/logic.py` rewrite
is entirely the CLAHE function; vibrance/saturation (mirrored by our
`colorPop`) are untouched.

**Ported:** nothing (nothing required).

**Not applicable (this range):**
- `232f26d` unify CPU/GPU CLAHE into one Lab-L algorithm (+ new
  `test_gpu_curve_parity` CLAHE case, PIPELINE.md now documents CLAHE as its
  own stage before Retouching) — local-contrast lab stage we don't ship.
  If we ever add local contrast, port THIS version (fixed 8×8 tile grid,
  256 bins, integer-count clip + even redistribution, smoothstep-bilinear
  CDF blend on CIELAB L*, CPU/GPU pinned to ~1e-6).
- `c714a24` Feat/finish — Finish panel: edge burn (true exposure burn in
  stops, radial↔rectangular roundness), filed-carrier black rebate, print
  mats — print-finishing stage, out of scope. The **edge burn** design
  (stops-domain `I·2^(−s·m)`, card-burn roundness) is the note-worthy idea
  if a vignette tool is ever wanted.
- `78b74ef` roll-aware Auto Crop All + consensus detection (~500 lines in
  `geometry/batch_autocrop.py` + big `geometry/logic.py` growth) — camera-scan
  auto-crop detection; we have no auto-crop detection feature.
- `16bcee8` DNG/JXL export via imagecodecs 16-bit CMS — their export CMS
  stack; we export JPEG/TIFF through ImageIO/ColorSync.
- `4a669ed` export_fmt/color_space override in all_saved scope — their
  export-preset session plumbing.
- `2682a0a` camera-scanning calibration fix; `69a2934` status toasts / crop
  busy overlay; `ca18ecb` overflow-menu fix; `86e73ba` changelog — UI/capture.

### 2026-07-15 (second review) — through `6b841a1` (0.38.0 tail, 9 commits)

**Kernel status: DEFAULT LOOK MOVED UPSTREAM.** Three commits (`0f063cc`,
`67b5a8c`, `6b841a1`) changed `exposure/models.py` and regenerated both
characterization goldens — the highest-priority signal this log tracks.
Net change at the tip (0f063cc bounced `auto_grade_target` to 0.6, 67b5a8c
back to 0.5, 6b841a1 settled it):

- `paper_dmin` default **True → False** (paper white d_min 0.06 → 0; highlights
  print brighter — goldens moved 0.86 → 0.94 in the lights).
- `true_black` default **False → True** — **upstream converged on OUR recorded
  divergence.** No code change here; the CLAUDE.md divergence entry ("trueBlack
  default on; NegPy ships it off") is now stale and should be dropped when the
  port lands.
- `EXPOSURE_CONSTANTS["auto_grade_target"]` **0.5 → 0.55**,
  `["auto_grade_strength"]` **0.4 → 0.3** — Auto Grade targets slightly higher
  contrast and adapts less to scene range. Maps to `K.autoGradeTarget` /
  `K.autoGradeStrength` (`ExposureConstants.swift:67-68`), CPU-only (no MSL
  duplicate). Also shifts `CurveLogic.defaultGradeRange` 1.0 → 1.1, the
  denominator of the Negative-character read-out — upstream's
  `stats._negative_row` uses the same `default_grade_range()`, so porting the
  constant keeps that diagnostic in lockstep automatically.

**Ported (2026-07-15, same day as the review):**
1. `K.autoGradeTarget` 0.5 → 0.55, `K.autoGradeStrength` 0.3
   (`ExposureConstants.swift` — CPU-only, no MSL duplicate). Shifts
   `CurveLogic.defaultGradeRange` 1.0 → 1.1, which the Negative-character
   read-out tracks automatically (verified via `negcli meter` on a real CR3:
   "density range 0.958  default grade range 1.100 → normal").
2. `ExposureSettings.paperDmin` default true → false, including the sidecar
   decoder fallback (user's call: no existing edits worth preserving, so no
   split-default dance). `negcli meter` confirms the paper floor is gone —
   brightest tone reads D 0.00 (was 0.06 = K.dMin).
3. trueBlack: no code change (already our default); the CLAUDE.md divergence
   bullet is retired — it's upstream's default now too.
4. Fixtures re-dumped from `6b841a1`. `dump_fixtures.py` now records
   `true_black` in the per-config manifest (it already recorded `paper_dmin`),
   and both parity harnesses (`AnalysisParityTests.settingsFrom`,
   `GPUParityTests.settings`) read it instead of hard-coding the old NegPy
   default — future default flips on either side can't silently skew parity.
   Drift-catcher mutation lists (`SidecarCodecTests`, `HistoryLabelTests`)
   updated for the new paperDmin default. All 132 tests pass; `negcli bench`
   unchanged (4.4 ms/frame, prepare 157 ms, finalize 25 ms).

**Deliberately skipped:**
- Lab sharpen default 0.25 → 0.5 + the `lab.wgsl` sharpen rework (`0f063cc`:
  real CIELAB L*-space unsharp mask with reflect-101 borders, sigma tracking
  scale_factor, smoothstep noise gate — replacing the old gamma-luma RGB-ratio
  scale). We don't ship sharpen (recorded divergence). **Divergence baseline
  moved:** if sharpen is ever implemented, port THIS Lab-space version, not
  anything older.

**Not applicable (this range):**
- `f309982` cast-removal strength sticks across frames, `6e55a4a` True Black
  sticks across frames — their Qt session's frame-to-frame settings carry-over;
  we have per-image sidecars + Copy/Paste Adjustments, no carry-over model.
- `07e3f8f` colour-manage from the working space, not Adobe RGB — their
  `AppState.workspace_color_space` was left at "Adobe RGB" when the working
  space moved to ProPhoto, skewing preview/export/thumbnails together.
  SwiftInvert has no such field: display tags `rommrgb` directly, export draws
  ROMM into an sRGB context; `ColorIOTests` oracles were generated directly
  from NegPy's bundled ProPhoto ICC (not through their AppState path), so they
  are unaffected. Their deleted `detect_color_space_from_raw` was never ported.
- `936ff88` untagged scanner-TIFF loading (sRGB → linear scanner data),
  `34e9efe` TIFF export via imagecodecs 16-bit CMS — scanner-TIFF input and
  their export CMS stack; we're camera-RAW only and export through
  ImageIO/ColorSync.
- `3a0175c` Feat/heal — retouch (out of scope); its `rawpy_loader.py` hunk is
  the VueScan/Adobe SubIFD **LinearRaw scanner-DNG** path, also out of scope
  (camera DNGs go through LibRaw unaffected).

### 2026-07-15 — through `ecec2bb` (0.37.2 → 0.38.0, 7 commits)

**Kernel status: untouched.** Zero commits in the range touch
`features/exposure/`, `features/process/`, `kernel/image/`, or the
characterization goldens — the path-filtered log is empty and
`git diff --stat` over those trees returns nothing. No renames in the range
(`--diff-filter=R` empty; all tracked pipeline files still present at the
tip), so this is a genuine null, not a path-filter miss. No fixture re-dump,
no constants drift, `dump_fixtures.py` signatures unaffected. 0.38.0 is
entirely triage/UI/workflow work.

**Ported:** nothing (nothing required).

**Closed since the last review** (were in "To port", now shipped here
independently of upstream):
- Fine rotation + Straighten (`7f4b7a7`) — shipped as our unified Crop &
  Straighten mode (`CropGeometry`, `commitFineRotation`), a Lightroom-model
  design rather than upstream's reference-line tool.
- "Enter confirms crop" (`3961b4d`) — shipped (`fa5c890`), plus Escape-cancel.

**Correction to this entry (same day):** the TIFF-compression item below was
first logged as "confirmed still absent". **That was wrong.** TIFF compression
has been implemented since `a499904` — `Exporter.swift` sets
`kCGImagePropertyTIFFCompression = 5` (LZW), and its comment already records
the deliberate LZW-over-Deflate decision (ImageIO's Deflate support is
undocumented). The false "confirmed" came from a verification grep truncated by
`head -12`, forty lines before the relevant code. **Item closed, not open.**
Lesson for future reviews: never `head`-truncate an absence check — absence is
exactly what truncation fabricates.

**Closed (was carried over as "To port"; already shipped here):**
- TIFF export compression (`fb4b7a7`) — done at `a499904` (LZW). Upstream's
  Deflate+predictor is a deliberate skip, not a gap: ImageIO won't reliably
  write Deflate TIFF. Do not re-raise without new evidence.

**Ported (2026-07-15, same day as the review):**
- `77c8113` **spot densitometer + zone strip** → `NegativeKit/Densitometry.swift`
  (`zone(ofEncoded:)`, `printDensity(ofEncoded:)`, `read(encodedRGB:)`,
  `zoneRoman`), `SwiftInvert/Densitometer.swift` +
  `DensitometerReadout.swift`, wired to a canvas hover in `DetailView` with the
  read-out in the control bar. NegPy-exact semantics, including luma-on-encoded
  before the OETF decode.
  **Divergence from upstream, deliberate:** the probe reads the displayed rgba8
  bitmap (already ROMM-encoded) instead of a GPU metric — upstream needs
  `density_hist.wgsl` to feed their H&D chart; we have no chart, so a 120-bin
  GPU pass would have no consumer. `density_histogram` therefore **not ported**;
  revisit only if we ever build the H&D chart.
  **Also skipped:** the ΔD-above-base per-channel figure — it needs the source
  linear pixel + bounds, i.e. an actor round-trip into `ImageSession` per
  pointer move, for the negative-diagnostic half of the read-out. D + zone is
  the darkroom core; ΔD is a candidate if the round-trip proves cheap.
- **Negative character** diagnostic → `Densitometry.character(densityRange:)`
  (NegPy's 0.80/1.25 gates and wording), shown under the Grade slider — the
  slider it's about. Reads `analysis.baseBounds.luminanceDensityRange`, verified
  to be the same `norm_density_range` upstream feeds both its curve and this
  diagnostic.
- Both are pure measurement (no render path), so the parity fixtures are
  untouched and no re-dump was needed. 92 tests pass, including a new
  `DensitometryTests` suite of closed-form oracles.
- Verified on real scans via a new `negcli meter` command: on three CR3s the
  brightest tone reads D 0.06 on every frame — exactly `K.dMin`, the paper white
  the pipeline targets — and the character gates fire correctly (0.902 → normal,
  0.710 / 0.494 → flat).
- ~~**Unverified:** the hover interaction itself~~ — **verified by hand
  2026-07-15**: hovering the canvas meters correctly. (The original gap:
  synthetic mouse events can't reach the unbundled binary — no accessibility
  permission, app won't take focus from a shell — and the known-working
  histogram hover ignored them too, proving the harness was what failed.
  Lesson kept: GUI hover paths in this app need a human pointer to verify.)

**New candidate (perf/architecture, not pipeline) — half ported, half declined:**
- `938fe9e` "halve preview-load memory; instant frame switching".
  **Ported: multi-frame retention** — `AppModel.sessionLRU` keeps the 2 most
  recent `ImageSession`s (upstream's `preview_cache_max_full_res_entries`
  budget: active frame + the one navigated from), with `releaseHQ()` stripping
  the full-res tier from the frame that isn't on screen. Retention alone
  suffices here because every cache tier is already keyed.
  **Declined: upstream's `RenderMemo`** — it memoizes the last displayed render
  because their re-render is expensive. With the tower warm ours is
  derive+GPU (~5 ms), so a memo of the CGImage would add a staleness surface
  (their key has to track HQ flag, working space, GPU engine, soft-proof ICCs,
  monitor profile) to save 5 ms. Not worth it.
  **Declined: copy elision** — their win was deleting numpy defensive `.copy()`
  calls. Swift arrays are COW, so we never had that class of waste; the one
  place it would matter is already explicit (`preview = meterPreview` at 0°,
  commented "COW: at 0° the render input IS the meter image, no copy"). No
  change made.
  Original analysis of both ideas, retained for context:
  1. **Multi-frame retention.** Upstream gives full-res cache entries a slot
     budget (default 2: active frame + the one navigated from) so navigate-back
     is instant, and memoizes each frame's last displayed render in a new
     `RenderMemo` keyed by config + every display-path input, painting from it
     immediately while the authoritative render refreshes underneath. We hold
     exactly **one** `ImageSession` (`AppModel.swift:506` reassigns a single
     optional), so navigating back drops the entire cache tower — decode,
     oriented preview, `Prepared` (~150 ms), textures — and rebuilds from
     scratch. A 2-slot session LRU would be the bigger win for us than the memo
     itself, since we discard more than they do. Wants a memory-budget check
     first (our HQ path already caches a full-res decode per session).
  2. **Copy elision under a read-only contract.** They removed three redundant
     full-size buffer copies (peak RSS 2880→1609 MB on a 56 MP load) by letting
     cache and caller alias one buffer. Less directly applicable — our
     `RGBImage` copies are value-semantic and our contract already forbids
     handing out live textures — but worth a look at the HQ decode path if HQ
     memory ever becomes a complaint.

**Not applicable (this range):**
- `976851c` UI refinements; `ecec2bb` panel pin/reset docking; `979b592`
  adaptive canvas-toolbar overflow — their Qt dock/panel layer.
- `13b9434` heal/scratch edits never persisted — retouch, not shipped here.
- `2079d7c` docs (their CLAUDE.md slim-down); `0ca292d` CI self-assign workflow.
- 0.38.0 changelog headliners we don't ship: Keep/Reject contact-sheet triage,
  unreadable-file badges, roll-wide undo, canvas-tool Esc grammar, colour/
  Filtration renaming. One idea worth noting even though the feature is out of
  scope: their **two-stage Esc** (first clears in-progress points, second puts
  the tool down) — our Escape already cancels Crop & Straighten wholesale,
  which is the right behaviour for a mode with no in-progress point list.

### 2026-07-14 — through `0500404` (0.37.1–0.37.2, 7 commits)

**Kernel status: untouched.** No commits in the range touch
`features/exposure/`, `features/process/`, `kernel/image/`, or the
characterization goldens — no fixture re-dump, no constants drift,
`dump_fixtures.py` signatures unaffected.

**Ported:** nothing (nothing required).

**Not applicable (this range):**
- `2ecaebb` GPU readback-vs-destroy race fix (per-texture lock in their wgpu
  `GPUTexture`; densitometer hover on the UI thread raced engine cleanup on
  file switch). SwiftInvert's architecture already precludes this class of
  bug: `RenderPipeline.render` is serialized by an internal lock and returns
  read-back buffers, never live textures. Note for the future: if we port
  the spot densitometer (flagged 2026-07-13), keep hover readouts on those
  read-back buffers, not on GPU probes.
- `c728874` more ASCII-encode EXIF crash fixes — their metadata writer.
- `2a62c6a` camera-scanning follow-ups (RGB-only Scanlights, live-view
  polish, crash logging) — feature we don't ship.
- `3961b4d` UX polish (drag-to-heal, Enter confirms crop, cursor/tooltip
  tweaks) — heal is out of scope; "Enter confirms crop" is a small UX idea
  we could copy independently of upstream.
- `a033b92`, `80041ca`, `0500404` — changelog/lint churn.

**To port (proposed, not yet implemented — carried over from 2026-07-13,
re-confirmed still open):**
- TIFF export compression (`fb4b7a7`, upstream default Deflate+predictor):
  our `Exporter.swift` still writes uncompressed TIFF. Best-value small
  item; verify ImageIO Deflate support, else LZW.
- `77c8113` spot densitometer + `density_histogram` metric + zone strip —
  useful darkroom tool we lack; moderate effort (new GPU metric + UI), no
  parity impact.
- "Negative character" diagnostic (flat/contrasty vs `default_grade_range()`)
  — cheap, blocked on us having a stats read-out surface.
- Fine rotation + Straighten tool (`7f4b7a7`) — geometry feature, not
  pipeline; still open as a feature candidate.

### 2026-07-13 — through `f279337` (0.37.0 release, 25 commits)

**Kernel status:** untouched. No changes to normalization/curve logic,
`EXPOSURE_CONSTANTS`, or the characterization goldens — no fixture re-dump
needed. Only two commits touched `features/exposure/` at all:

**Ported:** nothing (nothing required).

**Candidates flagged (not yet decided):**
- `77c8113` Analysis panel: merged H&D chart, **spot densitometer**, zone
  strip. New `density_histogram` metric (120 bins over −0.1…1.1 normalized-log
  density, `analysis.py`/`density_hist.wgsl`) computed per render; the
  densitometer maps hover → normalized-log coords → zone/density read-out.
  Presentation-layer, but a genuinely useful darkroom tool we lack.
- `stats.py` rework (same PR): "Negative character" diagnostic — measured
  density range vs `default_grade_range()`, ratio <0.80 → "flat (≈N−1)",
  >1.25 → "contrasty (≈N+1)". Cheap to add if we grow a stats read-out.
- `fb4b7a7` default TIFF compression LZW → Adobe Deflate (ZIP) + horizontal
  predictor. Our export currently writes **uncompressed** TIFF (no
  compression options set in `Exporter.swift`); worth adding compression,
  though ImageIO's Deflate-TIFF support needs verifying (LZW is the safe bet).
- `7f4b7a7` clockwise-positive fine rotation + Straighten reference-line
  tool — we only have 90° steps; geometry feature, not pipeline.

**Not applicable / out of scope:**
- `db72e9f` CPU/GPU parity for **B&W** renders (post-curve luma collapse in
  `exposure.wgsl` is `mode == 1` only; we don't ship B&W). Its histogram
  sub-fix (bin from float output before quantization, drop the 4× stride)
  is a CPU-preview artifact fix — our `histogram256` already bins the linear
  float content on GPU.
- `e56c199` preserve edits when EXIF rewrites change the file hash — NegPy
  keys sidecars by content hash; ours are filename-keyed.
- `1deeba6` batch export path/mode bug — their session-export plumbing.
- Toners (`4a3cd34`), crosstalk profile editor (`0bba263`), retouch/heal &
  dust (`9b04f49`, `05b8763`), dodge-burn masks (`2cf6afd`), camera scanning
  (`c558a16`, `15d15c7`), shortcut-editor UI (4 commits), EXIF string
  sanitizing (`8363495`), changelog/screenshot churn — all stages/features
  we deliberately don't ship.

### 2026-07-11 — through `cac6396` (v0.35 → v0.36 era, ~20 commits)

**Ported:**
- True Black (BPC): `t → (t−b)/(1−b)`, b referenced to physical d_max;
  negative toe raises the clip point. (SwiftInvert later defaulted it ON —
  our divergence; NegPy ships it off.)
- `toe_height` 0.35 → 0.90 with `toe_grade_strength` × 0.35/0.90 rescale
  (perceptual toe/shoulder balance; default output bit-compatible).
- Cast removal: confidence scaling always on (auto toggle removed).
- flare/surround deleted (were always-off in SwiftInvert).

**Deliberately skipped:**
- Per-layer R/G/B trims (grade/toe/shoulder/width/Snap) — per-channel
  crossover correction; candidate future feature.
- Split Grade + Zone Density — upstream's convergent equivalent of our tone
  controls (theirs: zone centers 1.49/0.34, sharpness 4.0, sequential
  ordering; ours keep 1.40/0.30 @ 3.5 with documented monotone bounds).
- Midtone-gamma user slider (cheap to add if wanted).
- C41 denoise (lab stage — not ported), multi-core CPU kernels (we're GPU),
  camera-scanning feature work.

**Known-unreviewed at time of writing:** local NegPy has already pulled past
the marker (14 commits, `cac6396..c558a16`, mostly camera-scanning work by
the titles) — NOT yet reviewed for pipeline changes.
