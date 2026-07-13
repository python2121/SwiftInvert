---
name: negpy-review
description: Review what changed in NegPy (the upstream reference implementation) since our last recorded review, recommend which changes to port into SwiftInvert — inversion-pipeline changes first — and update UPSTREAM.md with the new baseline and decisions.
---

# NegPy upstream review

SwiftInvert is a deliberate partial rewrite of NegPy (`~/Documents/code/NegPy`),
which remains the numerical reference and keeps improving. The standing goal:
**steal every good inversion-pipeline idea upstream ships**, while respecting
the divergences we chose on purpose. `UPSTREAM.md` (repo root) is the review
log — every run of this skill ends by updating it, even if nothing gets ported.

The deliverable of this skill is a **review report + an updated UPSTREAM.md**.
Do NOT implement ports during the review unless the user explicitly asks —
suggest, prioritize, and log; porting is a follow-up they approve.

## Step 1 — Establish the baseline

Read `UPSTREAM.md`:
- The **Last reviewed** block gives the baseline commit hash and whether the
  fixtures were dumped from it.
- Check the most recent history entry for a "known-unreviewed" or
  "proposed, not yet ported" note — those items are part of THIS review's
  scope (don't silently drop them).

Also skim the "Deliberate divergences from NegPy" list in `CLAUDE.md` — you
must not recommend "fixes" that would undo a divergence we chose
(preSaturation 1.15, trueBlack default on, analysis buffer 0.10, no lab
sharpen, SwiftInvert-only controls).

## Step 2 — Fetch and enumerate the unreviewed range

```bash
git -C ~/Documents/code/NegPy fetch origin
git -C ~/Documents/code/NegPy rev-parse --short origin/main   # <new-tip> — record this exact hash
git -C ~/Documents/code/NegPy log --oneline --reverse <baseline>..origin/main
```

**Always review against `origin/main`, never the local checkout's HEAD** —
the local working copy may be behind, ahead, or dirty. `<new-tip>` is what
goes into the updated marker.

For narrative context (what upstream *thinks* they shipped), also diff the
release notes and version:

```bash
git -C ~/Documents/code/NegPy diff <baseline> origin/main -- docs/CHANGELOG.md VERSION docs/PIPELINE.md
```

Changelog entries about the exposure/inversion pipeline are the fastest map
of what matters; PIPELINE.md changes signal semantic/stage-order changes.

## Step 3 — Triage commits

Split the range into three buckets. Get the pipeline-relevant subset with:

```bash
git -C ~/Documents/code/NegPy log --oneline <baseline>..origin/main -- \
  negpy/features/exposure/ negpy/features/process/ negpy/kernel/image/ \
  tests/test_scene_linear_relocation.py tests/test_characteristic_curve.py
```

1. **Pipeline-relevant** (the point of this review): anything touching
   `negpy/features/exposure/` (analysis.py, normalization.py, logic.py,
   models.py — home of `EXPOSURE_CONSTANTS` —, stats.py, processor.py,
   densitometer.py, and `shaders/*.wgsl`), `negpy/features/process/`,
   `negpy/kernel/image/`.
2. **Golden/characterization moves**: `tests/test_scene_linear_relocation.py`
   or `tests/test_characteristic_curve.py` changed → upstream deliberately
   changed the default look. Highest-priority items; find the producing
   commit and understand *why* the goldens moved.
3. **Everything else**: UI, camera scanning, contact sheets, crosstalk,
   papers/toning, B&W/E6, retouch, CPU parallelism, packaging — out of
   SwiftInvert's scope by design. List them in one line each in the report
   (so the log shows they were seen), no deep read needed. Exception: GPU
   parity fixes and B&W/histogram render fixes can hide shared-kernel
   changes — glance at the diff paths before dismissing.

If the range is large (>~40 commits), delegate the bucket-3 skim and the
changelog read to an Explore agent and spend your own context on bucket 1.

## Step 4 — Deep-read the pipeline diffs

```bash
git -C ~/Documents/code/NegPy diff <baseline> origin/main -- \
  negpy/features/exposure/ negpy/features/process/ negpy/kernel/image/
```

Map every substantive hunk to its SwiftInvert counterpart:

| NegPy | SwiftInvert |
|---|---|
| `exposure/normalization.py` (prefilter, `analyze_log_exposure_bounds`, `resolve_analysis_region`, `measure_neutral_axis_from_log`, meters) | `Sources/NegativeKit/Prefilter.swift`, `BoundsAnalysis.swift`, `Meters.swift`, `Stats.swift` |
| `exposure/logic.py` (grade→slope, pivot solve, curve fitting, cast removal) | `Sources/NegativeKit/CurveLogic.swift` |
| `exposure/models.py` / `EXPOSURE_CONSTANTS` | `Sources/NegativeKit/ExposureConstants.swift` (`K`) — MSL duplicates some constants, see CLAUDE.md "Constants sync points" |
| `exposure/shaders/*.wgsl` (normalization, exposure, output_encode, density_hist) | `Sources/MetalRenderKit/Shaders/NegPipeline.metal` **and** the CPU mirror `Sources/NegativeKit/ReferenceCurve.swift` (both sides must move together) |
| `exposure/processor.py`, `exposure/stats.py` | `ImageSession` / `ExposureKernel` orchestration (`ReferenceCurve.swift`, `Sources/SwiftInvert/ImageSession.swift`) |
| `kernel/image/logic.py` (decode/resize/orientation) | `Sources/RawDecodeKit/RawDecoder.swift` |
| `process/` (settings model, sidecar-ish state) | `Sources/NegativeKit/ExposureSettings.swift`, `SidecarStore` |

For each change, determine: is it a **numerical/semantic change** (constants,
curve math, analysis percentiles, stage order — these affect parity and may
require fixture re-dump), a **new control/feature** (candidate port; must be
identity-at-default here), or a **bug fix** (check whether SwiftInvert has
the same bug — our port may have inherited it, or may have never had it).

## Step 5 — Judge each change

Classify every pipeline-relevant change as one of:

- **Port** — improves the inversion; fits our scope (C-41, camera RAW).
  Note the effort and whether it needs a fixture re-dump. New controls go
  through the "Adding a new adjustment control" checklist in CLAUDE.md.
- **Deliberately skip** — conflicts with a recorded divergence, or is a
  convergent duplicate of something we already have (e.g. upstream Split
  Grade/Zone Density vs our tone controls). Record *why* in one line;
  future reviews must not re-litigate it silently.
- **Not applicable** — out of scope (UI, capture, papers, B&W/E6, CPU
  parallelism, stages we don't ship).

Judging rules:
- **Golden moves outrank everything** — a deliberate default-look change
  upstream is exactly the "steadily improving main project" signal we track.
- Constants changes are cheap to port but expensive to verify: `K` update +
  MSL duplicate + fixture re-dump + `make test`.
- A NegPy bug fix in code we ported ≈ a bug we probably have. Verify against
  our source before assuming either way.
- When upstream and SwiftInvert solved the same problem differently, prefer
  keeping ours *only* if the divergence is already recorded; otherwise treat
  upstream as the reference and lean toward converging.

## Step 6 — Report to the user

Lead with the headline: how many commits, how many pipeline-relevant, and the
single most important inversion change (or "no inversion-pipeline changes").
Then:

1. **Recommended ports**, priority order (inversion/analysis/curve changes
   first), each with: what changed upstream (commit + one-line mechanism),
   what it maps to here, estimated effort, fixture-re-dump yes/no.
2. **Deliberate skips** with one-line rationale.
3. **Not applicable** as a compact list.
4. Whether `scripts/dump_fixtures.py` still matches upstream signatures
   (check its imports against the diff if `normalization.py`/`logic.py`
   function signatures changed — a re-dump against changed signatures fails
   or, worse, silently dumps the wrong thing).

## Step 7 — Update UPSTREAM.md (always, even for a null review)

1. Update the **Last reviewed** block: new tip hash + subject, today's date.
   Leave the `fixtures:` line pointing at the commit fixtures were actually
   dumped from — it only advances when a re-dump happens.
2. Append a dated history entry matching the existing format:
   `### YYYY-MM-DD — through \`<new-tip>\` (<version range>, ~N commits)`
   with **Ported / To port (proposed, not yet implemented) / Deliberately
   skipped / Not applicable** subsections. Be honest about status: reviewed
   ≠ ported. Anything left in "To port" is automatically in scope next run.
3. If nothing pipeline-relevant changed, still append a one-paragraph entry
   ("no pipeline changes; range was X/Y/Z") — a recorded null review is what
   makes the next "what changed?" cheap.

## Step 8 — Only if the user asks to port now

Follow CLAUDE.md's control checklist and constants-sync rules. In order:
pull NegPy to the reviewed tip → verify/adjust `scripts/dump_fixtures.py` →
update `K`/kernels (CPU `ReferenceCurve.swift` + MSL `NegPipeline.metal`
together) → re-dump fixtures (`cd ~/Documents/code/NegPy && uv run python
~/Documents/code/SwiftInvert/scripts/dump_fixtures.py`) → `make test` →
`negcli bench` for regressions. Then move the item from "To port" to
"Ported" in UPSTREAM.md, advance the `fixtures:` line, and update CLAUDE.md's
"Kernel constants are synced with NegPy X.Y" sentence (and the divergences
list if a new divergence was created).

## Pitfalls

- The local NegPy checkout drifts — the baseline and new tip are **commit
  hashes against origin/main**, never "whatever is checked out".
- `git log` path-filtering misses renames of tracked files; if a bucket-1
  file vanished from the diff, check for renames (`--follow`, or look for
  `rename` lines in `git diff --stat`).
- WGSL-only changes are still kernel changes (upstream sometimes fixes the
  GPU side alone); our port folds both their CPU and GPU semantics into one
  pair of mirrored kernels.
- Don't recommend NegPy's defaults where CLAUDE.md records a deliberate
  divergence — flag the *change* if upstream moved such a default, but frame
  it as "our divergence baseline moved", not "we're wrong".
