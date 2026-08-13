# Upstream (NegPy) review log

SwiftInvert ports NegPy's negative-inversion pipeline; NegPy
(`~/Documents/code/NegPy`) keeps evolving. This file records **the last NegPy
commit we reviewed**, so "what changed since we last looked?" always has a
baseline. Keep it current: every upstream review ends by updating the marker
and appending a history entry.

## Last reviewed

```
commit:   2c46460  ("Badge composite frames on the film strip (#817)")
reviewed: 2026-08-12
fixtures: Tests/Fixtures/ dumped from 0369b10 except lab_color (partially
          re-dumped from a09cc46, 2026-07-31, with the pure gamut boost —
          the b8c596c dump had carried the interim in-boost skin damping).
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

### 2026-08-12 — through `2c46460` (0.49.0 unreleased, 12 commits)

**Nothing to port — but the review's own bug-class audit found a REAL,
reachable bug of ours, and it is exactly the shape of the one upstream spent
this range fixing.** Goldens unmoved (empty diff), `EXPOSURE_CONSTANTS`
untouched, no renames, VERSION still 0.49.0. The whole diff over the three
pipeline dirs is **41 lines in two files** — `exposure/processor.py` (one
line) and `process/logic.py` (38) — both from `a1f1a9c`, and both on the E-6
transfer path. `normalization.py`, `logic.py`, `models.py`, `analysis.py`,
`stats.py`, `densitometer.py` and every `exposure/shaders/*.wgsl` have an
**empty diff**.

**Ported:** nothing (nothing applicable).

**Deliberately skipped:**

1. **`a1f1a9c` `effective_linear_raw`** — the E-6 as-captured decode was
   obeying the Linear RAW toggle their Process panel documents as inert
   there, so a hidden sticky flag decided whether `use_camera_wb` applied. On
   a bracket that is expensive: `use_camera_wb` reads each *file's* as-shot
   multipliers, and a camera on auto white balance records different ones per
   frame (their 8-frame slide bracket: darkest frame B/G 1.41 against
   ~1.8–2.1), so `pair_ratio` cannot tell an exposure difference from a
   white-balance one and the shortest link solved to 0.75 EV instead of 1.00
   — printing as contour rings around a blown sun. **Unreachable from C-41
   twice over.** `effective_linear_raw` is `process.linear_raw or
   is_transparency_transfer(...)`, and the second term is false for C-41 by
   construction, so on our process mode the helper is *identically* the
   stored flag — no C-41 number moves. And we ship no Linear RAW toggle at
   all: `RawDecoder` **always** decodes unity WB (`use_camera_wb=0`,
   `user_mul=(1,1,1,1)`), which IS their linear_raw leg, and applies no
   camera matrix (`output_color=RAW`). Their invariant — "the decode and the
   matrix must make the same choice" — is satisfied here by having neither.
   Grepping the whole 12-commit diff for every parameter in our decode
   contract returns only prose and their transfer-path lines.
2. **`85afd44` HDR merge of bracketed captures** — a 424-line
   `features/hdr/`: exposure ratios solved from the images rather than
   shutter tags (`pair_ratio`), frames registered, samples combined by
   inverse variance (weight ∝ r², which is what makes the merge no worse than
   the best single frame), merged straight to float32, and a seeded Shadows
   Density lift because merging unclipped frames buys **precision, not
   range**. Skipped on upstream's own argument, which lands on our scope
   boundary by name: they now **hide the merge action on C-41** because "a
   colour negative holds about 5-6 stops between base and Dmax … both inside
   a single capture; a transparency runs to 10-12." We are C-41 only. It is
   also assembly rather than a stage — "no shader and no GPU parity surface"
   — so there is nothing in it for our kernels either.
3. **`5a885db` multi-core CPU rendering as a setting** — their Numba CPU
   kernels compiled serial and parallel and dispatched per call. No
   counterpart: our render path is Metal/Vulkan, and `ReferenceCurve` is a
   correctness reference (and the negcli no-GPU fallback), not a path anyone
   waits on. **Their macOS finding is worth one line anyway**, since it is
   the platform we ship: Numba's workqueue layer *terminates the process* on
   concurrent entry with no exception and no log line, so they default macOS
   off and record a clean-exit flag to notice afterwards. The transferable
   part is the shape of the mitigation — an abort that leaves no trace is
   un-diagnosable unless the *next* launch can see the last one didn't
   finish.

**Checked, and this is the substance of the review: their cache-key
discipline, audited against ours — WE HAVE ONE OF THESE BUGS.**

`a1f1a9c` also fixed `linear_raw_token`, which keyed the render source hash
on the **stored** flag while the decode had begun using the **effective**
value, so "the buffer decoded the other way" could be served under a matching
key; and the range adds `services/rendering/source_identity.py`, whose
docstring states the rule generally: *"Source assembly happens in more than
one place … and each used to build its own cache key by hand. A key that
forgot a field served the previous buffer for a setting the user had just
changed."*

We built our tower the same way, so it was worth checking rather than
assuming. Three of the four keys are clean, and one is not:

- **`MeterKey.meterAngle` is already the effective-value pattern done
  right** — `settings.analysisRect != nil ? settings.analysisRectFineRotation
  : 0` (ImageSession.swift:31), i.e. the key is computed by the same
  expression the work uses, not read off a stored field.
- **`OrientKey.fineRotation`** carries the raw `Double` while `oriented`
  only rotates past 0.005, and **`PreparedKey`** carries the raw rects while
  `prepare` ignores degenerate ones (<2 px). Both key *finer* than the
  computation, which can only over-invalidate — the safe direction, and the
  one upstream's rule permits.
- **The bug: `prepare()`'s COW alias ignores the very angle `MeterKey` was
  careful to derive** (ImageSession.swift:251-263). At |fineRotation| ≤ 0.005
  the render input is aliased to the meter image — `preview = meterPreview`,
  "COW: at 0° the render input IS the meter image, no copy" — but
  `meterPreview` was built at `mKey.meterAngle`, which is **not** the
  straighten angle when an analysis region exists. So the alias is only valid
  if the meter angle is also ~0, and nothing checks that.

  Reachable in three clicks, with a one-click path straight into it:
  1. Straighten to a nonzero angle.
  2. Crop for Analysis (⇧⌘K), draw a region → `commitSelection` sets
     `analysisRectFineRotation = settings.fineRotation` (AppModel.swift:296).
  3. Return straighten to 0 — the slider, an undo, or **the ⨯ /
     Image ▸ Clear Crop & Straighten**, which zeroes `fineRotation` and
     leaves `analysisRect`/`analysisRectFineRotation` untouched
     (`clearCropAndStraighten`, AppModel.swift:233-245).

  `mKey` is unchanged across step 3 (it holds the draw angle, not the
  straighten angle), so `meterPreview` is not rebuilt; `oKey` changes, the
  branch re-evaluates, `abs(0) > 0.005` is false, and the render input
  becomes the image rotated by the *stale draw angle* and auto-cropped to its
  inscribed rectangle. **The metering is correct** — that is the
  fine-rotation invariant working as designed — it is the render alias that
  is wrong.

  Both halves of the render are then wrong, and the second is the one the
  file already warned about: `displayAspect` and `contentWindow` read
  `settings.fineRotation` and compute `radians = 0`, against the comment four
  lines up — *"the conditions here must mirror it and `sourceTexture`
  exactly, or the layout would describe a rectangle the render didn't
  produce."* On a 1536×1024 proxy at 10° the inscribed rect is 1420×789, so
  the canvas lays out a 1.80-aspect bitmap as 1.50 **and** shows a picture
  visibly tilted at an angle the straighten slider reads as zero.

  **FIXED 2026-08-13 (same session, user approved).** The COW is now taken
  only when `meterPreview` was itself built unrotated: the decision moved out
  to a pure `ImageSession.aliasesMeterPreview(fineRotation:meterAngle:)`,
  which requires BOTH angles flat at the same 0.005 threshold
  `RGBImage.oriented` uses to decide whether a fine rotation is applied at
  all. Otherwise `prepare` falls through to an explicit
  `basePreview!.oriented(…, fineRotation: settings.fineRotation)`. The fast
  path survives in the overwhelmingly common cases (no analysis region, or
  one drawn at 0°) — the copy is paid only in the state that was broken.

  Pure-decision-plus-test rather than a test of the actor, following
  `HQMode.resolve`'s precedent (ImageSession needs Metal and a real RAW, and
  CLAUDE.md keeps it UI-verified). `MeterPreviewAliasTests` pins the fast path
  in both flat cases, refusal at each angle independently, the threshold
  against `oriented`'s, and — so the suite can't go vacuous — that a pinned
  angle really does change the render input's **shape** (1536×1024 inscribes
  to a different aspect at 10°), which is what made the alias a visible bug
  rather than a redundant copy. The bug case (`alias(0, 10)`) returned `true`
  under the old condition, so the test fails against the code it replaced.
  292 tests green; the app renders headlessly through the real `prepare`.

  No settings field, no kernel change, no parity surface, no fixture re-dump,
  no tripwire counts, no SPIR-V rebuild.

  **Left open deliberately (two design calls, not defects):** whether
  `clearCropAndStraighten` should also clear the analysis region's pinned
  angle — the same question one level up, and arguably what a user clicking
  one ⨯ expects; and the Qt/bridge divergence below.

- **A separate, pre-existing cross-platform divergence, found while
  confirming the Linux side was clean.** The bridge `Session` never
  implements the pinned angle at all: `meter()` (CoreBridge.swift:80-89) keys
  on rotation and flip only, and `source()` always builds its own oriented
  buffer from the tier base rather than aliasing the meter image — so Linux
  was immune twice over and needed no fix. But sidecars round-trip between
  the two platforms by design, so a sidecar carrying a nonzero
  `analysisRectFineRotation` meters a **different patch of film** in Qt than
  on the Mac, and the same file converts differently on the two frontends.
  Narrow to reach (it needs a region drawn while straightened) and not
  introduced by anything in this range. Decide it deliberately: teach the
  bridge the pinned angle, or record it as a divergence in CLAUDE.md.

**Checked, no counterpart (rest of the shared-bug-class sweep):**
- **`65893db` "Never file a render under a frame it did not come from"** —
  the follow-up to `169405c` (reviewed 2026-08-11 as N/A), and it turned out
  the first fix closed only half the hole: on a `source_hash` miss it still
  fell back to the current selection, which fires whenever the list has moved
  on, and the write is persisted so the wrong picture survived restarts.
  Found on disk, not reasoned about — an audit of 68 files by structural
  correlation caught one frame scoring 0.10 against its own file while
  matching another frame's thumbnail at 1.00. **We remain structurally immune
  and it is worth recording why precisely**: our thumbnails are not renders
  at all. `ThumbnailStore` is `[URL: CGImage]` of the *embedded camera JPEG*
  (`RawDecoder.embeddedThumbnail`), keyed by the URL it was decoded from and
  never written from a render's output, so there is no attribution step to
  get wrong and nothing persisted to outlive the mistake.

**Not applicable:** `d78d8f4`/`9167a90`/`c6f07d4` Pieusb scanner backend +
its CI (capture), `5fa7032` a Nikon HE/HE\* NEF (intoPIX TicoRAW under a
licensed codec) parsing cleanly and reporting full dimensions but failing at
unpack with a bare "Unsupported file format", now named with advice — same
family as the `1956dd0` JPEG-XL DNG gap recorded 2026-08-09, and our
`RawDecodeError.libraw("unpack", code:)` has the same terse-but-graceful
failure they improved on (a nicer message is a cheap future courtesy, not a
port), `895e650` current-file export dropping triplet/stitch config,
`2c46460` composite badges on the film strip, `d8b5b42` Hot Folder polling
quieting the import popup, `a51ddab` "1 file" not "1 file(s)" across fifteen
messages, plus the composite-inheritance fixes riding `85afd44` (a stitch
inheriting its primary part's edit by path, film process by majority vote,
Reset Settings no longer un-stitching) — their asset/DB layer; we have one
file per frame and one sidecar beside it.

**dump_fixtures.py:** compatible, checked import by import since one file it
imports did move. `exposure/processor.py`'s only change is inside
`NormalizationProcessor._process_transparency` (an import plus one call
swapped to `effective_linear_raw`); `PhotometricProcessor` is untouched, both
constructors are unchanged, and the script constructs
`PipelineContext(..., process_mode=ProcessMode.C41)` so that method is never
entered. Everything else it imports — `exposure/logic.py`,
`exposure/normalization.py`, `exposure/models.py`, `exposure/papers.py`,
`process/models.py`, `kernel/image/logic.py`, `domain/interfaces` — has zero
lines in the range. `process/logic.py` changed, but the script does not
import from it. The `fixtures:` line does not move.

**Still open (carried over, unchanged):** colour ring-around (±4cc/2cc spec),
`91a1b78` tunable Auto Density/Grade targets (user-initiated only), the
on-scan Color Mixer band re-tune pass (ours). **New and ours, both design
calls left open by the fix above:** whether `clearCropAndStraighten` should
clear the analysis region's pinned angle, and whether the Qt bridge should
learn that angle or record the divergence.

### 2026-08-11 — through `cb876d4` (0.49.0 unreleased, 6 commits)

**Two pipeline-relevant commits, both outside our scope — but one of them is
the second matrix-orientation bug upstream has shipped, and it comes with a
testing rule worth adopting by name.** Goldens unmoved (empty diff),
`EXPOSURE_CONSTANTS` untouched, no renames, VERSION still 0.49.0. The whole
diff over the three pipeline dirs is **two files**: `papers.py` (additive) and
`capture_color.py` (E-6 only). `normalization.py`, `logic.py`, `processor.py`,
`models.py` and every `exposure/shaders/*.wgsl` have an **empty diff**.

**Ported:** nothing (nothing applicable).

**Deliberately skipped:**
1. **`3f803fe` Lith printing** — infectious development as a render stage
   (`features/lith/` + `lith.wgsl`), plus a `lith_path` colour path per paper
   profile: four (a\*, b\*) anchors at density fractions 0.10/0.35/0.65/1.00,
   peach → ochre → **olive** → neutral, where the olive knot is the point ("the
   green transition between warm highlights and cold blacks is the signature of
   a lith print on warmtone paper"). Papers and toning are recorded scope
   boundaries. Its only reach into a directory we track is the additive
   `PaperProfile.lith_path` field — **no tonal constant moved**.
2. **`cb876d4` Cyanotype printing** — an alt-process stage
   (`features/cyanotype/` + its own WGSL, `features/altprocess/`). Same
   boundary; we ship one C-41 print path and no alt processes.

**Checked, no counterpart — but the discipline transfers, and this is the
substance of the review.** `ae7169f` fixes `camera_to_working_matrix`:
it inverted the camera's XYZ→cam matrix and *then* row-normalized, where
dcraw's `cam_xyz_coeff` normalizes the **forward** working→cam rows and inverts
after. The module docstring had claimed the dcraw construction while doing the
opposite. Measured against libraw's own cam→sRGB: 5–7% off in R/G on unclipped
frames (now within 0.1%), and **+119% on a saturated sunset**, doubling B/G
too — a warm sky rendering as neon magenta that reads as runaway saturation.

- **No counterpart here, structurally.** We never *build* a matrix by
  inversion. Our working-space matrices are published Adobe RGB (1998) D65
  literals carried as an explicit pair (`LabColor.toXYZ` / `toRGB`,
  duplicated in MSL and GLSL), and the Vulkan canvas transform composes two
  literals in the correct direction (`SRGB_FROM_XYZ * (WORKING_TO_XYZ * lin)`,
  NegPipeline.comp). Our exposure to this bug class is transcription drift
  between the three mirrors, not construction order — and per CLAUDE.md that
  is what the GPU-vs-CPU parity suites exist to catch.
- **The rule to keep is about witnesses, not matrices.** Their commit message
  states it exactly: *"No structural invariant separates the two orders:
  normalizing either direction gives M @ (1,1,1) = (1,1,1), and a matrix that
  maps neutral to neutral has an inverse that does too, so the coefficients are
  the only witness."* A grey chart cannot catch it; the error only appears far
  from neutral. **Our colour chain already has the right witness, verified
  rather than assumed**: `ColorIOTests.cases` pins the export transform against
  NegPy's littleCMS oracle on saturated red `[200,60,50]`, green `[70,190,60]`
  and blue `[60,80,200]` alongside the neutral `[128,128,128]` — a wrong-but-
  neutral-preserving matrix breaks the three chromatic cases while the grey one
  passes. `HueTrimTests` follows the same principle from the other direction
  (it asserts the effect GROWS with chroma, which is what distinguishes a
  rotation from a cast). Keep new colour-transform tests on that side of the
  line: **a neutral is never a sufficient witness for a colour transform.**
- Worth noting this is upstream's **second** matrix-orientation bug: 2026-07-30
  records a transposed RGB↔XYZ matrix inside their gamut bisection that
  "masqueraded as float precision noise". Both were caught by a chromatic
  measurement against an independent reference, neither by an invariant.
- One incidental confirmation: the fix is scoped to the E-6 as-captured render
  on both engines, and their own 96-case sweep over frame × mode × normalize ×
  paper × intent × engine "moves exactly the 16 E-6 Normalize-off cases and
  leaves C-41, B&W and E-6 Normalize-on byte-identical" — independent
  verification of yesterday's conclusion that the whole transparency path is
  unreachable from a C-41 render.

**Not applicable:** `169405c` a finished render's thumbnail filed under
whichever frame was selected when the pixels arrived rather than the frame the
render was started for (their filmstrip cache; ours keys thumbnails off the
URL, and the render memo they cite as already correct is the pattern we use
throughout), `85cee44` macOS drawing a Fusion-style mnemonic underline for a
key macOS never binds, `4998c24` an overflow toolbar measuring items by
`sizeHint()` — which `setFixedWidth` doesn't change and a bare `QFrame` reports
as -1 — so 1 px dividers took 3 px slots and the bar reserved room for a **»**
button before knowing it needed one, conjuring the chevron it reserved for
(both Qt-shell only; note their CI runs Linux and the failure was macOS-only,
which is the mirror image of our own Metal/Vulkan split).

**dump_fixtures.py:** compatible. It imports `effective_paper_profile`, whose
signature is unchanged, and `PaperProfile` gained only a defaulted `lith_path`
field — no `_TONAL_KEYS` entry and no numeric change to the default profile the
C-41 fixtures resolve to. Nothing else it imports was touched. The `fixtures:`
line does not move.

**Still open (carried over, unchanged):** colour ring-around (±4cc/2cc spec),
`91a1b78` tunable Auto Density/Grade targets (user-initiated only), the on-scan
Color Mixer band re-tune pass (ours).

### 2026-08-10 — through `5a6cc97` (0.49.0 → unreleased, 5 commits)

**The biggest exposure-pipeline commit in months, and essentially none of it
reaches us: it is the E-6 slide path, which CLAUDE.md scopes out by name.**
`658e16e` is the only commit the path filter returns, and it is large — a new
`features/exposure/transfer.py` (280 lines) with its own `transfer.wgsl`, a new
`features/process/capture_color.py`, and edits to the two files we do mirror
(`normalization.py`, `normalization.wgsl`). **Both goldens are untouched**
(empty diff), `EXPOSURE_CONSTANTS` is untouched, no renames, VERSION still
0.49.0 (the range is unreleased post-0.49.0 work).

**Ported:** nothing (nothing applicable).

**Our parity surface is provably untouched, which was the thing worth
checking.** The new path is gated by one predicate, `is_transparency_transfer`
= `process_mode == E6 and not e6_normalize` (never true for C-41, and
explicitly false for the FLAT intent), and it is the single source of truth on
CPU, GPU and UI by design. Line by line, for the two mirrored files:
- `normalization.wgsl` gains a camera-matrix multiply in LINEAR before the log,
  behind `is_transfer = is_e6 && normalize_flag == 0`. The C-41 log path is
  character-identical. The NormUniforms block grows three `vec4` rows (112 →
  160 bytes) — irrelevant to us, our uniform layout is our own.
- `normalization.py`'s only C-41-visible change is `resolve_crosstalk_matrix` →
  `effective_crosstalk_matrix`, a mode gate on a feature we don't ship.
- `_sample_log_bounds`'s `e6_normalize` argument sits behind
  `if process_mode != ProcessMode.E6 or e6_normalize`, so the **default flip
  (`e6_normalize` True → False) cannot move a C-41 bound.**
- `PhotometricProcessor` gained an optional `process_config` that defaults to
  `ProcessConfig()`; the print branch below it is unchanged.

**Our decode contract is REAFFIRMED, not changed — worth recording explicitly,
because the headline reads like we're missing something.** `capture_color.py`
builds a working-space-from-camera 3×3 from libraw's `rgb_xyz_matrix` (dcraw's
`cam_xyz_coeff`: invert XYZ→cam against XYZ→working, row-normalize so WB keeps
the grey point) and applies it in linear before the log. That is exactly the
camera matrix we deliberately DON'T apply (`output_color=RAW`), and upstream's
own docstring gives our reason back to us: "The print path never needed the
distinction — it derives colour from measured film density and a paper model —
but a transparency transfer does." Their PIPELINE.md diff confirms every rawpy
parameter we pin is unchanged (`adjust_maximum_thr=0` and `no_auto_bright` are
now cited in the *transfer* rationale, as the reason an as-captured slide
arrives dark). Grepping the whole 5-commit diff for every parameter in our
decode contract returns nothing but prose.

**Deliberately skipped:**
1. **The E-6 as-captured transfer path** (`658e16e`) — B&W/E6 is a recorded
   scope boundary ("Scope: … no B&W/E6"), and this is the whole feature: a
   fixed 3.0-decade log window anchored to the decoder's white level instead of
   a metered one (so a bracket stays a bracket), the camera matrix above, and a
   plain transfer curve where Density/Grade/Toe/Shoulder are exactly identity at
   their defaults because the paper H&D curve is not neutralizable (their
   argument: `d_max` floors the blacks whatever the toe says, and the midtone
   snap and paper-white reference stay live at neutral settings — a positive
   needs its own curve). Nothing here is reachable from a C-41 render.
2. **`crosstalk_process`** — crosstalk is a recorded skip. Their point stands
   on its own terms (a matrix describes one dye set; every bundled profile is a
   colour negative, so a slide was silently getting a negative's correction),
   and it strengthens the 2026-08-05 reframing that a matrix belongs to a whole
   scanning setup rather than to a film stock.
3. **Hiding Cast Removal outside C-41** — we are C-41 only, so the slider is
   always live and always meaningful here.

**Checked, no counterpart (shared-bug-class audits):**
- **`658e16e`'s "Crosstalk was silently skipped on the GPU"** — their shader
  applied the unmix on the print branch only, so the strength slider did
  nothing on the engine the app actually renders with. The fix is a discipline
  note we should keep, because we now carry THREE kernel mirrors: they moved
  the unmix out from behind the branch and made the CPU **pack identity rows**
  when it doesn't apply, with the comment "branching here is exactly how this
  silently stopped working on the GPU once before." **We already work this
  way** and it is worth naming as a rule rather than a habit: our kernels have
  no mode branches at all — offsets fold into `finalBounds`, temp/tint/exposure
  fold into `cmyOffsets`, and every SwiftInvert-only control is identity at its
  default value rather than skipped by a flag. That is why the NegPy fixtures
  still pass through controls upstream has never heard of. Keep new work on the
  fold-to-identity side of that line; a `if (mode == …)` in one of three
  kernels is the exact bug they just paid for twice.
- **`5a6cc97` DNG BlackLevel/WhiteLevel/LinearizationTable ignored** — their
  hand-rolled 3-channel LinearRaw SubIFD read normalized by the container's
  bit-depth max instead of the file's own calibration tags, so a DNG whose real
  white point sits well below container max exported **~20× too dark**. This is
  the follow-up to `1956dd0`, the subject of the previous review. **We cannot
  have it**: we have no hand-rolled tag path to get wrong — `RawDecoder` goes
  through `libraw_unpack`/`dcraw_process`, which apply those tags themselves,
  and grepping `RawDecodeKit`/`negcli` for `BlackLevel`/`WhiteLevel`/
  `Linearization`/`SubIFD`/`tifffile`/`jxl` returns nothing. It is a cost of
  the bypass we declined to build, which is a second, unprompted argument for
  that decline (the reopening condition from 2026-08-09 is unchanged).
- **The Zone Density taper** (changelog, transfer path): shadow/highlight
  density offsets on a curve with no paper are "tapered to nothing at the
  bottom of the window so a shadow lift cannot walk the black point away — a
  print gets that bound from paper black; this curve has no paper." **Our tone
  controls are on the print path and already have that bound**: the toe
  softplus toward `d_max_eff` is the paper black they're substituting for. No
  change indicated; recorded so the mechanism isn't mistaken for a gap.

**Not applicable:** `8a7c256` retouch consolidation onto one fill (deletes
`retouch.wgsl` and the whole GPU retouch pass, moving every repair to a
CPU bake into the linear source ahead of the meters — retouch, which we don't
ship; note in passing that this also deleted their `test_gpu_stage_skip.py`),
`4391ad7` Transport Line scratch tracing from one click (retouch; their
measurement is nice — a 10% scratch reads 1.3σ per pixel but 15.8σ once
integrated along a fitted line, which is why no per-pixel gate can find one —
and they deliberately do NOT offer auto-detection because the strongest
full-length ridge on their sample frame was a horizon), `be5f5a4` in-app
updater + update dialog (`kernel/system/updater.py`, installer.nsi — their
Windows/Qt distribution; we ship `make app`), `5a6cc97`'s Linear Output half
(a recorded N/A since `6410002`).

**dump_fixtures.py:** compatible, verified symbol by symbol rather than
assumed, since this range touched two files it imports from. Every imported
name still exists (`resolve_crosstalk_matrix` was added-alongside, not
replaced, and the script doesn't import it anyway); `PipelineContext`'s two new
fields (`cam_xyz`, `camera_wb`) both default to `None`;
`PhotometricProcessor.__init__`'s new `process_config` is optional; and the
script constructs `PipelineContext(..., process_mode=ProcessMode.C41)`
explicitly, so `is_transparency_transfer` is False on every dumped case and the
`e6_normalize` default flip is unreachable. The `fixtures:` line does not move.

**Still open (carried over, unchanged):** colour ring-around (±4cc/2cc spec),
`91a1b78` tunable Auto Density/Grade targets (user-initiated only), the on-scan
Color Mixer band re-tune pass (ours). The two 2026-08-09 items (gesture-time
proxy tier, session-LRU bump) stay DECLINED, not open.

### 2026-08-09 (second) — through `1956dd0` (1 commit)

**No pipeline changes — but a shared LibRaw limitation worth having on
record, because a user WILL hit it and ask.** `1956dd0` is loader-only
(`infrastructure/loaders/`); the three pipeline dirs and both goldens have
an empty diff.

DxO PhotoLab/PureRAW and Lightroom Enhance write DNG 1.7 linear RGB files
whose LinearRaw SubIFD is JPEG-XL compressed. LibRaw's decoder for that
landed in 0.22 but is gated behind the Adobe DNG SDK, and rawpy's published
wheels don't link it, so upstream's loader crashed on an unhandled
`LibRawFileUnsupportedError`. Their fix probes with `unpack()` and, on
failure, decodes the SubIFD directly through tifffile/imagecodecs, replaying
the LinearizationTable, BlackLevel/WhiteLevel and DefaultCrop\* tags that
`postprocess()` would have applied — and deliberately applies NO
camera-to-XYZ matrix, because they keep every RAW source sensor-native.
That last part is our decode contract too.

**We share the gap, and it is not an inference.** We link Homebrew LibRaw
**0.22.1** — the version that has the feature in principle — but the dylib
exports `LibRaw::jxl_dng_load_raw_placeholder`, which is LibRaw's stub for a
build without the decoder, and it links no libjxl and no DNG SDK (its only
deps are jpeg-turbo, lcms2, libomp, libz, libc++). So a JPEG-XL LinearRaw
DNG cannot be unpacked here either.

**Our failure mode is already graceful**, which is the half of their bug we
don't have: `RawDecoder.decode` guards `libraw_unpack` and throws
`RawDecodeError.libraw("unpack", code:)` (RawDecoder.swift:58-59). An
unsupported DNG surfaces as an error, never a crash.

**Deliberately skipped (for now), with the reasoning recorded so it isn't
re-litigated silently:** porting the fallback means a DNG/TIFF SubIFD
parser plus a JXL decoder plus the tag replay, with no tifffile/imagecodecs
equivalent to lean on — disproportionate for a file class whose fit with
this pipeline is questionable anyway (a PureRAW linear DNG is already
demosaiced and partly balanced, where our chain assumes unity-WB
sensor-native data). Two cheaper mitigations if it ever comes up: a
DNG-specific error message telling the user to export a linear TIFF from
DxO instead of surfacing a bare LibRaw code, or a LibRaw build that links
the decoder. Reopening condition: a user actually wanting to invert
PureRAW-denoised camera scans, which is a plausible high-ISO workflow.

**dump_fixtures.py:** unaffected — nothing it imports was touched.

### 2026-08-09 — through `0030f57` (0.48.1 → 0.49.0, 7 commits)

**A golden MOVED, and it is the rare case where that doesn't reach us: it is
entirely the lab sharpen stage, which we don't ship.** All three pipeline
dirs (`features/exposure/`, `features/process/`, `kernel/image/`) have an
EMPTY diff across the range — `c20951a` only appeared in the path-filtered
log because that filter includes the golden files themselves. No constants
drift, no renames.

`c20951a` fixed an unsharp radius that was multiplied by the pipeline scale
factor, so a 1 px radius became a ~6 px blur on a 24 MP export and read as
contrast rather than sharpness; the radius is now in OUTPUT pixels on both
paths, and the L\* noise gate was rescaled by the same factor it had been
calibrated against. `test_scene_linear_relocation.py` moved at 4 of 6 sample
points because its render carries the default sharpen. **Our fixtures are
structurally immune**: `dump_fixtures.py` deliberately excludes the lab
stage, and says so in a comment — "LabConfig.sharpen would contaminate
goldens".

**Ported:** nothing yet (both items below are proposals).

**Deliberately skipped — DECLINED 2026-08-09 (user call), don't re-propose:**

1. **Render a preview-size proxy while a gesture is live** (from `8fc35b6`,
   the 0.49.0 perf range). Upstream's HQ drag frame went 381.5 ms → 7.6 ms
   of GPU work by rendering interactive frames against a preview-resolution
   proxy and re-rendering the full frame on release: "Releasing the slider
   re-renders the full frame exactly as before, so what you inspect at HQ is
   unchanged; only what you drag against is cheaper."

   **We have the same gap, verified in source.** `scheduleRender`
   (AppModel.swift:871) resolves the tier as
   `Self.renderTier(mode: hqMode, active: hqActive)` with no knowledge of
   whether a control is being dragged, and `HQMode.resolve` takes no drag
   parameter. So with HQ engaged every coalesced slider tick renders at the
   full/medium tier — order 0.5–1 s at 24 MP against 4.4 ms on the proxy.
   It is reachable by DEFAULT, because `.auto` engages past 2× zoom, which
   is exactly where you sit judging a crop.

   **Our severity is lower than theirs and the fix is smaller.** We do not
   have their two aggravating faults: `renderTask`/`renderPending` is a real
   one-in-flight latest-wins queue that awaits the actor (their coalescing
   had silently broken because submit returned before the frame drew), and
   `ImageSession` is an actor, so a long render never blocks the UI thread
   the way Qt's single-threaded widget path did. The symptom here is a
   picture that updates about once a second during an HQ drag, not a frozen
   handle.

   Maps onto machinery we already have, which is the argument for doing it:
   - `controlEditCount` already tracks live drags exactly (it holds the
     history commit until release) — the signal needs no new plumbing.
   - `scheduleRender` ALREADY special-cases a live gesture two lines above,
     substituting `fineRotation = 0` mid-straighten-drag. A tier downgrade
     is the same idiom in the same function.
   - The proxy is already warm in the tower, and the tier-swap stability
     work (`displayAspect` / `contentWindow` / the INTER_AREA
     `downsampled`) already guarantees the two tiers land on the same
     pixel — `DisplayAspectTests` pins it. The visual swap this needs is
     the one we already built and tested.
   - Release must trigger a re-render at the true tier (`setControlEditing`
     false → `scheduleRender`), or HQ silently stops being export-truth.

   No parity surface, no fixture re-dump, no settings field. Small effort;
   the care goes into making sure the release re-render can't be lost.

   **DECLINED 2026-08-09**: the app is fast enough in practice and this
   buys nothing on the path that is actually used. The gap is real but
   narrow — it needs HQ ENGAGED (an explicit `.on`, or `.auto` past 2×
   zoom) *and* a slider drag at the same time; every other interaction is
   already on the 4.4 ms proxy path, which is what day-to-day use feels
   like. Unlike upstream we never freeze: the actor keeps renders off the
   UI thread and the latest-wins queue gives real back-pressure, so the
   worst case is a picture that refreshes about once a second while you
   drag at high zoom. Reopening condition: someone actually complains that
   dragging a slider zoomed-in feels laggy.

2. **Raise the session LRU above 2** (from `f758b9a`, "keep eight rendered
   frames, not two"). Upstream's memo shared a limit with the full-res
   decode cache and so held exactly two — "the current one and the one
   navigated from" — and they concluded that stepping three frames along a
   roll and back re-rendered. **`AppModel.sessionLRULimit = 2`
   (AppModel.swift:343) is the same number reached by the same reasoning**,
   and CLAUDE.md describes it in almost their words.

   Not a blind 2 → 8: their retained object is a ~27 MB rendered frame,
   ours is a whole cache tower (decode + oriented + `Prepared` + textures),
   so the memory math is entirely different and should be measured before
   picking a number. The discipline that makes it safe is already ours —
   every tier is keyed, so a returning session re-validates and cannot
   serve stale pixels, and `releaseHQ()` already keeps the HQ tier on the
   visible frame only, which is exactly upstream's "HQ frames stay on the
   full-res budget".

   **DECLINED 2026-08-09**: current behaviour is fine as it stands. The
   measured trade is a ~200 ms preview decode + ~24 ms analysis per
   navigate-back miss (timed on IMG_0348: 0.19–0.26 s decode over three
   runs) against roughly 100–150 MB of RAM per extra retained session —
   ~73 MB preview buffer + ~19 MB proxy + ~25 MB source texture, before
   oriented copies. Upstream's 2 → 8 was cheap because their retained
   object is a ~27 MB rendered frame; ours is an order of magnitude
   bigger, so the same number would cost ~1 GB. Not worth a quarter-second
   on a gesture that already feels fine. Reopening condition: someone
   comparing frames back and forth along a roll finds the repaint
   intrusive — and then the number to try is 4, not 8, after measuring
   real RSS rather than trusting the arithmetic above.

**Checked, no counterpart (shared-bug-class audits):**
- `c20951a`'s second half — **export downscales now use INTER_AREA**,
  because OpenCV gives INTER_LANCZOS4 no prefilter on a shrink so their CPU
  path was silently plain bilinear. `Exporter.swift:119` calls
  `image.downsampled(maxLongEdge:)`, which has been a real separable
  area filter since the 2026-08-04 convergence entry. **We are ahead of
  upstream here**, and their fix confirms that convergence was right.
  The general lesson is one we already recorded for halation radii
  (2026-08-02): any constant in PIXELS must scale with render scale, or
  preview and export silently disagree.
- `0030f57` **batch export using a stale per-frame block** rather than the
  live panel (their per-frame export settings were "an accident of timing,
  never a user choice"). Ours cannot have this: `performExport(urls:options:)`
  takes ONE `ExportOptions` from the sheet and applies it to every URL;
  only the per-frame ADJUSTMENTS come from sidecars
  (`SidecarStore.load(for:) ?? DefaultProfile.settings`). Same shape they
  converged to. Consistent with the `c6dfb35`/`404c62a` audit (2026-08-02).

**Noted — a testing-discipline lesson worth keeping:** their `source_hash`
was never threaded into `process_to_texture`, so the stage-skipping resume
"existed, was maintained, and had never once run" through roughly eight perf
passes. Wall-clock budgets did not catch it because the frame still landed
inside any sane budget at preview size; what catches it is asserting
ENGAGEMENT directly — a toning-only change must not re-dispatch geometry.
Our nearest equivalent, `ImagePipelineSeamTests`, pins that a reused
`Prepared` EQUALS fresh analysis — correctness, not engagement. Nothing
asserts that `prepare` was actually skipped. Lower risk here (the caches are
keyed and a missed one lights the "Analyzing…" pill), but if we ever chase
render latency, assert the skip, not just the answer.

**Not applicable:** the rest of `8fc35b6`/`f758b9a` is Qt-specific
(soft proof moved into a cached 65³ display LUT — ours is ColorSync's job,
we hand off a tagged CGImage; GPU-texture vs host-array publishing;
thumbnail readback threading; a retouch dispatch we don't have),
`dfe0ac7` canvas overlay punching an alpha hole that erased the
GPU-painted frame (their rendercanvas/Qt compositing; note it presented as
white on macOS), `fe38f81` two-pass IR scanner detection/alignment
(capture + retouch), `6e09803` 0.49.0 changelog + VERSION.

**dump_fixtures.py:** compatible, and unusually well insulated this time —
`features/exposure/logic.py`, `normalization.py` and `analysis.py` have zero
lines in the range, so every imported signature is stable, and the script's
deliberate exclusion of the lab stage means the golden move cannot reach a
re-dump either. The `fixtures:` line does not move.

**Still open (carried over, unchanged):** colour ring-around (±4cc/2cc
spec), `91a1b78` tunable Auto Density/Grade targets (user-initiated only),
the on-scan Color Mixer band re-tune pass (ours).

### 2026-08-08 — through `958d24c` (0.48.0 → 0.48.1, 8 commits)

**One pipeline-relevant commit, and it lands entirely inside the dodge/burn
stack we don't ship.** `2e018f4` **Local Grade** is the only commit the
path filter over `features/exposure/`, `features/process/`, `kernel/image/`
and both characterization goldens returns. Goldens unmoved,
`EXPOSURE_CONSTANTS` untouched (empty diff), no renames,
`features/process/` and `kernel/image/` untouched. 0.48.1 was tagged in the
range.

**Ported:** nothing (nothing applicable).

**Not applicable — the local-adjustments stack** (`negpy.features.local`;
we ship no masks at all, which `NegPipeline.metal`'s header states as a
scope boundary: "no dodge/burn EV map"). Read anyway, because `2e018f4`
edits the print-curve kernel and its WGSL mirror — the two files we do
mirror:
- `2e018f4` **Local Grade per mask** (ISO-R points off the frame's Grade).
  Two fragments worth recording even though the feature isn't ours:
  1. **The mechanism is our recorded per-layer-grade-trims skip, applied
     per pixel.** `local_grade_factor_map` is literally `_grade_trim_mult`'s
     ratio `R/(R+ΔR)` clamped to the ISO-R ladder, and in the kernel it
     multiplies the STRAIGHT-LINE SLOPE ONLY —
     `v = k·g·(x−x₀) + c·x²` — so the rotation is about the channel pivot
     and a grade-only change doesn't move its own midtone, while the
     cast-removal curvature `c` stays global. If we ever revisit per-channel
     grade trims, that pivot-rotation form (and leaving curvature alone) is
     the reference.
  2. **The stops sign convention flipped, and it cancels.**
     `local_ev_scale` went `−log10(2)/range` → `+log10(2)/range` at the same
     time the mask slider went brightness-signed (positive = dodge) to
     exposure-signed (positive = burn), matching `vignette_stops`; old saves
     migrate on the nested key. Net operator unchanged. **We are unaffected
     numerically**: `deriveRenderParams` folds `exposureStops` in as
     `stops × (−K.log10Two/range)` with positive = brighter, which is
     upstream's new operator with the slider negated — the same algebra, and
     the direction our whole UI convention already requires ("right =
     brighter", the recorded test-strip divergence). Only the code comment
     citing "the local_ev_scale domain" is now sign-stale against upstream;
     a comment refresh is optional and was NOT made during this review.
  Also noted: their GPU consumes the CPU-rasterised map (EV in `.r`, grade
  factor in `.g` of one texture — no extra bind slot), so shapes and local
  grade have no shader parity surface at all; and one `rasterise` serves
  render, canvas tint and printing notes so "none of the three can describe
  a different shape" — the same don't-fork-the-ruler discipline our
  `Densitometry` ruler already follows across probe/zone strip/zone overlay.
- `958d24c` **Oval + card-edge (gradient) masks, per-mask invert** — the
  vertices stay the universal store and `shape` says how to read them, so
  geometry mapping is shape-blind. Their oval is the unit circle under a
  possibly non-orthogonal affine frame from 3 points, which handles tilt,
  shear and raw aspect ratio for free; the gradient's handle spacing IS the
  softness (feather doesn't apply).
- `dbffc90` **Printing Notes** — a marked-up work print previewed on canvas
  (⇧N) and exported as `<stem>_notes.jpg`: burns hatched, dodges open, masks
  badged in stops, print recipe below. Their canvas/export layer, and it
  annotates the masks we don't have.

**Not applicable (rest of the range):** `239b5b6` JPEG XL input loader +
lossless output for Linear and Flat exports (Linear Output is a recorded
N/A since `6410002`; our export is JPEG/TIFF — their own PIPELINE.md now
documents at length that JXL has no legal untagged state and that
`imagecodecs` can't supply real primaries or an ICC profile, which is an
argument against JXL for a linear dump, not for it), `8dc5e22` Immersive
Canvas toggle and `c04b183` Sticky Zoom across image switches (their Qt
canvas chrome — Sticky Zoom is a mildly interesting UX idea for our
DetailView zoom `@State`, but it would interact with the HQ tier threshold
and nobody has asked), `ee9e402` slider tooltips firing on the label as
well as the groove, `e5ed758` 0.48.1 release chore.

**dump_fixtures.py:** compatible. It imports neither `local_ev_scale` nor
anything under `features/local/`; the only signature it touches that moved
is `apply_characteristic_curve`, whose new `grade_map` parameter defaults to
`None` (and gates `use_grade`), so every existing call is semantics-stable.

**Still open (carried over, unchanged):** colour ring-around (±4cc/2cc
spec), `91a1b78` tunable Auto Density/Grade targets (user-initiated only),
the on-scan Color Mixer band re-tune pass (ours — and per the last entry,
best done after living with Hue Trim).

### 2026-08-06 (second) — PORTED Hue Trim (`7a07f5c`)

Proposed 2026-08-05, user approved. Landed as `hueTrim` (degrees, ±30,
**0 = off**) through the full control checklist: settings field + decoder
line + HistoryLabels (tripwire 51 → 52 — note the count recorded in
CLAUDE.md had drifted at 49 and is corrected), derive passthrough
(degrees → radians, clamped to the slider range), both kernels, the
CurveUniforms ex-pad slot (stride unchanged at 272), `colorPopActive`, a
slider, and tests.

The port was close to mechanical because their working Lab is Adobe RGB /
D65 — byte-for-byte the matrices we have carried since b3490eb. Where
their GPU has to inline `rgb_to_lab`/`lab_to_rgb` (WGSL has no includes),
ours needed **no new conversions at all**: `colorPop` already does that
round trip, so the rotation is one block at the top of it. It runs FIRST,
ahead of the mixer/vibrance/saturation, which matches upstream (theirs
rides the exposure pass ahead of their Lab stage) and is the right order —
a capture correction should precede the look controls.

**Divergence recorded:** upstream makes it sticky across frames since the
light is a rig property. Ours is an ordinary sidecar-backed setting; bake
it into a `SettingsProfile` and it carries to every fresh frame, which is
the same behaviour through the mechanism we already have.

No fixture re-dump — identity at 0, so every NegPy fixture is untouched;
`negcli meter` is unchanged across four CR3s and the rendered TIFF hashes
identically. 286 tests green; prepare 16.8 ms and slider 4.3 ms/frame,
unchanged.

`HueTrimTests` pins the invariants that make it safe to sit in front of
everything else: identity at 0, neutrals stay neutral at every angle, L*
and chroma preserved, the measured rotation equals the angle asked,
opposite angles cancel, and — the distinction upstream's whole measurement
rests on — the effect GROWS with chroma, where a cast would not.
`GPUParityTests.hueTrimTightParity` runs it alone at the tight gate on a
deliberately chromatic frame, and asserts the rotation actually moves the
frame BEFORE comparing implementations (upstream's own lesson: a dead GPU
mirror once passed a loose test).

One tolerance worth recording, since it bit twice while writing the tests:
"a rotation preserves chroma exactly" is NOT assertable at 1e-6 on a
neutral. The working matrix's rowsums differ from the D65 reference white
in the 7th digit, so `rgbToLab` already reports a few times 1e-6 of chroma
for a perfect grey, and the rgb→Lab→rgb round trip perturbs it by as much
again. Swept 994 greys × 121 angles the worst residual chroma is 1.05e-5
and the worst channel spread 1.3e-7 — roughly 48,000× below a visible
difference. The test bounds absolute residual instead.

**Still open (carried over):** colour ring-around (±4cc/2cc spec),
`91a1b78` tunable Auto Density/Grade targets (user-initiated only), the
on-scan Color Mixer band re-tune pass (ours — and now best done AFTER
living with Hue Trim, since some of what the mixer compensates for may be
light rotation this fixes at the source).

### 2026-08-06 — through `7eb6837` (0.48.0 release, 3 commits)

**Kernel status: a genuine null.** The path-filtered log over
`features/exposure/`, `features/process/`, `kernel/image/` and both
characterization goldens is EMPTY; no renames. 0.48.0 was tagged in this
range, and its headline pipeline items — Hue Trim and the Calibration
section — are the PREVIOUS range, already reviewed (2026-08-05). Everything
new here is the scanner/Linear-Output half.

**Ported:** nothing (nothing required).

**Not applicable:**
- `53b36ae` **Scanner formats as Linear Output sources** — Coolscan NEF
  (TIFF-structured, largest RGB SubIFD), Flextight FFF including SGI LogLuv,
  Noritsu headerless BGR16, and generic TIFF with an input-gamma selector;
  plus IR dust removal baked into Linear Output. All of it lands in
  `infrastructure/loaders/` and `services/export/`, and touches zero
  exposure/kernel lines. We are camera-RAW only and ship no Linear Output
  (recorded N/A since `6410002`). **One fragment noted in passing:** their
  LogLuv decoder normalizes per channel at the 0.2/99.8th percentile,
  because LogLuv is HDR and routinely exceeds 1.0 so a bare `clip(0,1)` was
  truncating data — and the same step happens to correct Flextight CCD
  per-channel black/gain offset. Irrelevant while we read camera RAW only.
- `01232ed` / `7eb6837` release chore + changelog for 0.48.0.

**Decode contract re-checked** (it is a pinned reference for us, so the
PIPELINE.md Camera RAW line moving warranted a look): wording only —
"matching the MakeTiff/ColorPerfect convention" became "following the
`RAW-WB` XMP convention used by external linear-workflow tools". Every
rawpy parameter is unchanged.

**dump_fixtures.py:** unaffected — nothing it imports was touched.

**Still open (carried over):** **`7a07f5c` Hue Trim** (proposed 2026-08-05,
not yet ported — the live candidate), colour ring-around (±4cc/2cc spec),
`91a1b78` tunable Auto Density/Grade targets (user-initiated only), the
on-scan Color Mixer band re-tune pass (ours).

### 2026-08-05 — through `7a07f5c` (0.47.0, 4 commits)

**One pipeline commit, and it is a real port candidate for us specifically:
`7a07f5c` Hue Trim.** No golden move (`test_scene_linear_relocation.py` /
`test_characteristic_curve.py` untouched), no constants drift, no change to
`normalization.py` or `logic.py`.

**To port (proposed, not yet implemented):**

1. **`7a07f5c` — Hue Trim** (`process.hue_trim`, degrees, ±30, **0 = off**).
   A rotation of `a*`/`b*` about the neutral axis in the working CIELAB,
   applied to the scene-linear print — after the H&D curve, before the
   working OETF. `L*` untouched; the origin is a fixed point, so neutrals
   cannot move and it cannot fight the cast removal.

   The evidence is the good part: one negative scanned twice (narrowband-ish
   panel vs broadband white LED, same body, same frame), `|ΔH|` held within
   16–21° across CIELAB chroma 4 to 60+. A cast would have SHRUNK with
   chroma (a fixed a*/b* offset barely turns a saturated colour); a channel
   mix would have GROWN. Flat in chroma is the signature of a rotation, so
   the correction is one — and white balance cannot fix it, because there is
   no grey that is wrong.

   **Unusually applicable to us.** We are camera-RAW only, i.e. everyone
   here is camera-scanning on some light source, which is exactly the
   variable this corrects; cheap LED panels are routinely narrowband-ish.
   This is arguably more relevant to SwiftInvert's scope than to the scanner
   half of NegPy's.

   Maps cleanly, and the port is close to mechanical:
   - Their Lab is **Adobe RGB primaries, D65** — byte-for-byte the same
     matrices we already carry in `LabColor.swift` and `NegPipeline.metal`
     since the b3490eb port (checked: 0.5767309…, 2.0413690…, D65 white,
     0.008856 eps all identical).
   - Their GPU inlines rgb→Lab→rgb because WGSL has no includes; **we need
     no new conversions at all** — `colorPop` already does that round trip,
     so the rotation drops in at the TOP of the existing Lab block, before
     the colour mixer, which is also the right stage order (upstream applies
     it before the look controls).
   - `colorPopActive` must gain the term (academic in practice —
     `skinProtection` 0.5 already dispatches the pass every frame).
   - Full control checklist: settings field + decoder line + HistoryLabels
     (tripwire 49 → 50), derive passthrough (degrees → radians), both
     kernels, LabUniforms + `LayoutTests`, a slider, tests.
   - **No fixture re-dump** — identity at 0, like every SwiftInvert-only
     control.

   **Adaptation to decide:** upstream makes it *sticky across files* (a rig
   property). We already have that mechanism generically — bake it into a
   `SettingsProfile` and it carries to every fresh frame — so a normal
   sidecar-backed setting plus the default profile achieves the same thing
   without a new stickiness concept.

   Effort: small-to-moderate (~half day), almost all of it checklist and
   tests rather than math.

