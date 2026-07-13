# Upstream (NegPy) review log

SwiftInvert ports NegPy's negative-inversion pipeline; NegPy
(`~/Documents/code/NegPy`) keeps evolving. This file records **the last NegPy
commit we reviewed**, so "what changed since we last looked?" always has a
baseline. Keep it current: every upstream review ends by updating the marker
and appending a history entry.

## Last reviewed

```
commit:   cac6396  ("fix: Apply Settings roll count respects the filename filter (#450) (#451)")
reviewed: 2026-07-11
fixtures: Tests/Fixtures/ dumped from this commit
```

## How to run a review

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
