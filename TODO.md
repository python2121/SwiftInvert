# Feature TODO

Candidate work, prioritized within each section. Effort: **S** < half a day,
**M** ≈ a day or two, **L** = multi-day. Sources: NegPy (`UPSTREAM.md`
flagged candidates), Negative Lab Pro v3/v3.1 release notes, darktable's
negadoctor, and standard macOS app conventions.

## macOS-native nice-to-haves

- [x] **Arrow-key frame navigation** (←/→ walks the film strip, honoring
  folder order) — S, the single biggest workflow hole.
- [ ] **Rating/culling**: pick ★/reject flags with keyboard (1–5, X, U),
  filter bar in the library — M. Pairs with arrow-key navigation for a
  real culling pass.
- [ ] **Open via Finder/Dock**: register RAW UTIs (CFBundleDocumentTypes in
  Packaging/Info.plist) so drag-onto-Dock and "Open With" work; accept
  folder/file drops on the library pane — S/M.
- [ ] **Open Recent** submenu (folders) in File — S.
- [ ] **Zoom menu items**: Zoom In ⌘+ / Out ⌘− / Fit ⌘0 / Actual Pixels ⌘1
  in View — S.
- [ ] **Copy rendered image** ⌘C (current conversion to clipboard) — S.
- [ ] **Dock progress + notification** for batch export (NSProgress on the
  Dock tile; UNUserNotificationCenter when done/failed) — S.
- [ ] **Settings window** ⌘, — default export options, canvas color,
  preview size cap, sidecar behavior — M.
- [ ] **Share menu** (NSSharingServicePicker on an on-demand JPEG) — S/M.
- [ ] **Drag a frame OUT** of the library/canvas to Finder → exports on
  demand (NSItemProvider file promise) — M.
- [ ] **Help menu** → open README/guide; About panel version/credits — S.
- [ ] If ever distributed: sandbox + security-scoped bookmarks for the
  library folder, hardened runtime + notarization, Sparkle-style
  updates — L (not needed while personal).

## Image pipeline / conversion low-hanging fruit

- [x] **TIFF compression** (Adobe Deflate + horizontal predictor; NegPy
  `fb4b7a7`) — S. Exports are currently uncompressed; ~2-3× smaller files.
- [x] **Carry EXIF into exports** (capture date, camera/lens, exposure —
  ImageIO metadata copy from the RAW) — S/M. Exports are currently bare.
- [x] **Batch apply: "Paste Adjustments to Selection"** — S. Copy/paste
  exists; applying to a multi-selection is the missing 20 lines with
  outsized value.
- [ ] **Preset management** (save/apply named setting bundles; NLP v3
  headliner) — M. Builds directly on the copy/paste plumbing.
- [ ] **Roll analysis / match** (NLP v3's flagship; NegPy's roll-baseline
  concept): meter selected frames together → shared normalization so a
  roll converts consistently; pick a reference frame to "match" — M/L.
  Highest conversion-quality payoff for whole-roll work.
- [ ] **White-balance eyedropper** (NLP-style: click a neutral → temp/tint;
  negadoctor's pickers are the same idea) — M.
- [ ] **Straighten reference line** (drag a line along a horizon → exact
  angle; NegPy 0.37 has this) — S/M; plus a one-click
  **auto-straighten** via Vision's `VNDetectHorizonRequest` — S.
- [ ] **Auto-crop film borders** (detect rebate/holder edges; NegPy
  autocrop / Vision rectangle detection) — M.
- [ ] **Clipping "blinkies"** overlay on the image (shadow/highlight clip
  masks; the histogram indicators already compute this) — S.
- [ ] **Crop aspect-ratio presets** (original, 2:3, 4:5, 5:7, 1:1, free) — S/M.
- [ ] **Spot densitometer + density histogram** (NegPy 0.37 `77c8113`,
  flagged in UPSTREAM.md) — M. Hover readout of negative density/zone.
- [ ] **"Negative character" stat** (measured density range vs default →
  "flat ≈N−1 / contrasty ≈N+1" diagnostic; NegPy `stats.py`) — S.
- [ ] **Midtone gamma (Snap) slider** (deliberately-skipped NegPy port;
  cheap to add) — S.
- [ ] **Export sharpening** (unsharp after downscale) — S/M. We ship no
  sharpening at all.
- [ ] **B&W negative mode** (NegPy has a BW process mode; post-curve luma
  collapse) — M.
- [ ] **Slide film / positive (E-6) support** (NLP added it in v3.1; no
  inversion, WB + tone only) — M.
- [ ] **Duplicate/near-duplicate detection** (Vision feature prints) — M.

## Deliberately deferred (recorded so we stop re-deciding)

- Per-channel R/G/B trims, Split Grade, Zone Density (NegPy's convergent
  equivalents of our tone controls — revisit only if per-channel crossover
  correction is needed).
- Dust/scratch removal (NegPy retouch is a large subsystem), C-41 denoise,
  crosstalk profile editor, LUT film looks, camera-scanning/tethering,
  RGB narrowband scanning, contact sheets, flat/log export intent.