**Deliberately skipped:** the other half of `7a07f5c` reorganizes the
crosstalk UI (section renamed "Calibration", profiles renamed to
`kodak_*`, "Default" → "Generic C41", a new provenance `type` key grouping
the dropdown, all 16 bundled matrices marked "(approx)"). Crosstalk is a
recorded skip. **But log the reframing**, because it bears on that skip and
on the `negcli chart-solve` idea: upstream now says a crosstalk matrix is
properly a property of a whole SCANNING SETUP, not of a film stock — the
light's spectrum and the sensor's CFA mix channels too, and in log density
all three arrive as the same linear operator, so one 3×3 absorbs them
jointly. Their bundled matrices are datasheet-derived from dyes alone and
now say so, telling the whole story only for a true RGB scan or a
calibrated trichrome rig. That is an argument FOR a measured/solved matrix
over a shipped one, if we ever revisit.

**Not applicable:** `183553a` Linear Output batch config leak + XMP
metadata (their export path; Linear Output is a recorded N/A), `d93afd2`
SANE scan-exposure-time slider (capture), `8823afc` half-frame rectangle
editor (their scan-crop UI; we ship no rectangle editor).

**dump_fixtures.py:** compatible. `normalization.py`/`logic.py`/exposure
`models.py` are untouched; the only imported symbol whose file moved is
`ProcessConfig` (`features/process/models.py`), and both changes there are
benign — an added defaulted field (`hue_trim = 0.0`) and a renamed
`crosstalk_profile` default, which does not affect the render (a None
matrix still falls back to the built-in).

