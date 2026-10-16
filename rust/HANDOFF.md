# Orchestrator handoff — Rust port (branch `rust-port`)

Written 2026-10-16 by the Fable orchestrating session at model cut-over. **Read this, then
`PLAN.md` §10 (progress), then the three agent branches below.** Run all further agents and
reviews on **Opus** (user instruction).

## Where things stand

Merged and green on `rust-port` (pushed to `origin`, github.com/python2121/SwiftInvert):

| Crate | State | Tests |
|---|---|---|
| `negkit` (math core) | fully ported from NegativeKit; 5 cross-model reviews closed | 267 |
| `negraw` (RAW) | lcraw default = LibRaw area + bit-depth white; `libraw` FFI feature builds on the Mac; parity vs LibRaw at u16-LSB level | 15 |
| `neggpu` (wgpu/WGSL) | parity vs CPU ~1e-7 on every control group, Vulkan AND Metal; host fixes from review 6 landed | 42 |
| `negsession` (engine) + `negcli` | cache tower (ImageSession keys), sidecars, profiles, history, export (ICC), GpuRenderer, test strip, zone placement | 87 + gpu parity |
| `negui` + `apps/swiftinvert` | the egui SHELL (U1) merged: library, loupe, controls table, histogram, history, menus, selftest | 7 |
| `negui` tools/dialogs/darkroom (U2b, U3b) | crop & straighten, analysis rect, export dialog + batch, profiles picker, GPU texture presenter (ring of 2), densitometer + zone strip, zone overlay, test strip, zone placement, interactive histogram, selftest flags `--crop --dialogs --export <dir> --latency --teststrip --zones` | 16 + 15 |
| **Total** | `NEG_RAW_SAMPLE=/home/deck/Pictures/IMG_0420.CR3 cargo test --workspace` | **462 passed** (Linux) |

Verified on the Mac (Apple M2, Rust 1.96, via ClaudeShip jobs): whole workspace builds and
tests pass; `swiftinvert --selftest` renders a real negative headless (first render 71 ms,
adjust→frame 15 ms). Screenshots: `/tmp/swiftinvert-rust-port/shell_mac*.png` on the Mac;
Linux: `/home/deck/.cache/swiftinvert-wt/scratch-U1/shell*.png`.

## In-flight agent branches (Fable agents told to wind down and commit)

All worktrees are under `/home/deck/.cache/swiftinvert-wt/` (NOT /tmp — it's a 7.7 GB tmpfs
that filled once). Each agent was asked to commit a compiling state with a DONE/PARTIAL list in
the commit message and to report in ≤ 20 lines. **ALL FOUR MERGED (S2, U2 scaffold, U3b darkroom, U2b tools/dialogs/presenter) — rust-port has the complete feature set; 462 tests green on Linux.** A final Opus review of `negui` (review 8) is the last open item, plus the Mac confirmation run.
(U2 and U3 both add `// --- U2 ---` / `// --- U3 ---` blocks to `negui/src/app.rs`, `loupe.rs`,
`engine.rs`; expect a small manual merge there).

1. ~~`wt/wave4-S2`~~ merged.
2. `wt/wave4-U2b` (Opus) — tools & dialogs: Crop & Straighten, Crop for Analysis, Export dialog +
   batch, Choose Default Settings (profiles), GPU presenter (`presenter_gpu.rs`), selftest
   `--crop/--export`. Worktree `wave4-U2b`. (The Fable U2 landed only `tools.rs` commit math.)
3. `wt/wave4-U3b` (Opus) — darkroom UI: densitometer read-out + zone strip, zone overlay, test strip
   layer, zone placement, interactive histogram window, selftest `--teststrip/--zones`.
   Worktree `wave4-U3b`. (The Fable U3 wrote no code.)
Both add `// --- U2 ---` / `// --- U3 ---` blocks to `negui/src/app.rs`, `loupe.rs`, `engine.rs` and
share a `CanvasOverlay` trait + overlays slot in `loupe::canvas` — expect a small manual merge.

After merging each: `cd rust && cargo fmt --all -- --check && cargo clippy --workspace
--all-targets -- -D warnings && NEG_RAW_SAMPLE=/home/deck/Pictures/IMG_0420.CR3 cargo test
--workspace`, then push, then `git worktree remove --force <path>`.

### The S2 list (engine review 7) — ALL LANDED in f4dc641 (kept for reference)
1. sidecar/profile atomic writes: per-call unique temp name + fsync before rename.
2. export: temp + rename (a failed encode must not truncate the previous file).
3. `release_enhanced_tiers` keeps the MEDIUM tier (only the full tier is dropped) so HQ Auto
   never triggers a full decode.
4. `render_tool_preview` takes the resolved `RenderTier` (Mac rule).
5. `Session::render_into(request, &mut Vec<u8>)` and `Session::render_to_texture(request,
   &wgpu::Texture)` — the zero-allocation / zero-readback frame paths through the cache tower.
