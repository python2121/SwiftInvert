# Upstream (NegPy) review log

SwiftInvert ports NegPy's negative-inversion pipeline; NegPy
(`~/Documents/code/NegPy`) keeps evolving. This file records **the last NegPy
commit we reviewed**, so "what changed since we last looked?" always has a
baseline. Keep it current: every upstream review ends by updating the marker
and appending a history entry.

## Last reviewed

```
commit:   f279337  ("docs: consolidate 0.37.0 dust and analysis changelog entries (#486)")
reviewed: 2026-07-13
fixtures: Tests/Fixtures/ dumped from cac6396 (still valid — no kernel/constant
          changes in cac6396..f279337)
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