**Still open (carried over):** colour ring-around (±4cc/2cc spec),
`91a1b78` tunable Auto Density/Grade targets (user-initiated only), the
on-scan Color Mixer band re-tune pass (ours).

### 2026-08-04 (second) — convergence, not a review

Not an upstream range: closing an approximation of ours that had drifted
from the reference. `RGBImage.downsampled`'s docstring called it a
"stand-in for NegPy's cv2 INTER_AREA preview resize"
(`services/rendering/image_processor.py`), but it used integer box
boundaries (`Int(ox·s)`) rather than INTER_AREA's fractional weights. At
the preview's 1.96 ratio that gives boxes of 1 or 2 source pixels whose
centres drift up to 0.97 source px from the ideal grid, sliding smoothly
across the frame — a low-frequency geometric warp, which surfaced as local
misalignment between the proxy and the HQ preview at high zoom. Now a
separable fractional-weight area filter, matching INTER_AREA's kernel.

**This moved the default look slightly** — the proxy feeds analysis, so
the meters read a marginally different grid: probes shift ~0.05 D and one
reference frame crossed a zone boundary. Accepted deliberately as
convergence with the reference. Fixtures are synthetic and never touch
the decode/downsample path, so parity is unaffected (275 tests green).

### 2026-08-04 — through `bd81b85` (0.47.0, 4 commits)

