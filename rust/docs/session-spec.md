# Session spec — engine semantics for the `negsession` crate

What the Rust engine must reproduce from `Sources/SwiftInvert/ImageSession.swift`, the
non-UI parts of `AppModel.swift`, `SidecarStore.swift`, `DefaultProfile.swift`,
`ProfileStore.swift`, `Exporter.swift`, `ExportSheet.swift`, `Densitometer.swift`,
`HistoryLabels.swift`, and the C bridge (`Sources/CoreBridge/CoreBridge.swift`,
`qt/swiftinvert_core.h`). Every rule cites `file:line`. Math (prepare/finalize/derive/
kernels/contrast mask/crop geometry) is specified by PLAN.md §3-§4 and the Swift sources;
this document is about caching, keys, invariants, state machines and file formats.

## 1. Pipeline shape

decode → orient → analyze (CPU, proxy) → derive (µs) → render (GPU) → display/export.
Slider edits re-run ONLY derive + render; analysis re-runs only when its keyed inputs change
(ImageSession.swift:8-10). Settings are one value type (`ExposureSettings`, 54 fields,
PLAN §3.2); every seam is "settings → output", with the session owning all caches.

## 2. Settings, sidecars, profiles

| Rule | Value | Source |
|---|---|---|
| Sidecar name | `<source without extension>.swiftinvert.json` next to the source | SidecarStore.swift:14-16 |
| Legacy name | `<…>.negswift.json`, read as fallback (second), deleted on next save | :18-20,23,41 |
| Envelope | `{ "version": 1, "settings": { …ExposureSettings… } }`; written pretty-printed with sorted keys, atomic write | :9-12,33-38 |
| Load | first file that exists AND decodes wins; any decode error = treat as absent (returns nil) | :22-31 |
| Decode rules | every key optional → default; `rotation` int; `levelsRed/Green/Blue` accept `[[x,y],…]` or a legacy single `[x,y]` (kept only if x≠y) or absent → `[]`; unknown/retired keys ignored; rects `{x,y,width,height}` | ExposureSettings.swift:180-249; NormalizedRect.swift:5-16 |
| Bridge sidecar | same files + envelope; `si_sidecar_load` returns the BARE settings object (sorted keys), `si_sidecar_save` takes bare settings and canonicalises through decode→encode | CoreBridge.swift:492-541 |
| Fresh frame | `SidecarStore.load(url) ?? DefaultProfile.settings` | AppModel.swift:642 |
| `DefaultProfile.settings` | = `ProfileStore.shared.active.settings` | DefaultProfile.swift:22-24 |
| Built-in profile | stock + `exposureStops 0.6, highlights −0.2, shadows 0.3, blackPointOffset −0.09, overallContrast 0.2, shadowContrast 0.55, preSaturation 1.25` | :45-55 |
| Geometry fields | `rotation, flipHorizontal, fineRotation, cropRect, analysisRect, analysisRectFineRotation` — the one list used by `keepingGeometry(of:)`, `adjustmentsOnly`, Paste Adjustments and the profile editor | ProfileStore.swift:18-30 |
| `SettingsProfile` | `{ id: UUID, name: String, settings: ExposureSettings }`, Codable | :8-12 |
| Reserved rows | "None" id `00000000-0000-0000-0000-000000000000` = stock (first, default active); "SwiftInvert Default" id `…0001` = built-in; never persisted/edited/deleted | :52-63 |
| Persistence | UserDefaults `settingsProfiles` = JSON array of USER profiles only; `activeProfileID` = UUID string (ignored at load if not in `all`) | :76-87,119-124 |
| Order | `all = [None, builtIn] + userProfiles` (creation order); `active = all.first(id) ?? None` | :70-74 |
| upsert | refuses reserved ids; strips geometry (`adjustmentsOnly`); replace by id else append | :98-108 |
| delete | refuses reserved; deleting the active → active = None | :112-117 |

## 3. Session cache tower (ImageSession.swift)

One actor per image URL; all calls serialised. Tiers, top to bottom, with their keys:

| Tier | Stored | Key (rebuild when ≠) | Source |
|---|---|---|---|
| Decode (preview) | `basePreview` = native preview decode (EXIF baked) `.downsampled(maxLongEdge: 1536)`; `medium.base` = the native buffer, kept only if `native.w·h ≤ 20_000_000` and it is actually larger than the proxy | once per session | :257-271,216 |
| Decode (full) | `full.base` = `decode(quality: .full)` | lazily on first `.full` render; released per §5 | :648-650 |
| Meter preview | `meterPreview = basePreview.oriented(rotation, flip, fineRotation: meterAngle)` | `MeterKey{rotation, flipHorizontal, meterAngle}`; **change also clears `prepared`, `preparedKey`, `analysis`** | :21-32,275-286 |
| Render preview (proxy) | `preview` = `meterPreview` aliased when `|fineRotation| ≤ 0.005 && |meterAngle| ≤ 0.005`, else `basePreview.oriented(rotation, flip, fineRotation)` | `OrientKey{rotation, flipHorizontal, fineRotation}`; **change clears proxy `fullTexture`, `croppedTexture`, `croppedTextureRect`** | :53-55,118-123,287-305 |
| Prepared | `ExposureKernel.prepare(meterPreview, cropRect, analysisRect)` (buffer 0.10 default) | `PreparedKey{analysisRect, cropRect}`; change clears `analysis` | :128-134,306-314 |
| Analysis | `ExposureKernel.finalize(prepared)` | nil → recompute (offset-independent: WP/BP drags never re-run anything here) | :125-127,315-317 |
| GPU source (proxy) | `fullTexture` = upload(preview) when `uncropped || cropRect == nil`; else `croppedTexture` = upload(preview.cropped(cropRect)) keyed by `croppedTextureRect == cropRect` | per crop | :138-140,370-384 |
| Display tiers | `DisplayTier{base, oriented, orientKey, fullTexture, croppedTexture, croppedRect}` ×2 (`medium`, `full`); `oriented = base.oriented(rotation, flip, fineRotation)` keyed by `OrientKey`; textures keyed like the proxy | per tier | :175-208,392-417 |
| Mask plane | `ContrastMask.buildPlane(renderSource, baseBounds, spacerPercent)` + uploaded texture, **always from the PROXY** `preview` (cropped unless `uncropped`) | `MaskKey{orient: OrientKey, uncropped, spacer: maskSpacer, meter: MeterKey, prep: PreparedKey}`; freed when `contrastMask == 0` or plane build fails | :147-156,326-365 |

`meterAngle(settings) = analysisRect != nil ? analysisRectFineRotation : 0` (:30-32). The
`prepare(settings:)` entry (:257-319) runs those checks in order (decode → meter → render preview →
prepared → analysis) and returns `(preview, analysis)`.

**Fine-rotation invariant.** Analysis reads the orientation-only image (90° steps + flip), never
the straightened one, so the inscribed auto-crop can't drift the conversion (:16-20). Render
source = `oriented(+fineRotation, inscribed)` THEN `cropped(cropRect)` (:297-299,377-383), so
`cropRect` is normalised on the fine-rotated frame; `frameSize` reported to the UI is the
orientation-only size (decode dims swapped when `rotation % 180 != 0`, :104-117).
Mac nuance: when an `analysisRect` exists the meter image is built at the angle the region was
drawn (`analysisRectFineRotation`, :24-26) — the bridge and Qt instead always meter at 0° and write
`analysisRectFineRotation = 0` (CoreBridge.swift:92-101; qt/main.cpp:716-722). The Rust session
should implement the Mac keying (it is a superset: the bridge case is `meterAngle == 0`).

Rect semantics: `NormalizedRect.pixelROI` truncates (`Int(x·w)`…), clamps to the frame, nil when
< 2 px (NormalizedRect.swift:31-47); crops use it, so a proxy and a full-res render of one crop
cover slightly different windows — reported via `contentWindow`, never "fixed".

## 4. Render entry points

### `render(settings, uncropped = false, tier = .proxy, retainFull = false) -> RenderOutput` (:636-687)

1. `(image, analysis) = prepare(settings)`; `params = deriveRenderParams(settings, analysis)`.
2. Resolve tier (:420-423): `.mediumIfFree` → `.mediumIfFree` if `medium.base != nil` else `.full`.
   - `.full`: decode full if needed; `tierSource(&full)`.
   - `.mediumIfFree`: `if !retainFull { full.clear() }`; `tierSource(&medium)`.
   - `.proxy`: `if !retainFull { full.clear() }`; proxy `sourceTexture`.
   - The medium tier is never dropped by render (it was free).