6. `ExportOptions` sticky fields independent of the chosen format/destination (Swift keys).
7. library scan: `symlink_metadata`, skip symlinked dirs.
8. `DensitometerState::adopt(..., Encoding::{Working, Srgb})` + `negkit::reference::encoded_from_srgb`
   (the egui app displays sRGB bytes; the probe must invert the sRGB leg).
9. `needs_preparation_for(&settings, tier)`.
10. profiles: lenient per-entry parse, keep `activeProfileID`.
11. `sample_zone_pin` checked arithmetic.
12. negcli arg validation; bench temp-file guard.
13. lib.rs docs: front-end obligations; lenient-sidecar divergence; baseline semantics.

## What remains after those three merge (PLAN §6 waves 4–5)

- Wire U2's GPU presenter to S2's `Session::render_to_texture` (zero-copy loupe); measure
  adjust→screen in HQ-on full tier (engine Vec path was ~80 ms at 24 MP; neggpu alone 34 ms).
- Profile-editor WINDOW mode (shared draft applied to every frame, no sidecar writes) — U2 was
  told to ship only the picker + minimal editor.
- Light theme toggle; library context menu; interactive-histogram polish; rfd/portal folder
  picker decision (the shell uses an in-app picker to avoid GTK).
- Deferred `negkit` hygiene from the Sonnet sweep: private fields on `RgbImage`/`Plane`
  (constructors already check), `#[must_use]`, contrast-mask blur allocations, `fixtures`
  module behind a cargo feature (L1 purity), duplicated eps/clamp helpers.
- Full-tier display at 83 ms through the session (target 60): use `render_into`/texture path
  from the UI with double buffering; profile the 97 MB readback.
- Final rounds: Opus review of `negui` (never-crash in egui closures, no UI-thread blocking,
  worker/latest-wins correctness, selftest coverage); Sonnet/Opus quality sweep of
  `negsession`/`negui`; then a last Mac run of everything incl. `--selftest` variants.
- Update `CLAUDE.md` (repo root) with a "Rust port" section pointing at `rust/PLAN.md`,
  `rust/ci.sh`, the Mac procedure, and the decisions log — the file says to keep it current.
- `UPSTREAM.md`/`negpy-review` are unaffected (fixtures unchanged).

## Decisions already made (don't relitigate) — PLAN §0, plus:
- lcraw (pure Rust) is the default decoder with LibRaw's white level and image area; LibRaw FFI
  is the parity reference behind `--features libraw` (Mac/distrobox only).
- The CPU reference applies tone controls in the GPU's parallel form (the Swift CPU reference
  was sequential; the GPUs are what users saw).
- The port follows the Mac app where it differs from the Qt bridge (tool preview tiers, meter
  angle keying, medium→full fallthrough, unbounded history).
- GPL-3 stays; lightcraft/craft-libs crates are consumed as MIT/Apache deps; nothing flows back.

## Mac procedure (memory file `mac-access-via-claudeship.md` has the same)
```
git push origin rust-port
claudeship run --host "Macbook Air" --cwd '~/Documents/code/SwiftInvert' --max-seconds 2400 -- sh -lc \
  'cd /tmp/swiftinvert-rust-port && git fetch -q origin rust-port && git checkout -q --detach origin/rust-port && cd rust && cargo test --workspace 2>&1 | grep -E "test result|FAILED"'
claudeship jobs wait <id> --host "Macbook Air" | grep -v -E 'gitstatus|setopt|zsh'
```
The Mac's own checkout stays on `main`, clean. Sample negatives: `~/Pictures/Negative Test 1/*.CR3`
(their sidecars predate this work; the selftest never writes). **Cleanup at the very end:**
on the Mac `git -C ~/Documents/code/SwiftInvert worktree remove --force /tmp/swiftinvert-rust-port`;
here `git worktree prune` and remove `/home/deck/.cache/swiftinvert-wt/`; the lightcraft clone
and scratch crates live in the session scratchpad and vanish with it.

## Review protocol that worked
Fresh read-only reviewer per crate ("formula by formula vs the Swift file; findings numbered
BUG/RISK/NIT with file:line and both expressions; verified-correct list; no fixes"), then the
owning agent (or the orchestrator for small items) fixes with a test per finding, then a
second pass only on the fixes. Seven rounds so far found: 1 real panic, 1 GLSL bug in the
existing Linux Swift app (f32 gid split past 2^24 px, `NegPipeline.comp:341-343` — worth
fixing upstream in the Swift tree too), the sequential-vs-parallel tone controls, a decode
white-level mismatch, two file-safety risks. Everything else was confirmed faithful.

## Open questions for the user (PLAN §9) — unchanged
App name; keep LibRaw FFI long-term; egui's non-native look on macOS; when to retire `qt/` and
`CoreBridge`.