**Kernel status: a genuine null.** The path-filtered log over
`features/exposure/`, `features/process/`, `kernel/image/` and both
characterization goldens is EMPTY; no renames in the range
(`--diff-filter=R` empty); VERSION unmoved at 0.47.0 and CHANGELOG.md
untouched (the range is post-release 0.47.0 work). No fixture re-dump, no
constants drift. `dump_fixtures.py` unaffected — exposure
`normalization.py`/`logic.py`/`models.py`/`analysis.py` have zero lines in
the diff, so every imported signature is stable at the new tip.

**Ported:** nothing (nothing required).

PIPELINE.md moved in two places, both outside our pipeline: the Linear
Output section documents the new composite paths and correction toggles
(below), and the OpenICE section gains a §7 "Grain" step for `4baae2e`.

**Not applicable (each diff checked):**
- `b80eb4f` **Linear Output: composite support, correction toggles, no-op
  fixes** — read in full because the last review's lesson was that a
  decode-contract change (`2a6cb22`) hid in `services/`. It doesn't repeat:
  grepping the `linear_output.py` diff for every rawpy postprocess
  parameter (`adjust_maximum_thr`, `output_color`, `gamma`,
  `no_auto_bright`, `user_wb`/`user_mul`, `half_size`, `output_bps`,
  `user_flip`, `user_qual`) returns nothing — the decode contract we pin is
  untouched. What it adds is their capture stack: RGB-scan triplet merging
  and multi-part stitch assembly into one linear TIFF, plus three
  default-OFF toggles (apply WB / flatfield / sensor correction) that bake
  corrections into the raw dump, forced on per-part for stitches so
  vignetting and crosstalk don't seam. All of it sits on features we don't
  ship (RGB-triplet capture, stitching, flatfield, sensor crosstalk), and
  the Linear Output feature itself is already a recorded N/A (`6410002`,
  2026-08-02 — `negcli decode` serves the debugging role). Their raw-dump
  philosophy is worth noting as convergent with ours: the default output is
  unmodified sensor data.
