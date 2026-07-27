# Upstream (NegPy) review log

SwiftInvert ports NegPy's negative-inversion pipeline; NegPy
(`~/Documents/code/NegPy`) keeps evolving. This file records **the last NegPy
commit we reviewed**, so "what changed since we last looked?" always has a
baseline. Keep it current: every upstream review ends by updating the marker
and appending a history entry.

## Last reviewed

```
commit:   31aea5c  ("chore: release 0.44.0 (#648)")
reviewed: 2026-07-27 (second review this date)
fixtures: Tests/Fixtures/ dumped from 0369b10 (2026-07-25, with the 127bcd7
          cast-removal estimators) — still valid; the only golden move since
          (6fd61c5) is Dye Mute's default, a feature we don't ship and the
          dump configs don't touch.
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

### 2026-07-27 (second) — through `31aea5c` (0.44.0 release, 5 commits)

**Kernel status: untouched — no golden moves, no constants, no parity
impact.** The one `features/exposure/` hit is additive measurement math for
a UI feature (below); `dump_fixtures.py` unaffected (its `density_histogram`
import is signature-stable).

**To port (proposed, not yet implemented — presentation layer, not
pipeline):** `ae56f8b` **Zone system overlay** (0.44.0's headliner, Shift+Z):
integer Adams zone per cell of a fixed grid (24 cells along the long edge)
computed from the display-encoded frame via area-average downsample (the
averaging damps grain — no extra smoothing); contiguous same-zone cells
merge into regions (shared edges simply not drawn), one bold Roman numeral
per region with the label anchor snapped to a cell the region actually owns
(a concave region's centroid can land outside it), paper black (0) and
white (X) flagged red, dark underlay beneath the grid lines for readability
over blown highlights; built once per render so toggling costs a repaint.
Crucially it reads the SAME zone ruler as the sidebar zone strip and spot
densitometer ("the three never disagree") — which we already ported
(`Densitometry.zone(ofEncoded:)` over the displayed rgba8 bitmap, cached in
`DensitometerState`). A Swift port is pure app-layer: grid + tiny
connected-components over a ~24×16 int grid (no OpenCV needed), a SwiftUI
canvas overlay, and a toggle; no parity surface, no fixture re-dump.
Moderate effort (~half day incl. the pinned-to-picture pan/zoom mapping).

**Not applicable:**
- `f7f3972` chroma denoise edge-bloom fix (taps weighted by chroma
  similarity as well as distance; slider renamed Chroma Denoise) — lab
  denoise stage we don't ship. Divergence note updated: if denoise is ever
  implemented, port THIS edge-aware version.
- `b9b98c3` DNG export dropped + config-migration centralization (their
  export formats/config plumbing; we never offered DNG),
- `a8027af` thumbnail-cache clear in Manage Database (their DB tooling),
- `31aea5c` release chore. (0.44.0 changelog's remaining entries — Dye Mute
  0.25, glow/halation luma, paste re-meter, filed-carrier flare — were all
  reviewed in the two prior entries.)

**Still open (carried over):** `91a1b78` tunable Auto Density/Grade targets
(user-initiated only); the on-scan Color Mixer band re-tune pass (ours).

### 2026-07-27 — through `9722a9c` (0.43.0 → 0.43.1+, 6 commits)

**One golden move, and it's inside a recorded skip: `6fd61c5` lowers the
Dye Mute default 0.5 → 0.25** (`lab/models.py` chroma_damping; relocation
goldens regenerated — "less chroma damping on the default look"). This is
the divergence baseline moving TOWARD us: upstream themselves flagged the
0.5 default as tuned on the old ProPhoto gamut (2026-07-20 review) and have
now halved it, walking their canonical look halfway back to our no-damping
position. **Skip stands, reinforced**; reopening condition unchanged
(visible oversaturation at hard grades on real rolls) — and if Dye Mute is
ever ported, the reference default is now 0.25. Our fixtures are unaffected
(damping is derivation-side and no dump config exercises it);
`dump_fixtures.py` signatures unaffected.

**Not applicable (each checked):**
- `3ca00f2` glow/halation luma → Adobe RGB weights (0.2974/0.6273/0.0753):
  fixes ONLY the glow/halation taps in `features/lab/` (stage we don't
  ship). Verified the core analysis luma upstream is still Rec.709
  (`domain/types.py` LUMA_R 0.2126 at the tip) — so our `K.lumaR/G/B`
  mirror remains correct for the uses we ported (bounds/meters/histogram/
  densitometry). Worth remembering if we ever add a halation-class effect:
  luma taken on WORKING-encoded triplets must use working-space weights.
- `5a00dd9` re-meter when pasted settings change the analysis region —
  their paste can transfer the region; ours structurally can't (analysisRect
  is in the `keepingGeometry` list, never pasted), so the bug has no
  counterpart here.
- `0a9d22a` slider commit baseline on external sync (their Qt settings-db
  plumbing), `f783fd0` 0.43.1 release chore, `9722a9c` filed-carrier bevel
  flare + paper margin + 2-D edge roughness (print finishing, out of scope).

**Still open (carried over):** `91a1b78` tunable Auto Density/Grade targets
(user-initiated only — see 2026-07-26 note); the on-scan Color Mixer band
re-tune pass (ours, post-b3490eb).

### 2026-07-26 — through `0e5b3f6` (0.43.0 tail, 4 commits)

**Kernel status: untouched — a genuine null.** The path-filtered log over
`features/exposure/`, `features/process/`, `kernel/image/` and the
characterization goldens is empty; no renames; no VERSION/CHANGELOG
movement. The one render-adjacent hunk was read in full:
`desktop/workers/render.py` (+5) adds a separate Qt signal so a batch
thumbnail overwrite can't clobber an already-rendered canvas frame — their
worker plumbing, zero kernel lines. No fixture re-dump, no constants
drift, `dump_fixtures.py` unaffected.

**Ported:** nothing (nothing required).

**UI idea worth noting (not pipeline):** `e5f99ff` slider UX — their Qt
CompactSlider now grabs on click anywhere in the groove (absolute jump,
Shift = relative fine-drag) and forwards near-miss clicks in an ~8 px band
above/below the groove into a real drag. Our SwiftUI sliders already
click-to-jump; the near-miss band + Shift-for-fine-drag are candidates if
slider precision ever comes up.

**Not applicable:** `9319408` camera live-view ability checks (capture),
`00d01c3` RGB-triplet thumbnail rendering (their thumbnail service;
camera-RAW only here), `0e5b3f6` macOS release CI (their packaging).

**Still open (carried over):** `91a1b78` user-tunable Auto Density / Auto
Grade targets — the "needs a Settings surface" blocker is weakening now
that the app has the profile picker/editor windows (2026-07-25), but those
manage ADJUSTMENT profiles; tunable targets are analysis CONSTANTS
(anchor/grade calibration), a different layer. Note the adjacent history:
an anchor-meter recalibration A/B for these same constants was built and
dropped 2026-07-25 (user: don't pursue) — any tunable-targets port should
be user-initiated. Also open: the on-scan Color Mixer band re-tune pass
(ours, post-b3490eb).

### 2026-07-25 — through `0369b10` (0.42.0 → 0.43.0, 14 commits)

**Kernel status: ANALYSIS SEMANTICS MOVED UPSTREAM — goldens regenerated.**
One commit, `127bcd7` ("Improve cast-removal estimators and fix chart cast
parity", 0.43.0's headliner), rewrites the two analysis estimators we ported,
with `test_scene_linear_relocation.py` goldens moved (small deltas ~1e-3 on
the synthetic scene — it has no strong cast; the point of the change is real
scans WITH casts). No WGSL/shader changes anywhere in the range — this is
CPU-analysis only, so a port touches NegativeKit + fixtures, not the Metal
side.

**PORTED 2026-07-25 (same day, user approved and chose it as the default)
— `127bcd7`, all pieces together as one semantic unit.** Landed in
`Meters.neutralAxis` (two-pass rewrite), `BoundsAnalysis.samePixelColorFloorRefs`
+ `analyze` wiring, `K` (six constants), and `Stats` [Double] overloads
(np-matching percentile/median — the same-pixel path is float64 end-to-end
like upstream, bit-comparable arithmetic). The vestigial
`analyze(channelsSorted:)` public overload was folded away (offset
re-derives stopped re-running analysis at 2125a34; nothing external used
it). Fixtures re-dumped from `0369b10`; all 132 tests pass, including
AnalysisParityTests/GridParityTests against the new-estimator fixtures.
Branch coverage verified positively: the same-pixel branch FIRES on
synthetic_grid (refs ≈0.1D off the percentile floors, Swift matches) and
correctly falls back on synthetic64; the two-pass axis moves synthetic64's
confidence 0.984 → 0.337 (the n/(n+256) size term bites on a 32×32 grid —
real previews sit near 393k grid px, where it doesn't). Real-scan A/B
(three CR3s, `negcli meter` old vs new binary): neutral-rich frames move
≤0.004D; IMG_0365's bright probe went 0.768/0.785/0.791 → 0.789/0.784/0.777
— a slightly cyan highlight now reads neutral, the exact failure mode the
same-pixel floors target. An independent line-by-line Python↔Swift audit
cleared all eight semantic areas and surfaced two last-ulp fidelity gaps,
both fixed: `Stats.percentileOfSorted` now mirrors numpy's two-sided
`_lerp` (interpolates from the upper bound when frac ≥ 0.5 — both
overloads), and the neutral axis's pass-1 luma/chroma read the ROUNDED
float32 norm values (upstream computes them from the float32 normalized
image). The audit's remaining note (two guards invert only under NaN) is
unreachable: the grid is log10(clip(...)), finite by construction.
Bench: slider path unchanged (4.6 ms/frame); prepare 157→182 ms,
finalize 25→47 ms (per-image analysis, matches the added work). CLAUDE.md
§2 + the constants-sync paragraph updated.

The pieces, for the record:

1. **Hue-uniform chroma metric**: near-neutral ranking switches from
   `max−min` to pairwise RMS `sqrt(((r−g)²+(g−b)²+(r−b)²)/3)` — max−min
   scores an opposed R/B split double a same-side deviation. Used by both
   estimators below. Maps to our `finalize` neutral-axis band selection.
2. **Two-pass neutral axis** (`measure_neutral_axis_from_log`): pass 1
   selects under a LOOSE cap (`neutral_axis_first_pass_cap` 0.55 — admits
   strong-but-correctable casts the old single 0.35 cap rejected outright;
   saturated content still fails); the affine R/B→G correction implied by
   pass-1 mid+shadow refs (normalized space) re-ranks chroma; pass 2 selects
   true neutrals under the strict cap, now **0.29** (was 0.35).
3. **Confidence rebuilt**: `tight × size × agreement` —
   `tight = 1 − max(midChroma, shadowChroma)/cap` on CORRECTED chroma (was
   mid-only, uncorrected); `size = n/(n+256)` (`neutral_axis_confidence_n0`);
   `agreement` = 1 minus the mid↔shadow R/B deviation-difference beyond a
   0.10 dead zone, rolled off over 0.20 (`neutral_axis_agreement_deadzone`/
   `_scale`). Drives our `strength = confidence × slider` directly.
4. **Same-pixel colour floors** (`_same_pixel_color_floor_refs` +
   `analyze_log_exposure_bounds_from_log` wiring): the colour axis's dense
   end (print whites) now reads ONE shared chroma-gated pixel set — the
   luma-extreme percentile band `[colorClip, colorClip+4]`
   (`color_bounds_band_width` 4.0), chroma measured base-anchored (offsets
   from the thin-end refs, per-channel span as provisional gamma, refined
   once from band medians, same two-pass loose/strict caps) — instead of
   independent per-channel percentiles, so coloured highlight content stops
   masquerading as film cast. Falls back to the percentile pass when the
   band holds no trustworthy neutrals. The thin end stays percentile-based
   (film density is bounded below by base). Maps to `BoundsAnalysis.analyze`'s
   colour axis.

   New `K` constants: `neutral_axis_chroma_cap` 0.35→**0.29**,
   `neutral_axis_first_pass_cap` **0.55**, `neutral_axis_confidence_n0`
   **256**, `neutral_axis_agreement_deadzone` **0.10**,
   `neutral_axis_agreement_scale` **0.20**, `color_bounds_band_width`
   **4.0**. All CPU-side (no MSL duplicates). `dump_fixtures.py` verified
   compatible before the re-dump — every imported symbol's signature is
   unchanged at 0369b10.

**Not applicable:**
- `127bcd7`'s chart-parity half: `cast_solve_inputs` (single source of truth
  for CPU processor + chart — our `deriveRenderParams` already IS that),
  `CharacteristicCurve` gaining `curvature` (chart-only class; their render
  path `apply_characteristic_curve` always had it, as does our
  `ReferenceCurve`), GPU engine exporting raw cast refs (chart plumbing).
- `ab5a6ad` sensor-crosstalk calibration: 3×3 CFA unmix on the linear
  capture for single-shot narrowband-LED camera scans, calibrated from bare-
  light exposures, default None, gated off for RGB-triplet composites —
  capture-side crosstalk stack we don't ship. **Noted for the crosstalk
  thread:** upstream now formally splits sensor crosstalk (linear domain,
  pre-inversion, per-setup) from dye crosstalk (density domain, Density
  Mixer) — the planned `negcli chart-solve` is on the dye side and remains
  a SwiftInvert-original.
- `8a1a3e1` selectable scanner backend, `6cea8a5` live-view drop/output-
  folder guards (capture stack); `d7e2502` IR sidecar case-insensitive
  matching (retouch); `59e5352` DMG target-arch build flag (their
  packaging); `6e294fa`/`d7ca80c`/`5a9cc2d`/`9b2345a`/`91cac6d` docs,
  `76ae85f`/`f85877b` changelog, `0369b10` lint.

**Still open (carried over):** `91a1b78` user-tunable Auto Density / Auto
Grade targets (blocked on a Settings surface); the on-scan Color Mixer band
re-tune pass (ours, post-b3490eb).

### 2026-07-23 — through `07dd965` (0.41.0 → 0.42.0, 7 commits)

**Kernel status: untouched — a genuine null.** The path-filtered log over
`features/exposure/`, `features/process/`, `kernel/image/` and the
characterization goldens is empty; no renames in the range; the full
`diff --stat` touches only desktop/Qt, presets, contact-sheet and export-form
code. The one rendering-adjacent hunk was read in full:
`services/rendering/image_processor.py` (+18) is the contact-sheet tile
downsampling fix (`fdf43b7`) — memory management in a feature we don't ship,
zero kernel lines. No fixture re-dump, no constants drift,
`dump_fixtures.py` unaffected. Changelog: 0.41.0 finalized + 0.42.0 opened,
all entries UI/workflow/export-plumbing.

**Ported:** nothing (nothing required).

**UI ideas worth copying independently (not pipeline):**
- `ec66499` per-setting copy/paste + apply-to-roll: pasting opens a picker
  listing exactly the settings that differ on the source frame, grouped by
  section with values shown; per-frame geometry never overwritten. Our
  Copy/Paste Adjustments (⇧⌘C/⇧⌘V) is all-or-nothing (geometry excluded) —
  the granular picker is the natural upgrade if multi-frame workflows grow.
- `1dcccbe` presets redesign builds on the same picker (a preset stores
  exactly the ticked settings, applied as overlay or replace) — relevant
  only if we ever grow presets.

**Not applicable:** `fdf43b7` contact-sheet tile memory (feature we don't
ship), `4772dcd` export-destination-mode restore (their export form; our
`exportOptions` JSON blob round-trips the destination already), `07dd965`
print-border tool-coordinate compensation (we ship no print borders, and
our coordinate space is unified by baking orientation into pixels),
`5e08828`/`650cf4b` changelog/lint/skill churn.

**Still open (carried over):** `91a1b78` user-tunable Auto Density / Auto
Grade targets (blocked on a Settings surface); the on-scan Color Mixer band
re-tune pass (ours, post-b3490eb).

### 2026-07-22 — through `a9e5169` (0.39.0-dev, 3 commits)

**Kernel status: untouched — a genuine null.** The path-filtered log over
`features/exposure/`, `features/process/`, `kernel/image/` and the
characterization goldens is empty; no VERSION/CHANGELOG/PIPELINE.md movement.
No fixture re-dump, no constants drift, `dump_fixtures.py` unaffected.

**Not applicable (all three, each diff checked):**
- `a9e5169` ICC tagging fixes across export formats — read in full because
  export tagging is a shared bug class, but all four defects live in features
  we don't ship: greyscale JPEG/WebP CMS (B&W), ACES/XYZ export targets
  (rawpy decode spaces; our export offers only sRGB/Adobe RGB), untagged
  contact-sheet JPEGs. Our ImageIO path tags from the CGImage's colorspace
  and can't produce the swallowed-lcms-error untagged case.
- `40f7d2f` toolbar streamline + compare/flat-peek stabilization — Qt UI.
  Their fix (rotate/flip re-render INSIDE an active compare view via
  `rerender_active_view` instead of dropping to the plain edit) doesn't map:
  our baseline compare is hold-to-press (`showingBaseline`), so geometry ops
  can't interleave with it, and we have no flat-peek.
- `1d48e01` stitch-config sidecar-key whitelist warning — scan stitching,
  capture side.

**Still open (carried over):** `91a1b78` user-tunable Auto Density / Auto
Grade targets (blocked on a Settings surface); the on-scan Color Mixer band
re-tune pass (ours, post-b3490eb).

### 2026-07-21 — through `8d6f44e` (0.39.0-dev, 21 commits)

**Kernel status: untouched — a genuine null.** The path-filtered log over
`features/exposure/`, `features/process/`, `kernel/image/` and the
characterization goldens is empty; no renames in the range; the pipeline-tree
diff is empty. No fixture re-dump, no constants drift, `dump_fixtures.py`
unaffected. The range is film-scanner integration (Coolscan/SANE `bba487d`,
RGBI + IR dust over SANE `8d6f44e`), IR dust reconstruction rework
(`f612a74`), lab sharpen improvements (`d3436c7` — stage we don't ship),
multi-part scan stitching (`5e141df` — capture-side composite assembly),
contact-sheet/DB/UI work, and docs churn.

**Noted for the active crosstalk thread (not a pipeline change):** `a27f035`
widens the crosstalk matrix adjustment range in their EDITOR dialog by one
line — the matrix math is untouched. Confirms the Density Mixer remains
manual upstream; the chart-based auto-solve being planned here
(`negcli chart-solve`, see 2026-07-20/21 conversations) stays a
SwiftInvert-original with no upstream counterpart to converge with yet.

**Not applicable:** `2d8e55e` TIFF sRGB decode gating (their TIFF ingestion;
we're camera-RAW only), `67e2051` scan-window exposure gating, `ce28dc1` DB
dialog, `9c260e2` contact-sheet labels, `2cdc569` filmstrip thumbnails,
`b805578` icon/migration fixes, readme/changelog/tutorial commits.

### 2026-07-20 — through `2cdc569` (0.39.0 → 0.40.0, 9 commits)

**Kernel status: untouched.** Zero commits in the range touch
`features/exposure/`, `features/process/`, `kernel/image/`, or the
characterization goldens — the path-filtered log AND `git diff --stat` over
those trees are empty, and there are no renames in the range
(`--diff-filter=R` empty). The shared-kernel-adjacent hunks were checked by
hand: `lab.wgsl` + `lab/logic.py`/`models.py` changed ONLY the sharpen block
(rewritten for the 0.40.0 sharpening feature — zero lines touch vibrance/
saturation/Dye Mute or the Lab matrices our `colorPop` mirrors), and the
`domain/models.py` hunks are sidecar migrations for features we don't ship
(IR inpaint radius, filed-carrier toggle fold). No fixture re-dump, no
constants drift; `dump_fixtures.py` signatures unaffected (exposure
`normalization.py`/`logic.py` unchanged).

**Ported:** nothing (nothing required).

**Divergence baseline moved (no action):** `d3436c7` rebuilt sharpening —
Method selector (Unsharp Mask with halo-suppression overshoot clamp +
gradient Masking, or Richardson-Lucy deconvolution on linear Y applied as a
chroma-preserving RGB ratio), Radius/Masking sliders, preview/export parity
via shared `gaussian_kernel_1d` taps (the old fixed 5×5 GPU kernel sharpened
the wrong band at export scale). We don't ship sharpen (recorded
divergence); if it's ever implemented, port THIS version (PIPELINE.md §5.4
documents the full math). Default `sharpen` stays 0.25.

**Noted upstream self-doubt on Dye Mute (strengthens our skip):**
PIPELINE.md now carries "(The default was tuned against the old ProPhoto
working gamut — it may run strong now that the working space is Adobe RGB.)"
— upstream themselves flag the 0.5 default as possibly too strong post-
b3490eb. Our 2026-07-17 skip stands; reopening condition unchanged
(visible oversaturation at hard grades on real rolls).

**UI idea worth copying independently (not pipeline):** `b805578`'s
analysis-region active-indicator — a small dot on the Freedraw Analysis
Region button whenever a custom region is overriding the buffer. We have the
same override semantics (`resolve_analysis_region` port) and the same
discoverability gap after the tool closes.

**Not applicable:** `a27f035` crosstalk matrix range (stage we don't ship),
`bba487d` Coolscan/SANE film scanning (capture stack), `f612a74`/`8a98326`
IR dust reconstruction (retouch; runs pre-normalization on IR-carrying
scans — we're camera-RAW only, no IR plane), `d3436c7`'s non-sharpen bulk
(their GPU engine sharpen-pass plumbing), `c3e83ff` tooltips, `b805578`'s
icon fixes/carrier migration, `c53aa26` USER_GUIDE rewrite, `2cdc569`
filmstrip thumbnail fixes.

**Still open (carried over):** `91a1b78` user-tunable Auto Density / Auto
Grade targets — blocked on us growing a Settings surface; the on-scan
Color Mixer band re-tune pass (ours, post-b3490eb).

### 2026-07-19 — through `96adfde` (0.38.0 → 0.39.0-dev, 19 commits)

**Seven pipeline-relevant commits — the deepest range since the original
port, headlined by a full working-space swap.**

**The headline: `b3490eb` Adobe RGB (1998) working space.** Upstream
REVERSED the 2026-07-16 ProPhoto output fix: full ProPhoto primaries at
output are now judged "unnaturally saturated, pain to correct", and the
whole pipeline boundary moved to Adobe RGB — OETF ROMM TRC (1.8 + linear
toe) → pure 563/256 gamma (no linear segment), output tagging, Lab
matrices, CLAHE/toning shaders, display/export management, ALL goldens
regenerated. Dye Mute stays at default 0.5 on top, so their canonical look
is now markedly more muted than either the pre-fix or the ProPhoto-era look.
**Third output-space change in four days** (stale Adobe RGB bug → ProPhoto
+ Dye Mute → deliberate Adobe RGB).

**Judgment (revised 2026-07-20 after researching issue #537 / PRs
518/538/544):** the "third change in four days" framing was wrong — this is
NOT a flip-flop. NegPy's output had been Adobe-RGB-interpreted for its
whole life via a stale tag; PR #518 exposed true ProPhoto for the first
time, two independent users immediately reported neon reds AND hue shifts
(red→magenta, sky→cyan) with image evidence, Dye Mute could damp chroma but
not hue, and #544 restored Adobe RGB coherently (primaries + pure 563/256
TRC + D65) within three days. The argument (user thetalkingdrum, adopted
verbatim by the maintainer): the pipeline ASSIGNS primaries to raw
uncharacterized sensor RGB at output — an interpretation, not a conversion
— and ProPhoto's imaginary green/blue primaries stretch dye separations
outside the spectral locus. Decision is settled upstream (both reporters
confirmed the fix; follow-ups are routed to crosstalk characterization).

**The argument applies to SwiftInvert verbatim** — we tag sensor-native
pipeline output as ROMM the same way — and one of our own divergences may
be evidence: redHue +0.5 exists because "C-41 reds skew magenta out of the
box", which matches the reported ProPhoto-interpretation hue shift exactly.
A correct-primaries output might let redHue return toward 0.
**PORTED 2026-07-20** — the user chose convergence over A/B ("I want to
stay close to the main project"). Full port: WorkingOETF → pure 563/256
gamma (Swift + MSL 0.45470693 literal), Lab matrices/white → Adobe RGB
D65 both sides, output tagging → adobeRGB1998 (ImageConversion, ColorIO
workingColorSpace, negcli TIFF), littleCMS oracles regenerated from
NegPy's AdobeCompat-v4/sRGB-v4 at 96adfde, fixtures re-dumped from
96adfde (dump script adapted to the paper_black rename, manifest key
stable), coupled constants ported (anchor 0.75, grade target 0.6,
strength 0.5). Space-sensitive tests recalibrated with intent preserved:
True Black's contract restated in the linear domain (the old encoded
thresholds baked in the ROMM toe), pre-saturation chroma margin 1.05 →
1.02 (smaller-gamut Lab), mixer-band test pixel moved in-band (real-
content blues land 240–265° in the new Lab — the band constants
themselves survive). ExportColorSpace's wide option relabeled Adobe RGB
(case name kept for sticky-JSON compat). Export-note: existing exports
re-render slightly different, as upstream's changelog warns. Follow-ups:
redHue +0.5 RETIRED same day (it countered the hue skew this port removes
at the root; default 0 again, explicit sidecar values preserved); the
on-scan mixer-band re-tune pass remains open. 132 tests green incl. GPU parity against Adobe-world
fixtures; bench unchanged.

**Coupled to the above — do NOT port separately:** `2db0470` + `088c393`
auto-constant tunings (anchor_target_density 0.74 → 0.75, auto_grade_target
0.55 → 0.6, auto_grade_strength 0.3 → 0.5, goldens moved). These were tuned
against the Adobe RGB output; applying them under our ROMM output applies
their new-space taste to our old-space look. Port together with b3490eb or
not at all.

**Ported (2026-07-20) — we shared the bug:** `2125a34` Cast Removal
neutral axis vs PRE-trim bounds. Their CPU measured the neutral axis
against user-trimmed (WP/BP-adjusted) bounds while their GPU measured
pre-trim; they standardized on pre-trim ("the film's inherent cast is a
source property — creative trims shouldn't perturb it"). Our `finalize`
had faithfully ported the now-declared-buggy CPU side. Ported as the
simplification it implies: `finalize`/`analyze` lost their offset params
(the analysis is now fully offset-independent; offsets fold into
finalBounds at derive time only), ImageSession's second cache tier
(AnalysisKey) dissolved — wp/bp handle drags re-run no analysis at all —
and ImagePipelineSeamTests pins the new semantics (axis == base-bounds
measurement; offsets still reach the render). Fixtures unchanged (all
dump configs use zero offsets); CLAUDE.md §2 updated.

**To port (proposed, needs a Settings surface + the visible-improvement
bar):** `91a1b78` user-tunable Auto Density / Auto Grade targets —
app-global calibration (TUNABLE_TARGETS ranges over 5 anchor/grade
constants, Set Targets dialog, TARGETS_REVISION cache-bust). The right
shape for per-scanner calibration; we have no Settings window yet (TODO).

**Bookkeeping (no action):** `687bcd5` True Black renamed **Paper Black,
inverted** — new default (off) keeps BPC on, so upstream's effective
default still matches ours; only nomenclature/polarity changed. Our
`trueBlack` field and UI naming stand; renaming to match upstream is a
cosmetic option, recorded here so the next review doesn't re-derive the
polarity mapping (paper_black = !bpc).

**Not applicable:** `81bea3c` half-frame mode (their asset/session model),
`4678569`/`96adfde` camera-scanning fixes, `7e888ef` dodge/burn mask
visibility, `13a7ba5`/`3113241` update banner, `b14dc62` tutorial,
docs/changelog/roadmap churn (`a11535f` `18f6dd5` `7a0046d` `46c85c2`
`c27b379` `5bd0c03` `57948f5` `0ca43de`-style lint).

**dump_fixtures.py: BROKEN against ≥b3490eb** — `_oetf_encode_flat`/
`_oetf_decode_flat` lost their break-point parameters and the OETF
semantics changed; the closed-form `working_oetf` fixture case would dump
Adobe-gamma values against our ROMM oracle. Verify/adjust the script
BEFORE any re-dump past b3490eb (it still works at ≤6b841a1 checkouts).

### 2026-07-17 — through `0ea27d2` (0.38.0 tail, 6 commits)

**One pipeline-relevant commit, and it's a golden move:** `8bc9678`
**Dye Mute — grade-coupled chroma damping**, ON by default upstream (0.5),
relocation goldens regenerated. Mechanism: `damp = (slope_min/slope_g)^strength`
(green reference slope, clamped) folded into the Lab saturation multiplier at
parameter-derivation time — per-channel print curves multiply channel
separation by the slope, so chroma inflates with grade; the damping counters
it ("mimicking paper dyes' unwanted absorptions"). No WGSL/uniform change
upstream; the identical fold works here (`colorPopActive` reads derived
params, so `params.saturation × damp` self-activates the pass). Port cost
~15 lines + tests, NO fixture re-dump (derivation-side; `apply_saturation`
unchanged). `dump_fixtures.py` signatures unaffected (new function only).

**Deliberately skipped (for now) — reopening condition recorded:** Dye Mute
is not ported despite being a golden move. Their motivation was a regression
fix: the ProPhoto output bug had been squeezing their gamut for releases;
fixing it (`07e3f8f`) suddenly exposed full-gamut chroma their users never
saw, reading oversaturated at hard grades. **We never had the bug** — our
color has been full-gamut ProPhoto from day one, and our deliberate
divergences (preSaturation 1.15, redHue +0.5) were A/B-tuned ON that gamut
toward MORE color, not less. Adopting their 0.5 default would desaturate our
established default look ~15% at typical grades (up to ~55% at slope 10);
adopting the slider at 0 would add exactly the kind of dormant control the
2026-07-16 removals just cleared out. Reopen if hard-grade frames ever
visibly oversaturate on real rolls — the mechanism above is the ready-made
fix, and one negcli A/B decides the default.

Also in `8bc9678`: default lab sharpen 0.5 → 0.25 (we don't implement lab
sharpen — divergence note updated in CLAUDE.md).

**Not applicable (this range):** `5811662` crop-ratio picker consolidation
(their UI; NOTE their final ratio list — 7:5, 16:9, 16:10, US Letter — as
reference for our own TODO'd aspect presets), `611b251` Finish/geometry
sidebar cleanup + auto-narrowband-on-RGB-scan (capture-side scanning stack),
`0ca43de` lint, `1c07529` tooltips, `0ea27d2` changelog. Changelog-only
item: "Cast Removal strength sticks across frames" is their session-sticky
settings model — ours is per-image sidecars + explicit Copy/Paste, an
architectural divergence, not a gap.

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

### 2026-07-16 — film-base sampling and the cast experiments REMOVED

The 2026-07-15 "Ported" entry for `77c8113`-adjacent film-base sampling is
superseded: **Sample Film Base was removed** after its trial, together with
the three patent-derived cast experiments built 2026-07-16 (saturation-
weighted colour metering, tempered cast removal, roll balance — see
PLAN-cast-experiments.md at `2b73c5d` for the mechanisms). All were
default-off and parity-clean; all verified mechanically correct end to end.

**Why removed:** the trial standard was "visibly better on real rolls", and
none met it. On neutral-rich frames every method converges with the existing
statistical estimate BY DESIGN (film-base agreed within 0.02D; weighted
metering moved bounds ~0.01D; tempered cast at shipping constants shifts
pixels ~1/7 of an 8-bit step). The existing two-axis bounds + confidence-
gated neutral axis already handle these rolls well; the added UI surface
wasn't paying for itself. Direction judged wrong by the user 2026-07-16.

**Do not re-propose without new evidence** — specifically, a real roll where
casts visibly fail (dominant color, no neutrals, no usable rebate). If that
roll appears, the removed work is complete and recoverable:
`fed271a` (film-base sampling), `fa3e095` (weighted metering),
`36ae788` (tempered cast), `420f583` (roll balance), `2b73c5d` (plan) —
local-only commits, never pushed, reachable via reflog until gc.

Kept: the NegPy 0.38 constants port (`1c1c3cb`, this entry's parent), the
spot densitometer / zone strip / negative character (shipped earlier and in
regular use), and the research findings in the 2026-07-16 conversation.
The densitometer remains the darkroom read-out surface; sidecars and the
one roll-profile dotfile were scrubbed of the removed features' keys.