3. Mask plane from the proxy (§3) regardless of tier.
4. `pipeline.renderDisplay(source, params, maskPlane)` → rgba8 + 4×256 histogram.
5. `RenderOutput{ image, histogram, frameSize, displayAspect, contentWindow, densityRange }`.

`uncropped` widens only what is RENDERED (source texture + mask plane); the meter stays scoped by
the settings' own `cropRect` (CoreBridge.swift:150-162 explains why a settings substitution is wrong).

| Output field | Definition | Source |
|---|---|---|
| `frameSize` | orientation-only dims of the PROXY base | :104-106,681 |
| `displayAspect` | `CropGeometry.displayAspect(frame: proxy frame, radians: fineRotation if |·| > 0.005 else 0, crop: uncropped ? nil : cropRect)` — continuous, tier-independent | :89-99 |
| `contentWindow` | `CropGeometry.contentWindow(frame: THIS tier's oriented frame size, radians, crop, orientedPixels: tier's oriented dims, roi: pixelROI used)` — where the bitmap really sits in the ideal rect; must measure a tier against its OWN frame | :57-82,108-117 |
| `densityRange` | `analysis.baseBounds.luminanceDensityRange` (pre-offset) | :243-246,686 |

### `renderDetached(settings) -> RenderOutput` (:567-624)

Straighten 0°-base precompute. Touches NO cache state: reuses the cached `analysis` read-only when
`MeterKey` and `PreparedKey` both match, else computes a fresh prepare+finalize; orients, crops,
uploads and builds an inline mask plane without storing anything; proxy only, cropped, no tier.

### `needsPreparation(settings, tier) -> Bool` (:441-459) — the "Analyzing…" rule

True when: resolved tier is `.full` and (`full.base == nil || full.oriented == nil || OrientKey ≠ full.orientKey`);
or `preview/prepared/meterPreview` is nil; or `MeterKey` ≠ cached; or `PreparedKey` ≠ cached.
Fine-rotation-only changes → false (re-orient is cheap). The medium tier never flashes it.
AppModel shows the pill only if this is true AND it isn't a mid-straighten-drag render (AppModel.swift:875-881).

### `releaseEnhancedTiers()` (:433-436): `full.clear(); medium.clear()` (base + oriented + textures).

### `exportRender(settings) -> RGBImage` (:690-708) — the what-you-see invariant

`(proxy, analysis) = prepare(settings)` (analysis from the 1536 proxy); fresh `decode(.full)`
`.oriented(rotation, flip, fineRotation)`, `.cropped(cropRect)` if set; params from the proxy
analysis; mask plane from the proxy (cropped); `pipeline.render(image, params, computeHistogram:
false, maskPlane:)` → the ENCODED (Adobe-RGB TRC) float image. No resize here.

### Test strip (:471-501)

`renderTestStrip(settings, orientation: Int) -> CGImage`: one `prepare`, one cropped proxy source,
one mask plane; for each `TestStrip.cells(orientation:)` (row-major, 5×5) set `density`/`grade`
from the cell, derive, `renderDisplay(computeHistogram: false)`, `TestStrip.copyPatch` into the
mosaic (same size as the frame). Ladders: columns density `[1.6,1.3,1.0,0.7,0.4]` (brighter →),
rows grade `[75,95,115,135,155]` (TestStrip.swift:16-18); `cell(row,col,orientation)` rotates the
logical grid (:48-63). Always the proxy tier.

### Zone placement (:510-565)

| Call | Semantics |
|---|---|
| `sampleZonePin(settings, u, v, radius = 2) -> (valRGB, valLuma, maskVal)` | `prepare`; `maskVal` = bilinear plane sample at (u,v) or 0; bounds = `baseBounds.applyingOffsets(wp, bp)`; map (u,v) through `cropRect` (`cu = x + u·w`); centre pixel `clamp(Int(cu·W), 0, W−1)`; mean over the clamped (2r+1)² patch of `(log10(max(p,1e-6)) − floor)/(ceil − floor)` per channel (denominator → 1e-6 if |·| < 1e-6); `valLuma = lumaR·r + lumaG·g + lumaB·b` |
| `predictedZone(settings, valLuma, maskVal = 0) -> Double` | `ZonePlacement.predictedZone(settings, analysis, valLuma, maskVal)` |
| `solveZonePlacement(settings, pins) -> Solution?` | `ZonePlacement.solve(settings, analysis, pins)` (CPU, in the actor) |