- `4baae2e` **OpenICE synthetic grain** — restores ICE's §8 dither inside
  the IR-dust reconstruction (zero-mean density noise, α = 0.015/0.015/
  0.025, parabolic envelope over the 1st–99th density percentiles, no grain
  unless the dithered value stays in-band), because a confidence-weighted
  reconstruction returns smoother than the film around it and the finest
  detail band can't fix that — it restores a pixel's grain FROM that pixel,
  and a pixel under solid dust has none left. Retouch/IR stage we don't
  ship. **Noted for any future synthesis-class effect:** they draw from a
  hash of the pixel's absolute coordinate rather than ICE's frame-global
  LCG — the LCG is the algorithm's only serial dependency and would make a
  banded pass differ from a whole-frame one. That is the same
  render-scale-invariance discipline as the `14bc87b` halation note
  (2026-08-02).
- `07a7f18` session panel stays stacked when its sections collapse (Qt
  layout), `bd81b85` clickable empty state + canvas-tracked centring +
  foldable branding header (their Qt shell chrome).

**Still open (carried over, unchanged):** colour ring-around (±4cc/2cc
spec), `91a1b78` tunable Auto Density/Grade targets (user-initiated only),
the on-scan Color Mixer band re-tune pass (ours).

### 2026-08-03 — through `41ae8c5` (0.47.0, 5 commits) + PORTS of the 2026-08-02 items

**Addendum review before porting yesterday's items — one overnight commit
changed the spec of the very feature being ported:** `9dff124` extends zone
placement with a THIRD pin that solves one knee control (their Shadows
Grade / Highlights Grade / Snap), picked by MEASURED purchase (probe each
candidate against the forward model, keep the one that moves the pin most
— explicitly not a zone-number rule) with an alternating outer/mid solve
and an honest achieved-vs-asked readout. Also in the range, all N/A:
`86ba21f` first-frame selection order, `0713263` camera-scanning crash
fixes, `41d3aee` work prints (named per-frame versions surviving undo —
their session/DB layer; noted as a UX idea only), `41ae8c5` changelog.

**PORTED 2026-08-03 (both 2026-08-02 items, user approved; zone placement
at the new `9dff124` spec):**

1. **`2a6cb22` decode white-level pin** — `adjust_maximum_thr = 0.0` in
   `RawDecoder` (the one decode function serves preview + export).
   Verified: the reference Canon frames don't trip the 75% threshold
   (negcli meter byte-identical on IMG_0365/0348, probe values match the
   127bcd7 A/B record); frames with base near clipping will shift — the
   fix working. CLAUDE.md decode contract updated: the rawpy
   byte-identical claim now requires `adjust_maximum_thr=0` passed
   explicitly on the rawpy side.

2. **`5a095f3` + `9dff124` zone placement** — `ZonePlacement` in
   NegativeKit (Pin/Solution/1-2-3-pin solve, bisection at half slider
   precision, autos off, clamp+round with achieved recomputed, honest
   clamped flag), `Densitometry.encoded(ofZone:)` exact ruler inverse,
   `ImageSession.sampleZonePin/predictedZone/solveZonePlacement`,
   zone-strip cell arming + draggable pins + solved-look canvas preview +
   Apply-as-one-history-entry ("Zone placement"), Escape/edit/tool/
   baseline/HQ/navigation clears (test-strip state rules). Forward model
   runs the REAL kernel collapsed to green (1-px ReferenceCurve — can't
   fork from the render); deliberate inclusions upstream's model omits:
   the green cmyOffset (carries exposureStops, our control) and the green
   levels remap. **Documented adaptations:**
   - Knee candidates are our tone-control knees (`shadowContrast`,
     `highlightContrast`) — upstream solves Split Grade/Snap, which sit
     inside our recorded Split Grade/Zone Density skip; the
     measured-purchase picker is control-agnostic by design, so it
     transfers. No midtone candidate (our only midtone contrast is
     global `overallContrast`, collinear with the grade the 2-pin solve
     owns).
   - **The knee solve NESTS instead of alternating.** Upstream iterates
     (place extremes, place middle, ×3 passes) to avoid multiplying the
     2-pin cost — sound for their zone-centred grade windows, whose
     centres sit outside the span between typical pins. Our knee anchors
     (1.40/0.30) sit BETWEEN typical pin positions, the raw and
     post-re-solve effects can have opposite signs, and the alternation
     measurably diverged to the slider corners (grade 50, knee −3, every
     pin off). The composite-residual bisection (2-pin solve nested in
     every evaluation, picker measures composite purchase too) converges
     unconditionally, and monotonicity makes the clamped endpoint the
     argmax — never worse than no knee. Affordable because our forward
     model is µs-scale where theirs is ms-scale Python.
   No settings field (density/grade/knees are existing fields — tripwire
   counts untouched), no parity surface, NO fixture re-dump. 229 tests
   green (ZonePlacementTests: ruler inverse, model monotonicity +
   stops/levels visibility, 1/2/3-pin contracts, order-invariance,
   degenerate/clamp honesty, unrelated-settings passthrough); bench
   unchanged (prepare 126 ms, 7.7 ms/frame); app renders headlessly.
   Human-verify the pointer paths (pin drag, strip arming) per the GUI
   verification rule.

**Still open (carried over):** colour ring-around (±4cc/2cc spec),
`91a1b78` tunable Auto Density/Grade targets (user-initiated only), the
on-scan Color Mixer band re-tune pass (ours).

### 2026-08-02 — through `82f7c62` (0.45.0 → 0.47.0-dev, 18 commits)

**Kernel status: no golden moves, no constants drift — but one decode-contract
fix we share verbatim, and one substantial exposure feature.** The 0.46.0
release landed in this range; the path-filtered pipeline log holds only
`5a095f3` (zone placement) and `a06381c` (content hashing, N/A); `2a6cb22`
hides in `services/` because their decode params live there.

**To port (proposed; BOTH PORTED 2026-08-03 — see the entry above):**

1. **`2a6cb22` — pin RAW decode scale to the camera white level.** LibRaw's
   default `adjust_maximum_thr=0.75` swaps the u16 scaling reference from the
   camera's white level to the frame's own brightest pixel once that pixel
   passes 75% of white level — routine when the film base is exposed near
   clipping, i.e. exactly negative scans. Each such frame then decodes on its
   own scale while normalization bounds, metering and A/B assume one shared
   scale. Upstream now passes `adjust_maximum_thr=0.0` on EVERY decode
   (pipeline/preview/detection/thumbnails), documented in PIPELINE.md as part
   of the decode contract. **We share the bug**: `RawDecoder.swift` never
   sets `params.adjust_maximum_thr`, so we inherit the 0.75 default on both
   the preview and export paths. Port = one line in `configure` (both paths
   share it) + a decode test; NO fixture re-dump (fixtures are synthetic
   dumps — decode is outside the parity surface). Bookkeeping: CLAUDE.md's
   decode-contract list gains the param, and the "verified byte-identical
   against rawpy" claim must be restated (the rawpy comparison now needs
   `adjust_maximum_thr=0` passed explicitly). Expect affected frames (base
   near clipping) to shift slightly in metering — that's the fix working.
   Trivial effort, highest priority: it's a per-roll-consistency correctness
   fix in the pinned reference contract, and upstream made it the
   unconditional default, not a toggle.

2. **`5a095f3` — Zone placement** (0.46.0's headliner): click a zone-strip
   cell, click that spot on the photo, and Print Density (1 pin) or Density +
   ISO-R Grade (2 pins) solve so the tone prints on that zone; pins drag
   live; Apply is one undo step. The math (`placement.py`, ~130 lines) is
   pure forward-model inversion by bisection: `predicted_zone` runs the
   ACHROMATIC green-reference curve (`curve_params_from_metrics` →
   `print_curve` → output encode → `zone_of_encoded`) on the pin's frozen
   normalized-log luma; 1 pin bisects density (autos → off), 2 pins do an
   outer grade bisection against the light pin with an inner density solve
   pinning the dark pin (residual decreasing in R), both at half the
   sliders' precision, clamped+rounded with achieved zones recomputed.
   `analysis.py` gains `encoded_of_zone` (exact ruler inverse, kept beside
   `zone_of_encoded` "so the ruler can't fork"). Maps cleanly: we already
   ship the ruler (`Densitometry.zone(ofEncoded:)`), the zone strip, the
   probe's frozen samples, and a µs derive — the solver is a small
   NegativeKit type calling `deriveRenderParams` + the green
   `ReferenceCurve` per iteration (closed-form testable; their
   test_zone_placement.py is 547 lines of spec to mirror). Bulk of the work
   is UI: pin overlay + drag + zone-strip arming + Apply-as-one-history-
   entry. No parity surface, no fixture re-dump. Moderate effort (~1 day).

**Checked, no counterpart (shared-bug-class audits):**
- `c6dfb35` + `404c62a` bulk ops / Export Sidecars stamping the active
  frame's edit onto unedited frames — their config-hydration bug. Ours
  resolves per-file at use (`SidecarStore.load(for:) ?? DefaultProfile
  .settings`, live settings only for the open URL) in batch export; Copy
  Adjustments is explicit-only. Verified in `performExport`.
- `4ff2855` test-strip/ring-around preview corrupted by alpha before RGB888
  display — read because we ported the test strip; it's a Qt display-
  conversion bug with no counterpart (our display path is rgba8unorm
  end-to-end, no RGB888 repack).
- `14bc87b` glow/halation: stage we don't ship. Noted for any future
  halation-class effect: GPU blur radii must scale with render scale
  (their fixed 15/25 px collapsed the bloom 4–8× at export), and the
  highlight mask is now LINEAR (the CPU's `**2` was dropped to converge on
  the shipped GPU look).

**Not applicable:** `6956add` flat-field profile store, `0353bf2` RGB-scan
triplet stitching (capture), `6410002` Linear Output export (decoded-source
TIFF dump — `negcli decode` already serves the debugging role here),
`69befb9` OpenICE IR removal (retouch), `92d6913`/`71069f1` export filename
templates + EXIF ImageDescription fields, `a06381c` content-hash interior
sampling (their edit-store identity; our sidecars key off the filename),
`4edce33` IR dust detection scale, `82f7c62` library folders/metadata
search, `7292fcd`/`cd8362a`/`e9c4e8a` changelog/readme churn.

**dump_fixtures.py:** compatible — exposure `normalization.py`/`logic.py`
untouched in the range; the `analysis.py` change is additive
(`encoded_of_zone`).

**Still open (carried over):** colour ring-around (±4cc/2cc spec),
`91a1b78` tunable Auto Density/Grade targets (user-initiated only), the
on-scan Color Mixer band re-tune pass (ours).

### 2026-07-31 — through `a09cc46` (0.45.0-dev, 10 commits)

**Headline: upstream REDESIGNED the skin protection we ported yesterday.**
`bfcd90a` + `fb94aed` replace 1b900ab's in-boost damping (what our
2026-07-30 port ships) with a separate operator, after measuring the
interim design's flaws: the hue-only 52±25° mask couldn't tell a face from
a red car (~40° sits in-band), it was inert at Chroma 1.0 (a face arriving
sunburnt from the print curve got nothing), and the first replacement's
wide chroma window reined sunsets at weight 0.98. End state at tip:

- The **gamut-aware knee stays exactly as we ported it** — but with the
  skin term REMOVED from inside it (pure gamut math now).
- **Skin Protection** is a user-facing Lab control (`skin_protection`,
  default 0.5): `skin_chroma_rein`, a one-directional soft chroma ceiling
  (ceiling = 22/strength, softplus knee from 0.6×ceiling, scale blended by
  mask weight; a*/b* together so hue and L* never move) applied AFTER the
  saturation scale and independent of it — it also catches skin that
  arrived over-chromatic from the curve, and reduce-only means
  saturation 0 still reaches true grey and it can never overshoot the
  gamut the knee fitted.
- The mask is the axis-aligned CIELAB skin locus: hue Gaussian 52° σ20
  (was ±25), one-sided chroma window full ≤35 zero ≥60 via smoothstep
  (skin C*≈12–40; pure red ≈104 drops out entirely), lightness rolloff
  below L*15/above 95, chroma-gate 2 for hue stability. Measured weights
  after the tightening: skin 0.95–1.00 vs sunset 0.04, terracotta 0.18,
  brick 0.25, autumn leaf/rust 0.00.
- Their goldens provably unmoved (no skin-band pixels in the relocation
  frame); their WGSL parity test was strengthened after discovering a dead
  GPU mirror could pass the old one.

**PORTED 2026-07-31 (same day, user approved).** Landed exactly as
proposed below: the boost is pure gamut math again, `skinProtection`
(default 0.5, keyless sidecars adopt it like upstream) + `skinChromaRein`
both sides, fixture parity pinned at 0 in both harnesses, lab_color
re-dumped from `a09cc46` (yesterday's b8c596c dump still carried the
interim in-boost damping — the two boost cases moved again and our pure
implementation matches at 1e-4), and the dead-mirror lesson became a
parity test whose PRECONDITION asserts the rein moves a deliberately
warmed frame (mean > 2e-3) before checking GPU==CPU. Known cost, accepted:
default-on rein dispatches colorPop every frame — bench 4.6 → ~7.2 ms
preview (still ~140 fps). 185 tests green.

**Original proposal, for the record:** remove the skin term from `gamutAwareBoost` (keep the
knee), add `skinProtection` (default 0.5) + `skinChromaRein` in colorPop
after saturation, both sides; pin `skinProtection = 0` in the GPU fixture
parity settings (fixtures predate the control — same pattern as
preSaturation); colorPop activation gains the new term (default-on means
the pass always dispatches — bench it); dedicated coverage that actually
exercises the rein (their dead-mirror lesson). No fixture re-dump
(`apply_saturation`'s new kwarg defaults to 0.0, so the dump script's
existing calls are semantics-stable).

**Not applicable:** `a09cc46` Linear RAW defaults OFF + `b6d9b8e` scanning
setup wizard — their onboarding defaults for uncalibrated setups (the
wizard turns it back on); our linear rawpy-style decode is the pinned
reference contract, unchanged upstream. `e62aacb` filmstrip scrolling,
`7528f9c` favourites panel, `df91cb4` Filtration reset icon, `d1a3b61` IR
dust fixes, `076e676` test-junk cleanup, `16b8f6d`-style prefetch order
(`16b8fd6` — their prefetch; we retain sessions, no prefetch).

**Still open (carried over):** colour ring-around (±4cc/2cc spec),
`91a1b78` tunable targets (user-initiated only), the mixer band re-tune.

### 2026-07-30 — through `b8c596c` (0.45.0 tail, 10 commits)

**Kernel status: no golden moves, no constants drift — but three port
candidates, one of which is a bug-class we share.**

**PORTED 2026-07-30 (same day, user approved) — items 1 and 2** (see the
commits for full detail): **Separation Damping** landed with the exact gain
formula, ref spread 0.35 as a new constants sync point, and the
population-reversal contract tested both ways; **gamut-aware Chroma**
landed in LabColor + colorPop with the skin band, bisection and knee —
the dump script's lab_color case was repaired at the same time (frozen
verbatim apply_vibrance for our SwiftInvert-maintained control, upstream
apply_saturation for the rest) and lab_color partially re-dumped from
`b8c596c`: ONLY the two boost cases changed, desat/in-gamut/vibrance
byte-identical, and our Double implementation matches upstream's float32
fixtures at the 1e-4 gate. A dedicated tight CPU/GPU test (mean < 1e-3,
max < 0.02) guards the path their transposed-matrix bug hid in.

**Original proposals, for the record:**
1. **Gamut-aware Chroma + skin-tone protection** (`1b900ab`) — a flat Lab
   a*/b* scale preserves hue, but overshooting pixels get hard-clamped PER
   RGB CHANNEL afterward, which shifts the visible hue (their measurement:
   a 1.2× push clips 6.3% of a realistic population, mean hue error 8.65°
   on the clipped set). Fix: bisect each pixel's true in-gamut headroom (10
   iterations) and soft-knee toward it; in-gamut pixels stay byte-identical;
   pulls (<1.0) unchanged; skin-tone hues get a gentler push. **Our colorPop
   has the same hard clamp** (`clamp(lab_to_rgb(lab), 0, 1)` after
   saturation/vibrance/mixer) — the bug class applies verbatim. Port =
   LabColor.swift + colorPop MSL both sides + skin-band constants; CAUTION:
   changes `apply_saturation` semantics for clipped pixels, so the
   `lab_color` fixture must be re-dumped (requires the dump-script
   adaptation first) or its clipped cases recalibrated. Medium effort.
   Their war story worth keeping: a transposed RGB↔XYZ matrix inside the
   bisection masqueraded as float precision noise — verify our port with
   their tight CPU/GPU gate (~1e-4), not just the 0.04 parity bar.
2. **Separation Damping** (`d86a5aa`, `separation_damping` τ 0…1, default
   0 = byte-exact off): makes the dye-separation k chroma-selective per
   pixel — `h(c) = (c0−c)/(c0+c)`, `k_eff = k^((1−τ)+τ·h(c))` on the RMS
   dye-density spread above base — so a push lands on muted colour and
   REVERSES on already-extreme colour (gain crosses 1.0; no flat k
   reproduces it: their Ektar sample reads ×1.37 on the least-coloured
   decile, ×0.89 on the most). The per-pixel-targeting idea we ported and
   retired, reborn as a modifier of the surviving control: maps directly
   onto our printSaturation (identity dye matrix → k_eff around the
   achromatic mean). Only meaningful when printSaturation ≠ 1 (grey out at
   1.0, like upstream). Kernel both sides + one constant (their c0
   reference spread); no re-dump. Medium.
3. **Proof-ladder rotation** (`a2455ab`) — **PORTED 2026-07-30** (same
   day): TestStrip gained orientation 0–3 (logical grid rotated under the
   fixed display grid; bijection + axis-purity tested per orientation),
   rotate buttons/⌘[/⌘] intercepted while a strip is up or building,
   labels/preview badge/picks orientation-aware, orientation sticky for
   the session. Documented divergence: upstream assembles all four
   orientations from cached tiles; we re-render from the warm tower
   (~one strip build) instead of retaining 25 preview-size tiles.

**Noted:** the ring-around spec at the tip is now ±4cc in 2cc steps (was
±2cc/1cc when first proposed) — still carried as an open candidate.

**Not applicable:** `d982c4b` zoom-button alignment (Qt), `b8c596c`
tethered-capture deletion for bodies without a capture target (camera
scanning), `4aa1001`/`540cd9a`/`65c7667`/`9714664`/`1869f2e`
changelog/screenshot/docs churn.

**Still open (carried over):** colour ring-around (above), `91a1b78`
tunable Auto Density/Grade targets (user-initiated only), the on-scan
Color Mixer band re-tune pass (ours).

### 2026-07-29 (second) — through `723e2c5` (0.45.0 release, 12 commits)

**Headline: upstream DELETED the per-pixel Dye Separation — the control we
ported this same morning — after measuring it redundant with Print
Saturation.** The three-commit saga (`3ca2229` → `1d02071` → `3fb5ca8`)
ends with only the matrix-slot density saturation surviving, renamed
**dye_separation** (default 1.0, slider 0.25–1.75, per-channel trims). The
range's golden-file touch is comment-only (default annotations; no numeric
move — both retired states were identity).

The saga, for the record (each step supersedes the last; only the end
state matters for porting):
- `3ca2229` measured the mask's problem: the 0.4 spread gate put the
  sigmoid half-point at 0.44 D spread — the p90 of real frames — so ~90%
  of pixels sat on the mask's flat top and the control acted as a
  near-uniform gain; per-pixel deltas correlated r=0.94/0.97 against
  Print Saturation on two real frames ("a second Print Saturation").
  Fix: gate 0.4 → 0.12 (median-spread calibrated), correlation → 0.64/0.75.
- `156252a` widened the desktop slider; `1d02071` gave the positive branch
  a ×3 gain (muted pixels have tiny deviations by selection, so k−1 barely
  moved them) and returned the slider to ±0.5.
- `3fb5ca8` concluded "the two density-domain colour controls read the
  same on real frames" and deleted the per-pixel block from BOTH engines
  (kernel block, WGSL constant, UBO slot, `dye_separation_spread_scale`
  gone); `density_saturation`(+trims) renamed `dye_separation`(+trims);
  their sidecar migration DROPS retired per-pixel values.

**PORTED (RETIRED) 2026-07-29 (same day, user approved) — the per-pixel
`dyeSeparation` is gone, converging on the end state.** Our port (this
morning, UPSTREAM entry below) was the ORIGINAL 0.4-gate variant —
precisely the one upstream measured as r≈0.95 redundant with our
`printSaturation`; porting the intermediate fixes just to land where
upstream started from would have been churn. Removed: the kernel block
from BOTH `ReferenceCurve.swift` and `NegPipeline.metal`,
`K.dyeSeparationSpreadScale` + the `DYE_SEPARATION_SCALE` MSL literal
(that constants-sync point is closed), the CurveUniforms field + pads
(stride 272 → 256, LayoutTests re-pinned), the settings field + decoder
line + history label + UI slider (tripwires 50 → 49). Mirroring
upstream's migration, a sidecar `dyeSeparation` key from the field's
hours-long lifetime decodes IGNORED — pinned by a new
DensityChromaTests retired-key test (the suite's dye-separation
behavioral cases dropped with the control; the sign-selects-population
contract is upstream history now). The Lab vibrance help text now points
at Print Saturation as the density-space counterpart. `printSaturation`
STAYS under our name — a recorded cosmetic divergence (upstream calls
the survivor Dye Separation; renaming would collide with the retired key
in same-day sidecars). UI range kept 0–2 (upstream 0.25–1.75; derive
clamp 0–3 unchanged). Per-channel trims remain not-ported (recorded
skip). NO fixture re-dump (identity in every dump config). All 167 tests
green (incl. GPU/CPU parity with printSaturation 1.4 active); bench
4.0 ms/frame steady state; app renders headlessly. CLAUDE.md updated —
including its tripwire counts, which had gone stale at 48 during the
morning port (now 49) — and the constants-sync list.