## 5. Display tiers and HQ (AppModel.swift)

| Rule | Value | Source |
|---|---|---|
| `RenderTier` | `.proxy` (1536) · `.mediumIfFree` · `.full` | ImageSession.swift:161-169 |
| `HQMode` | `off / auto / on`; default `.auto`; `next` cycles off→auto→on→off; session-only | AppModel.swift:694-720,774 |
| `renderTier(mode, active)` | `!active → .proxy`; `.on → .full`; else `.mediumIfFree` | :769-772 |
| `HQMode.resolve(zoom, threshold, hasSelection, canvasOwnedByProxyTool, cropToolActive)` | `!hasSelection || canvasOwnedByProxyTool → false`; off → false; on → true; auto → `!cropToolActive && zoom ≥ threshold` | :736-749 |
| threshold | `hqAutoZoomThreshold = 2.0` (zoom 1 = fit) | :760 |
| `canvasOwnedByProxyTool` | `testStrip != nil || testStripTask != nil || zonePlacement != nil`; `cropToolActive = toolMode != .none` | :807-814 |
| `canvasZoom` didSet | only in `.auto`, only on a threshold CROSSING; upward swap debounced 350 ms, downward instant | :788-801,822-838 |
| `hqMode` didSet | clears strip + placement; `.off` → `session.releaseEnhancedTiers()`; re-resolve immediately | :774-784 |
| render call | `session.render(settings, uncropped: toolMode != .none, tier: renderTier(hqMode, hqActive), retainFull: hqMode != .off)` | :870-884 |
| LRU | `sessionLRU` holds 2 sessions (active + previous); on open: reuse by URL (move to back) else new; evict oldest beyond 2; `releaseEnhancedTiers()` on every retained session except the one opened; cleared when `folderURL` changes | :342-343,658-674,13-19 |
| Strip / placement entry | an explicit `.on` is demoted to `.off` before building (proxy renders only) | :1012,1124 |

## 6. AppModel engine state machine

### Settings change (`settings` didSet, only when `oldValue != settings`) :89-91,676-685

1. `clearTestStrip()`, `clearZonePlacement()`.
2. Profile editor: `profileDraft = settings.adjustmentsOnly`.
3. `scheduleRender()`.
4. `if !isRestoringSettings { scheduleSave() }` — debounced 1 s, snapshot of settings, skipped in the profile editor :923-932.
5. `if !isRestoringSettings && !isNavigatingHistory { scheduleHistoryCommit() }`.

### Render coalescing (`scheduleRender`) :852-921

Latest-wins: one task in flight; a request during a render sets `renderPending` and the loop
re-renders once with the newest state. Snapshot = `showingBaseline ? baselineSettings() : settings`;
`fineRotation` forced to 0 while `straightenDragValue != nil` or `toolMode == .crop`;
`uncropped = toolMode != .none`. After success: `frameSize/displayAspect/contentWindow/densityRange/
displayImage/histogram` updated, `displayedFineRotation = snapshot.fineRotation`, `statusMessage = nil`;
a 0°-base precompute is scheduled 350 ms later (`prepareStraightenBase`) :179-196. Failure → `statusMessage = "Render failed: …"`.

### Opening a frame (`selection` didSet → `openSelection`) :45-47,610-652

Cancel render; clear strip/placement; a live crop mode is CANCELLED (not committed); `toolMode = .none`;
`showingBaseline = false`; `displayImage = histogram = nil`, `densityRange = 0`; multi-selection ⊇ selection;
`canvasZoom = 1`; HQ re-resolved; session from LRU; `restored = sidecar ?? DefaultProfile.settings`
(profile editor: `profileDraft.keepingGeometry(of: frame)`); `isRestoringSettings = true` around the
assignment (equal settings → `settingsChanged()` called explicitly); then `loadHistory(url)`.

### Tool modes :100-139,290-303