**Carried port candidate updated:** `db37476` widens the **colour
ring-around** (item 3, still to port) to 2cc steps out to ±4cc — 1cc
rungs were unreadable on a preview-sized mosaic. `RING_CC_STEP` 0.05 →
0.1; any port should use the new spec.

**Not applicable:** `4f4d7a1` broken live view no longer kills the
capture session (camera scanning), `4ee5267`/`7dc7903` Analysis-panel
info button + guide-window centring (their Qt help UI), `111492b` IR
channels from 64-bit HDRi RAW DNGs (retouch/IR loading), `35b6994`
crosstalk Separation → Strength slider rename (their crosstalk UI/docs —
renamed to avoid colliding with the NEW Dye Separation; no math),
`156252a` desktop slider range (superseded by `3fb5ca8`), `2f91a98`
version bump/discord URL, `723e2c5` user-guide churn.

**dump_fixtures.py:** still broken ≥de79e13 (lab_color `apply_vibrance`
import — carried); the `3fb5ca8` `apply_characteristic_curve` signature
change is benign for the script (no saturation kwargs passed; new
`dye_separation` default 1.0 is identity).

**Still open (carried over):** colour ring-around (`b646e23` + `db37476`
spec); `91a1b78` tunable Auto Density/Grade targets (user-initiated
only); the on-scan Color Mixer band re-tune pass (ours).

### 2026-07-29 — through `de79e13` (0.45.0-dev, 9 commits)

**The chroma stack was rebuilt upstream across three commits, with TWO
golden moves — the biggest pipeline reshape since 127bcd7.** End state:

- `7bc8bdc` **Print Saturation** (`density_saturation`, default 1.0, +
  per-channel trims): density-space saturation — an achromatic-projection
  matrix `k·I + (1−k)·J` composed into the paper dye-crosstalk matrix slot,
  applied to density-above-base AFTER the H&D curve, so its push scales
  with what the curve left in each channel (shoulder-compressed channels
  move less) and composes per paper.
- `c0d6ff0` (golden move) folded Dye Mute into that density-space slot as a
  grade-coupled damping of `density_saturation` (default OFF, was 0.25) and
  renamed Lab Saturation → **Chroma**.
- `de79e13` (golden move) **replaced Dye Mute AND Lab Vibrance with one
  signed `dye_separation`** (−0.5…+0.5, default 0): a per-pixel
  spread-masked rescale of dye-density separation inside the print-curve
  kernel — positive boosts muted pixels (vibrance), negative compresses
  already-separated ones (true anti-vibrance, which Lab never had); the
  sign flips the mask's TARGET population, not just the direction.
  Lab Vibrance is deleted (LabUniforms shrank; `apply_vibrance` gone).

**Bookkeeping — two records close/change:**
- The **Dye Mute skip is CLOSED, vindicated**: the control we declined
  (2026-07-17, reaffirmed 2026-07-27) no longer exists upstream; its
  default had already been walked 0.5 → 0.25 → off before deletion.
- Our **Lab vibrance now has NO upstream counterpart** (their vibrance is
  the density-space Dye Separation). Ours stays for sidecar compat and is
  identity in every parity config — recorded as a SwiftInvert-maintained
  control unless/until we converge on Dye Separation.

**PORTED 2026-07-29 (same day, user approved) — items 1 and 2:**
**Print Saturation** (`printSaturation`, default 1.0, UI 0–2, derive clamp
0–3) and **Dye Separation** (`dyeSeparation`, signed ±0.5) landed through
the full control checklist: settings + decoder + history labels (tripwire
counts 48 → 50), derive passthrough, printCurve BOTH sides — after the toe
softplus, on density above paper base, Print Saturation as the uniform-k
mean-deviation reduction of NegPy's saturation matrix (identity dye matrix
here; per-channel trims NOT ported, consistent with the recorded
per-layer-trims skip), Dye Separation with the exact spread-sigmoid mask
(`2·σ(spread/0.4) − 1`, sign selects the population; scale =
K.dyeSeparationSpreadScale, MSL literal — new constants sync point).
CurveUniforms grew via the ex-pad slot + a tail field (stride 256 → 272,
pinned in LayoutTests). DensityChromaTests: identity at defaults, neutral
invariance, k-direction on density spread, the sign-selects-population
contract both ways, sidecar round-trip; the tone-controls GPU-parity case
now runs with both active. No fixture re-dump (identity in every dump
config; all 160 tests green incl. NegPy fixture parity). Bench steady-state
unchanged (~4 ms/frame). Lab vibrance stays visible for now (its help text
points at Dye Separation as the successor); hide/deprecate is a future UX
call.

**Still to port (proposed, not yet implemented):**
3. **Colour ring-around** (`b646e23`, Shift+F) — the RA4 filtration proof:
   5×5 mosaic of real renders stepping ±2cc in 1cc steps on magenta and
   yellow, absolute ladder centred on neutral; click a patch to keep its
   filtration; shares the canvas with the test strip. Pairs naturally with
   the carried test-strip port.
4. **Test strip — PORTED 2026-07-29** (same day, user approved; revised
   5×5 spec): `TestStrip` grid math + incremental mosaic assembly in
   NegativeKit (tiling/hit-test/nearest-rung/assembly all closed-form
   tested), `ImageSession.renderTestStrip` reusing the warm cache tower
   (one analysis, one source upload, 25 derive+renderDisplay passes,
   ~150–250 ms, never HQ), canvas presentation with upstream's rules (no
   gridlines, hovered-patch outline, bold axis labels, current-rung
   accent), Print-group button + ⇧T (key monitor, text-field-guarded) +
   Escape-clears-first. Click = one history entry setting density+grade;
   auto toggles untouched (patches were rendered under them); cleared by
   any edit/navigation/tool/baseline/HQ change, stale builds fenced by a
   generation counter. One documented presentation divergence: columns
   mirrored so the image BRIGHTENS left→right (our "right = brighter"
   slider convention; upstream draws density ascending).
   TestStripRenderTests pins the core invariant end-to-end on the GPU:
   every mosaic patch is byte-identical to a full render at that patch's
   settings. 167 tests green.

**Noted, not proposed:** `38aa023` step wedge (21-step transmission wedge
printed through the frame's settings under their curve chart — tied to the
H&D/Analysis panel we deliberately don't ship, same rationale as the
density_histogram skip); `64b6e79`/`9b04748` grain-focuser loupe (2×
unsmoothed loupe + acutance figure, preview/full-res badge — UI tool; the
acutance read-out is the interesting fragment if we ever want a sharpness
aid).

**Not applicable:** `da9a033` contact-sheet tiles at proof scale (feature
we don't ship), `717db98` hide _IR TIFF sidecars from the contact sheet
(retouch/scan stack).

**dump_fixtures.py: BROKEN ≥de79e13** — see the Last reviewed block; adapt
the lab_color case before any re-dump past this range.

**Still open (carried over):** `91a1b78` tunable Auto Density/Grade targets
(user-initiated only); the on-scan Color Mixer band re-tune pass (ours).

### 2026-07-28 — through `e4bc450` (0.44.0 → 0.45.0, 6 commits)

**Kernel status: untouched — no golden moves, no constants.** The two
`features/exposure|process/` hits are additive UI-feature math and capture
gating; `dump_fixtures.py` unaffected.

**To port (proposed, not yet implemented — presentation layer, not
pipeline):** `e4bc450` **Test strip** (0.45.0's headliner, Shift+T): the
frame printed as a 6×6 grid of REAL renders — Print Density 0.4…1.9 in
steps of 0.3 left to right, Grade R55…R180 in steps of 25 top to bottom —
so the diagonals read light-to-dark and soft-to-hard like a split-filter
test strip; click a patch to commit its density+grade. Design decisions
worth keeping: ladders are ABSOLUTE (strips comparable frame to frame,
both straddle the defaults inside slider ranges), Auto Density/Grade stay
untouched on pick (the patches were rendered under them), always
preview-res, session-only state cleared by any real edit, NO gridlines
(reads as one print; only the hovered patch outlines), current-settings
rung accented on the axis labels. Their new `analysis.py` helpers
(`strip_cells`/`strip_mosaic`/`strip_patch_rect`/`strip_cell_at`/
`strip_nearest_cell`) are pure grid math — a Swift port is app-layer:
36 derive+renders at preview size is ~150–250 ms here (our slider path is
~5 ms/frame), one mosaic texture or 36 draws, hit-test + commit. No
parity surface, no fixture re-dump. Moderate effort (~half day).

**Not applicable:** `6073a98` sensor-profile gating when linear raw is off
(crosstalk/capture stack), `44f2a74` BEFORE-badge sync (their compare UI),
`d06f45f` auto-crop rebate + Rebate Trim (their camera-scan auto-crop
detection; we ship none), `9ef7732` settings-picker default-values fix
(their preset picker), `acf9ca5` IR-removal mottling on noisy IR planes
(retouch).

**Still open (carried over):** `91a1b78` tunable Auto Density/Grade targets
(user-initiated only); the on-scan Color Mixer band re-tune pass (ours).

### 2026-07-27 (second) — through `31aea5c` (0.44.0 release, 5 commits)

**Kernel status: untouched — no golden moves, no constants, no parity
impact.** The one `features/exposure/` hit is additive measurement math for
a UI feature (below); `dump_fixtures.py` unaffected (its `density_histogram`
import is signature-stable).

**PORTED 2026-07-27 (same day, user approved):** `ae56f8b` **Zone system
overlay** — `ZoneGrid` (NegativeKit: grid + flood-fill region labels, closed-
form tests incl. stride padding, concave-anchor and densitometer-ruler
agreement), `ZoneOverlay` (SwiftUI Canvas: merged-region edges with dark
underlay, bold numerals, paper ends red), computed from `DensitometerState`'s
cached bytes per adopt/toggle, gated to the plain presentation like the
probe. Toggle: "Zones" control-bar button + View menu + ⇧Z — implemented in
the window key monitor with a first-responder text guard (a bare-letter menu
equivalent would fire while typing; lesson from the profile-name field).
Deviation from upstream, documented in code: integer-boundary box means
instead of INTER_AREA (differences sit under the zone rounding step; no
parity surface). 147 tests green. Original proposal, for the record:
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