`ToolMode { none, analysisRegion, crop }`. didSet (on change): clear strip + placement, re-resolve HQ,
entering `.crop` snapshots `(fineRotation, cropRect)` for cancel, then `scheduleRender()`.
`cancelCropMode()`: restore the snapshot (label "Cancel crop") and exit with `cropModeCancelled = true`
so the view skips its commit. `commitSelection(rect)`: analysis tool → `analysisRect = rect`,
`analysisRectFineRotation = fineRotation`, label "Analysis region"; crop → `cropRect = rect`, label "Crop";
then `toolMode = .none`.

### Geometry commits :218-264

`commitFineRotation(deg)`: sets `fineRotation`; a committed crop is remapped with
`CropGeometry.remapCrop(crop, from: old rad, to: new rad, frame: frameSize)` (content-preserving).
`clearCropAndStraighten()`: both cleared, label "Crop & straighten cleared" / "Crop cleared" / "Straighten cleared".
`rotateClockwise/Counterclockwise`: `rotation = (rotation ± 90 + 360) % 360`, labels "Rotate 90° CW/CCW"
(while a test strip is up they rotate the LADDER instead, :995-999). `flipHorizontal()` toggles, label "Flip horizontal".

### Straighten preview :143-200

`straightenDragValue` non-nil = live display rotation by `(target − displayedFineRotation)`; first
touch on an image baked at ≠ 0° swaps in the precomputed 0° base (`straightenBase`, keyed by exact
settings with `fineRotation = 0`) if the canvas is plain (no tool, no baseline, no HQ), else renders.

### Baseline hold :1278-1299

`baselineSettings()` = stock `ExposureSettings()` + current `analysisRect, cropRect, rotation,
flipHorizontal`. `setBaselinePreview(on)` clears strip/placement when turning on and re-renders.

### Reset / scratch :939-957 — see controls-spec §14.

### Export batch :1304-1452

`performExport(urls, options)`: save sticky options; flush the open image's sidecar immediately
(not in the profile editor); sequential loop: progress `{done: index, total, currentName}`;
settings = live settings for the open URL, else `sidecar ?? DefaultProfile.settings`; session =
the live session for the open URL else a fresh `ImageSession`; `exportRender` → `Exporter.write(encoded,
options.destinationURL(url), options, source: url)`; failures counted; cancel via task cancellation
("Export cancelled (k of N done)"); final status "Exported N image(s)" / "Exported k of N (f failed)".

### Copy / paste adjustments :1347-1396

`copiedAdjustments` = live settings (or another frame's `sidecar ?? DefaultProfile.settings`).
Paste to the open frame: `settings = source.keepingGeometry(of: settings)`, label "Paste adjustments"
(disarmed if no-op); other frames: sidecar rewritten with `source.keepingGeometry(of: target)`;
refused in the profile editor.

## 7. Test strip state (AppModel.swift:967-1090)

`TestStripState{ image, baseSettings, orientation, preview: (row, col, image)? }`; `testStripBuilding`,
generation counter guards stale builds. `toggleTestStrip()`: clears if up/building; requires session,
selection, `toolMode == .none`, `!showingBaseline`; clears placement; demotes HQ `.on` → `.off`; builds
with a settings snapshot and publishes only if settings/tool/baseline are unchanged. Single click →
`previewTestStripCell` (one `session.render` with the patch's density/grade over `baseSettings`);
click while previewing → back to grid; double-click → `confirmTestStripCell`: `density`+`grade` from
the cell applied to CURRENT settings, label "Test strip pick" (autos untouched). Any settings change,
navigation, tool, baseline or HQ change clears it. Ladder orientation (0…3) is session-sticky.

## 8. Zone placement state (:1097-1273)

`ZonePlacementState{ pins, labels, solution, previewImage, armedZone, solving }`; `maxPins = 3`.
`armZonePlacementTarget(zone)`: requires selection, no tool, no baseline; clears strip; demotes HQ
`.on`; toggles `armedZone`. `addZonePin(u,v)`: sample; target = armed zone, else the predicted zone
snapped to thirds `round(z·3)/3`; append, or replace the nearest pin when at `maxPins`; disarm; re-solve.
`moveZonePin(index,u,v,final)`: re-sample, untargeted pins re-snap, solve only on `final`.
`removeZonePin`. `resolveZonePlacement()`: labels = `zoneRoman(predictedZone(valLuma))` per pin
(without maskVal), `solution = solve(...)`, `previewImage = render(solution.settings)`.
`applyZonePlacement()`: `settings = solution.settings`, label "Zone placement". Escape/any edit clears.

## 9. Densitometer state (Densitometer.swift)

`DensitometerState` adopts the displayed rgba8 bitmap once per render (`adopt`, :33-47) and probes
at normalised `(u, v)`: `x = min(Int(u·w), w−1)`, `y = min(Int(v·h), h−1)`, byte index
`y·bytesPerRow + x·4`, `rgb = bytes/255` (already working-space encoded),
`reading = Densitometry.read(encodedRGB:)` (luma on the ENCODED triplet → `printDensity`, `zone`)
(:71-96). Out-of-range or no bitmap → `reading = nil`. The zone grid (`ZoneGrid.compute`, 24 cells on
the long edge) is rebuilt from the same bytes on adopt and when the overlay toggle changes (:52-69).

## 10. History model (AppModel.swift:351-492; Qt :640-668)

| Rule | Mac | Qt |
|---|---|---|
| Scope | per image URL, session-only (`histories[url]`), stashed/restored on switch | per file path, session-only |
| Seed | `[ "Original conversion" : settings at open ]`, index 0 | `[ "Open" ]` |
| Entry | `{ id, label, settings }` (full settings) | `{ label, settings JSON }` |
| Drag edits | `setControlEditing(true)` cancels the pending commit; on the LAST release (`controlEditCount == 0`) commit once | `sliderReleased` commits |
| Non-drag edits | coalesced: commit 700 ms after the last tick (`scheduleHistoryCommit`), skipped while a drag is held, while restoring, or while navigating history | commit immediately |
| Commit | no-op if `entries[index].settings == settings`; truncate `(index+1)...`; label = `pendingHistoryLabel ?? historyLabel(from: entries[index].settings, to: settings)`; append; `pendingHistoryLabel = nil` | same truncation; label passed by caller |
| Cap | unbounded | 100 (drop oldest) |
| Undo | cancel pending; if uncommitted edit → commit first (so undo steps back exactly one); then `index −= 1`, apply | `jumpHistory(index−1)` |
| Redo | cancel pending; an uncommitted edit is committed and the redo STOPS there (it truncated the tail); else `index += 1`, apply | `jumpHistory(index+1)` |
| Jump | `index = i` (disarms `pendingHistoryLabel`), apply with `isNavigatingHistory = true` (no new commit; render + save still fire) | restore, mark sidecar dirty, render |
| `hasUncommittedEdit` | `entries[index].settings != settings` (keeps Undo enabled at index 0) | — |

### History labels (`historyLabel`, HistoryLabels.swift:7-76)

Diff field by field in this order; 0 changes → "Edit"; 1-2 → joined ", "; ≥3 → "Multiple changes".
`num(v, fmt)` = `String(format:)`, default `"%.2f"`.

| Field | Label + value | Field | Label + value |
|---|---|---|---|
| density | `Brightness %.2f` of `2 − density` | grade | `Grade %.0f` |
| exposureStops | `Exposure %+.2f` | overallContrast | `Contrast %.2f` |
| shadows | `Shadows %.2f` | shadowContrast | `Shadow contrast %.2f` |
| darkShadows | `Dark shadows %.2f` | highlights | `Highlights %.2f` |
| highlightContrast | `Highlight contrast %.2f` | preSaturation | `Pre-saturation %.2f` |
| printSaturation | `Print saturation %.2f` | separationDamping | `Separation damping %.2f` |
| contrastMask | `Contrast mask %.2f` | maskSpacer | `Mask spacer %.2f` |
| hueTrim | `Hue trim %.2f` | vibrance | `Vibrance %.2f` |
| skinProtection | `Skin protection %.2f` | saturation | `Saturation %.2f` |
| redHue | `Red hue %+.2f` | redSaturation | `Red saturation %.2f` |
| yellowHue | `Yellow hue %+.2f` | yellowSaturation | `Yellow saturation %.2f` |
| greenHue | `Green hue %+.2f` | greenSaturation | `Green saturation %.2f` |
| blueHue | `Blue hue %+.2f` | blueSaturation | `Blue saturation %.2f` |
| castRemovalStrength | `Cast strength %.2f` | temp | `Temp %.2f` |
| tint | `Tint %.2f` | colorShadows | `Shadow color` |
| colorMids | `Mid color` | colorHighs | `Highlight color` |
| levelsRed | `Red levels` | levelsGreen | `Green levels` |
| levelsBlue | `Blue levels` | wbCyan/wbMagenta/wbYellow (any) | `WB trim` |
| toe | `Toe %.2f` | toeWidth | `Toe width %.2f` |
| shoulder | `Shoulder %.2f` | shoulderWidth | `Shoulder width %.2f` |
| whitePointOffset | `White point %.3f` | blackPointOffset | `Black point %.3f` |
| autoExposure | `Auto exposure on/off` | autoNormalizeContrast | `Auto contrast on/off` |
| paperDmin | `Paper white on/off` | trueBlack | `True black on/off` |
| rotation | `Rotate` | fineRotation | `Fine rotation %.1f°` |
| flipHorizontal | `Flip` | cropRect | `Crop` / `Crop cleared` (when new is nil) |
| analysisRect or analysisRectFineRotation | `Analysis region` / `Analysis region cleared` | | |

Named labels set via `pendingHistoryLabel` before a mutation: "Rotate 90° CW/CCW", "Flip horizontal",
"Crop", "Crop cleared", "Analysis region", "Analysis region cleared", "Crop & straighten cleared",
"Straighten cleared", "Cancel crop", "Reset all", "Start from scratch", "Test strip pick",
"Zone placement", "Paste adjustments". A named action that changes nothing must disarm the label
(AppModel.swift:943-947 pattern).

## 11. Export (Exporter.swift, ExportSheet.swift)

| Item | Value | Source |
|---|---|---|
| `ExportFormat` | `jpeg` ("JPEG", ext `jpg`) · `tiff16` ("TIFF (16-bit)", ext `tiff`) | Exporter.swift:8-25 |
| `ExportColorSpace` | `sRGB` ("sRGB") · `rommRGB` ("Adobe RGB (wide gamut)" — case name kept for sticky JSON) | :29-42 |
| `ExportOptions` defaults | `format .jpeg`, `jpegQuality 0.92`, `resize false`, `maxLongEdge 3000`, `colorSpace .sRGB`, `useCustomDestination false`, `customDestinationPath nil` | :46-57 |
| `destinationURL(source)` | dir = custom path (if `useCustomDestination && path != nil`) else source's directory; name = source basename without extension + `.jpg`/`.tiff`; overwrites | :61-70 |
| Sticky | UserDefaults `exportOptions` JSON; `loadSticky` falls back to defaults on any decode failure | :72-83 |
| Sheet ranges | quality slider 0.5…1.0 shown as `%.0f%%`; long edge text field + stepper 256…20000 step 500; destination picker "Next to originals"/"Folder…"; Export disabled when custom destination chosen but no path; Cancel = Esc, Export = Return | ExportSheet.swift:38-101 |
| Write | resize first if `resize && maxLongEdge ≥ 16` via `downsampled(maxLongEdge:)` on the ENCODED image; sRGB: 16-bit working-space image converted by ColorSync to sRGB at 8 (JPEG) / 16 (TIFF) bits; Adobe: tag directly; JPEG quality = `jpegQuality`; TIFF compression LZW (5); orientation tag `.up`; EXIF (minus pixel dims), TIFF Make/Model/DateTime + Software "SwiftInvert", GPS copied from the source | Exporter.swift:114-157 |
| Bridge export | options JSON `{"colorspace":"srgb"|"adobe","maxLongEdge":N}` (absent/0 = full size); analysis from a fresh 1536 preview decode oriented (rotation, flip only); full decode oriented+fine-rotated+cropped; sRGB leg in-kernel (`srgbEncode`); resize after encode; `si_export_render` → RGBA8 (alpha 255); `si_export_tiff` → 16-bit baseline TIFF ICC-tagged (sRGB or Adobe RGB 1998) | CoreBridge.swift:573-699 |
| Qt dialog sticky | `export/format 0`, `colorspace 0`, `quality 92` (50…100), `resize true`, `maxLongEdge 3000`, `toFolder false`, `folder` | qt/main.cpp:1280-1353 |

## 12. CoreBridge contract (CoreBridge.swift, qt/swiftinvert_core.h)

Conventions: settings = sidecar-format JSON (missing keys = defaults; empty string = defaults);
every returned pointer malloc'd → `si_free`; failures return 0/NULL and set `si_last_error`;
thread-safe (registry lock; pipeline serialises) (:6-13; header :3-8).

| Function | Semantics | Source |
|---|---|---|
| `int64 si_open(path)` | native preview decode → `Session` (proxy = `downsampled(1536)` when long edge > 1536, medium = native buffer if `w·h ≤ 20 MP` and it was downsampled) → handle ≥ 1; 0 on failure | :78-89,285-304 |
| `si_close(h)` | drop the session | :306-311 |
| `si_size(h, &w, &h)` | PROXY dims; 1/0 | :313-325 |
| `uint8* si_render(h, json, srgb_display, tier, uncropped, &w, &h, hist[1024]|NULL)` | analysis (keyed `{rotation, flip, cropRect, analysisRect}`, meter = proxy oriented WITHOUT fine rotation) → derive → source for tier (oriented+fineRotation then crop unless `uncropped`, cached per tier by `SourceKey{rotation, flip, fineRotation, cropRect-or-nil}`) → mask plane (proxy, key = source key + spacer + analysis key) → `renderDisplay(srgbDisplay:)` → RGBA8; tier 0 proxy, 1 medium (falls back to PROXY when not retained), 2 full (lazy decode, kept for the session) | :92-232,343-394 |
| `int32 si_render_into(h, json, srgb, tier, uncropped, dest, cap, &w, &h, hist)` | builds the source first, sets `w,h`; returns −1 if `dest == NULL` or `cap < w·h·4` (caller resizes and retries; source stays cached); writes RGBA8 straight from the GPU readback; 1 ok, 0 error | :404-456 |
| `char* si_session_info(h)` | JSON `{width, height, densityRange, defaultGradeRange, anchor, character?, castConfidence?}` from the cached analysis (or default-settings analysis) | :462-485 |
| `si_sidecar_load(path)` / `si_sidecar_save(path, json)` | §2 | :503-541 |
| `uint8* si_thumbnail(path, &len)` | embedded camera JPEG bytes | :545-559 |
| `char* si_default_settings()` | stock `ExposureSettings` as sorted-key JSON | :563-569 |
| `si_export_render` / `si_export_tiff` | §11 | :645-699 |
| `si_device_name()` | GPU name or "" | :702-708 |

Bridge-vs-Mac differences the Rust session should NOT copy: the bridge never releases the full
tier, its medium fallback is proxy (Mac: full), and its meter ignores `analysisRectFineRotation`.

## 13. Numbers at a glance

| Constant | Value | Source |
|---|---|---|
| Proxy long edge | 1536 | ImageSession.swift:265 |
| Medium tier budget | 20 000 000 px | :216 |
| Fine-rotation apply threshold | `|deg| > 0.005` | :75,94 |
| Alias threshold | both angles `≤ 0.005` | :53-55 |
| Analysis buffer | 0.10 | ExposureSettings.swift:396 |
| Levels clamp / max | 0.02 / 8 | :615-617 |
| LRU sessions | 2 | AppModel.swift:343 |
| Sidecar save debounce | 1 s | :928 |
| History coalesce | 0.7 s | :423 |
| HQ auto threshold / upward debounce | 2.0× / 350 ms | :760,833 |
| Straighten-base refresh delay | 350 ms | :904 |
| Straighten commit precision | 0.1° | CropRotationSection.swift:102 |
| Zone pin patch | 5×5 (radius 2) | ImageSession.swift:511 |
| Zone target snap | thirds | AppModel.swift:1151,1201 |
| Max pins | 3 | ZonePlacement.swift:26 |
| Test strip | 5×5, densities [1.6,1.3,1.0,0.7,0.4], grades [75,95,115,135,155] | TestStrip.swift:16-18 |
| Sidecar version | 1 | SidecarStore.swift:10 |
| JPEG quality default | 0.92 | Exporter.swift:48 |
| Resize default / long edge | off / 3000 | :49-50 |
