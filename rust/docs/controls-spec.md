# Controls spec — the adjustments sidebar, window chrome and library

Source of truth: the Mac app (`Sources/SwiftInvert/*.swift`), cross-checked against the Qt
mirror (`qt/main.cpp`, `qt/imageview.cpp`). Every value cites `file:line`. Where the Mac and
Qt disagree the Mac wins and the difference is listed in §15. Settings field names are
`ExposureSettings` properties = sidecar JSON keys (see session-spec §2).

## 1. Sidebar frame

| Item | Value | Source |
|---|---|---|
| Pane width | 290 pt, content width = 290 − legacy-scroller gutter (17 pt when `preferredScrollerStyle == .legacy`, else 0); never sized to scroller visibility | ControlsSidebar.swift:9,18-23,353-354 |
| Top header | "Adjustments" collapse chevron (rotates 0°→90° open) + "Reset All" button (small) | :38-57 |
| Vertical order | header → [Adjustments: pinned HistogramView, scrolling control groups] → resize handle → Crop & Rotation section → resize handle → History panel | :66-105 |
| Scrolling group order | Pre-process, Print, Exposure, Color, Color Mixer, Color Grading, Tone, "Export…" button, status text | :146-341 |
| Group spacing | 16 between GroupBoxes; 10 (8 in Pre-process) between rows; group padding 6; outer padding 12 | :145-147,174,343 |
| Adjustments min height | 210 (histogram floor) | :79,137-141 |
| History height | default 150, drag range 40…600 (`historyHeight`), clamped to `available − above − cropH` with above = 370 open / 130 collapsed, cropH = 175 open / 40 collapsed | :28,84,137-141 |
| Collapse persistence | `@AppStorage` `adjustmentsCollapsed`, `cropRotationCollapsed`, `historyCollapsed` (all default false) | :25-27 |
| Collapse animation | easeOut 0.12 s | :128 |
| Drag → history hold | every slider reports edit begin/end via `controlEditingChanged` → `AppModel.setControlEditing` | :127,396-405 |

## 2. Slider widget contract (`LabeledSlider`, `GradientSlider`)

| Rule | LabeledSlider (ControlsSidebar.swift:407-445) | GradientSlider (ColorGradingSection.swift:78-147) |
|---|---|---|
| Row | caption (`.caption`) · reset-⨯ · spacer · value (`.caption.monospacedDigit`) over the slider | same |
| `isChanged` | `abs(value − default) > 1e-9` → ⨯ shown, value text primary (else secondary) | same (:89) |
| Reset-⨯ | sets `value = defaultValue` (no drag bracket → coalesced history commit) | same (:96-105) |
| Double-click | on the slider resets to default (:441) | same (:141) |
| Value format | per-control `format` (table below) | always `"%.2f"` (:107) |
| Step | none (continuous `Slider`), `.controlSize(.small)` | continuous; knob 14 px, track Capsule 8 px tall, linear gradient of `colors` leading→trailing (:113-123); fraction = (x − 7)/(w − 14) clamped (:133) |
| Drag bracket | `Slider(onEditingChanged:)` → `controlEditingChanged(true/false)` (:439) | `DragGesture(minimumDistance: 0)` first change → true, end → false (:127-140) |
| Wheel | the wheel never changes a slider; Qt swallows wheel events over any slider and re-sends them to the controls scroll viewport (qt/main.cpp:1529-1545). The Mac's canvas pan monitor consumes wheel only over the canvas (ScrollWheelCatcher.swift:378-396) | |
| Qt quantisation | Qt sliders are integer ticks `(max−min)/step` (qt/main.cpp:967-973); release commits history, programmatic sets commit immediately (:987-993) | |

## 3. Control table (on-screen order)

Columns: label · field · min…max · value format (Mac) / step+decimals (Qt) · default (= reset-⨯) ·
notes. All sliders are continuous on the Mac.

### Pre-process (ControlsSidebar.swift:146-175)

| Row | Kind | Behaviour | Source |
|---|---|---|---|
| Crop for Analysis | tool button (`viewfinder.rectangular`) + ⨯ when `analysisRect != nil` | toggles `toolMode .analysisRegion`; ⨯ sets `analysisRect = nil`; caption "Drag on the image to select the area." while active | :148-157,366-389 |
| View Original | hold button (`eye`), enabled iff selection | pressed → `setBaselinePreview(true)`, released → false | :158-164,450-463 |
| Start from Scratch | button (`arrow.counterclockwise`), enabled iff selection | `resetToStock()` = stock `ExposureSettings()` | :165-172 |

### Print (:177-224)

| Label | Field | Range | Format / Qt step | Default | Notes |
|---|---|---|---|---|---|
| Auto exposure | `autoExposure` | toggle | — | true | :179 |
| Brightness | `density` (displayed `2 − density`; set `density = 2 − v`) | −3…4 | `%.2f` / 0.01, 2 | 1.0 (density 1.0) | right = brighter :183-186,359-363 |
| Auto contrast | `autoNormalizeContrast` | toggle | — | true | :187 |
| Grade (ISO R) | `grade` | 50…180 | `%.0f` / 1, 0 | 115 | :189-192 |
| *Negative character row* | read-out | — | — | — | §4; sits under Grade :196 |
| Contrast Mask | `contrastMask` | −0.5…0.5 | `%+.2f` / 0.01, 2 | 0 | not display-inverted; right = + = softer :204-207 |
| Mask Spacer | `maskSpacer` | 2…6 | `%.1f%%` / 0.1, 1, suffix "%" | 4 | **disabled when `contrastMask == 0`** :208-212 |
| Test Strip | button (`square.grid.3x3`), active style while strip showing/building, enabled iff selection | — | — | — | `toggleTestStrip()`; ⇧T :213-221 |

### Exposure (:226-258)

| Label | Field | Range | Format | Default |
|---|---|---|---|---|
| Exposure (stops) | `exposureStops` | −2…2 | `%+.2f` | 0 |
| Contrast | `overallContrast` | −1…2 | `%.2f` | 0 |
| Shadows | `shadows` | −2…2 | `%.2f` | 0 |
| Shadow contrast | `shadowContrast` | −3…6 | `%.2f` | 0 |
| Dark shadows | `darkShadows` | −2…2 | `%.2f` | 0 |
| Highlights | `highlights` | −1…1 | `%.2f` | 0 |
| Highlight contrast | `highlightContrast` | −1…1 | `%.2f` | 0 |

Qt: all step 0.01, 2 decimals (qt/main.cpp:1141-1151).

### Color (:260-305)

| Label | Field | Range | Format | Default | Notes |
|---|---|---|---|---|---|
| Pre-saturation | `preSaturation` | 0.5…2.0 | `%.2f` | 1.15 | |
| Cast strength | `castRemovalStrength` | 0…2 | `%.2f` | 0.5 | `Divider` after |
| Hue Trim | `hueTrim` | −30…30 | `%.1f°` (Qt 0.1, 1, "°") | 0 | `Divider` after |
| Print Saturation | `printSaturation` | 0…2 | `%.2f` | 1.0 | |
| Separation Damping | `separationDamping` | 0…1 | `%.2f` | 0 | **disabled when `printSaturation == 1.0`** :290 |
| Vibrance | `vibrance` | 0…2 | `%.2f` | 1.0 | |
| Saturation | `saturation` | 0…2 | `%.2f` | 1.0 | |
| Skin Protection | `skinProtection` | 0…1 | `%.2f` | 0.5 | |

`wbCyan/wbMagenta/wbYellow` have NO UI (fields remain in sidecars) :262-263.

### Color Mixer (ColorMixerSection.swift)

| Item | Value | Source |
|---|---|---|
| Band picker | segmented, labels "Red" "Yellow" "Green" "Blue"; default `.red` (Qt labels "R/Y/G/B", qt/main.cpp:1194) | :11-16,72,77-82 |
| Hue | field per band `redHue/yellowHue/greenHue/blueHue`; range −1.5…1.5; default 0; GradientSlider | :84-87 |
| Saturation | `redSaturation/…/blueSaturation`; range 0…2; default 1.0; GradientSlider | :88-91 |
| Hue track colours [−, band, +] (sRGB floats) | red: (0.85,0.2,0.5) (0.85,0.2,0.2) (0.9,0.55,0.15) · yellow: (0.9,0.55,0.15) (0.88,0.8,0.2) (0.6,0.82,0.25) · green: (0.6,0.82,0.25) (0.25,0.72,0.3) (0.15,0.72,0.6) · blue: (0.15,0.68,0.8) (0.25,0.45,0.88) (0.55,0.3,0.85) | :40-67 |
| Qt hex equivalents | red `#D93380 #D93333 #E68C26` · yellow `#E68C26 #E0CC33 #99D140` · green `#99D140 #40B84D #26B899` · blue `#26ADCC #4073E0 #8C4DD9` | qt/main.cpp:1176-1181 |
| Saturation track | `[Color(white: 0.4), hueColors[1]]` (Qt `#666666` → band colour) | :69; qt:1183 |
| Band switch | restyles both tracks; values re-read from the selected band's fields | qt:1198-1203 |

### Color Grading (ColorGradingSection.swift)

| Label | Field | Range | Default | Track colours [−, mid, +] | Qt hex |
|---|---|---|---|---|---|
| Temp | `temp` | −1…1 | 0 | (0.25,0.35,0.85) · white 0.4 · (0.75,0.7,0.15) | `#4059D9 #666666 #BFB326` |
| Tint | `tint` | −1…1 | 0 | (0.2,0.75,0.25) · white 0.4 · (0.7,0.2,0.75) | `#33BF40 #666666 #B333BF` |
| *Band picker* | segmented "Shadows" "Mids" "Highs"; **default `.mids`** (Qt initial index 1) | | | | :24; qt:1211 |
| R ↔ C | `colorShadows/colorMids/colorHighs[0]` | −1…1 | 0 | (0.85,0.2,0.2) · white 0.4 · (0.1,0.75,0.8) | `#D93333 #666666 #1ABFCC` |
| G ↔ M | `…[1]` | −1…1 | 0 | (0.2,0.75,0.25) · white 0.4 · (0.8,0.2,0.7) | `#33BF40 #666666 #B333BF` |
| B ↔ Y | `…[2]` | −1…1 | 0 | (0.25,0.35,0.85) · white 0.4 · (0.8,0.75,0.2) | `#4059D9 #666666 #CCBF33` |

Sources: :26-30 (colours), :35-62 (sliders), :68-73 (component binding); qt/main.cpp:1223-1234.

### Tone (ControlsSidebar.swift:311-333)

| Label | Field | Range | Format (Qt step, decimals) | Default |
|---|---|---|---|---|
| Toe | `toe` | −1…1 | `%.2f` | 0 |
| Shoulder | `shoulder` | −1…1 | `%.2f` | 0 |
| True black | `trueBlack` | toggle | — | true |
| White point | `whitePointOffset` | −0.3…0.3 | `%.3f` (0.001, 3) | 0 |
| Black point | `blackPointOffset` | −0.3…0.3 | `%.3f` (0.001, 3) | 0 |

`paperDmin`, `toeWidth`, `shoulderWidth` have no UI anywhere (history labels only, HistoryLabels.swift:50-53,58).

### Tail (:335-341)

"Export…" button → `requestExportCurrent()`, disabled while `isExporting || selection == nil`;
then `statusMessage` as `.caption` secondary text.

## 4. Negative character row (NegativeCharacterRow.swift, Densitometry.swift)

| Rule | Value | Source |
|---|---|---|
| Text | left "Negative" (10 pt secondary); right `String(format: "%.2f · %@", densityRange, label)` monospaced 10 pt, or "—" (tertiary) when nil | NegativeCharacterRow.swift:12-19,28-31 |
| Input | `AppModel.densityRange` = `RenderOutput.densityRange` = `analysis.baseBounds.luminanceDensityRange` (pre-offset: `lumaR·|Δr| + lumaG·|Δg| + lumaB·|Δb|`), 0 until first render | AppModel.swift:213,890; BoundsAnalysis.swift:15-17 |
| Character | nil if range ≤ 0 or non-finite; `ratio = range / CurveLogic.defaultGradeRange` (= `K.autoGradeTarget·K.autoGradeNominalRatio·K.autoGradeNominalRange`); `< 0.80` → "flat (≈N−1)", `> 1.25` → "contrasty (≈N+1)", else "normal" | Densitometry.swift:96-120; CurveLogic.swift:48-50 |
| Qt | `"negative: %.2f · <character>   cast confidence %.2f"` from `si_session_info` (adds cast confidence; Mac does not) | qt/main.cpp:1512-1527 |

## 5. Histogram panel (HistogramView.swift)

| Rule | Value | Source |
|---|---|---|
| Size | chart height 128, rounded 6, background black @0.88; "—" tertiary when no bins; bins valid only when `count == 1024` | :27,41,53,32-34 |
| Bar height | `f = count / max(maxCount,1)`; linear → `f`; log → `log1p(f·100000)/log1p(100000)`; **peak-normalised, fixed constant** (never `log1p(count)/log1p(max)`) | :63,76-80 |
| Scale toggle | footer button "log"/"lin", default log | :17,246-250 |
| Gridlines | verticals at 1/4, 2/4, 3/4 width, white @0.08, 1 px | :106-111 |
| Channels | filled closed paths, blend `.screen`: R (0.85,0.1,0.1)@0.85, G (0.1,0.8,0.15)@0.85, B (0.15,0.25,0.95)@0.85; luma (bins 768…1023) stroked white @0.75, 1 px, normal blend; x = `w·i/255` | :90-122 |
| Clip indicators | per end: fraction = `bins[c·256 + (0|255)] / Σ bins[0..<256]` (R total); shown when any channel > 0.001; triangle 8 pt coloured per channel (1 if > 0.001 else 0.25); tooltip `"Shadow clipping — R %.1f%%  G %.1f%%  B %.1f%%"` / "Highlight clipping — …" | :128-172 |
| Hover read-out | bin = `clamp(Int(x/width·255), 0, 255)`; text `"L %d  ·  R %.1f  G %.1f  B %.1f"` (percent of R total); capsule black @0.7, white @0.9, 8 pt, top-aligned | :271-306 |
| BP/WP handles | travel = 0.35·width; full travel = 0.3 D; `blackX = clamp(−bp/0.3,0,1)·travel`, `whiteX = width − clamp(wp/0.3,0,1)·travel`; drag Δ = `(dx/travel)·0.3`: black `bp = clamp(start − Δ, −0.3, 0)`, white `wp = clamp(start − Δ, 0, 0.3)`; drag brackets `setControlEditing`; double-click handle → 0; 16 pt hit width, 1.5 px line white @0.55 + 7 pt arrow | :21-22,176-227 |
| Footer | "Shadows" … "Highlights" labels 8 pt tertiary; reset-⨯ beside each when its offset ≠ 0 | :231-265 |
| Double-click chart | opens the "interactive-histogram" window | ControlsSidebar.swift:73-76 |

### Interactive histogram window (InteractiveHistogram.swift)

Channel picker Red/Green/Blue (default red) :32-41; constants `gap = 0.01` (min spacing both
axes), `grabTolerance = 0.015` :28-31; endpoint clamp `ExposureKernel.levelsClamp = 0.02`, max
anchors `levelsMaxPoints = 8` (ExposureSettings.swift:615-617); new anchor input clamped to
`[prev.x+gap, next.x−gap] ∩ [0.02, 0.98]` :176-183; output clamped to
`[prev.y+gap, next.y−gap]` :193-194. Anchors = `levelsRed/Green/Blue` `[SIMD2(input, output)]`.

## 6. Crop & Rotation section (CropRotationSection.swift)

| Item | Value | Source |
|---|---|---|
| Header | "Crop & Rotation" chevron collapse (`cropRotationCollapsed`) | :26,40-57 |
| Crop or Straighten | bordered button (`crop`), tinted accent while `toolMode == .crop`; toggles the mode; ⨯ beside it when `cropRect != nil || |fineRotation| > 1e-9` → `clearCropAndStraighten()` | :117-134 |
| Straighten | shown ONLY while `toolMode == .crop`; range −45…45; label `"%.1f°"`; ⨯ when ≠ 0 → `commitFineRotation(0)`; editing-begin enters crop mode and seeds `straightenDragValue`; release → `commitFineRotation(round(v·10)/10)`; hover → `prepareStraightenBase()` | :61-112,139-141 |
| Show grid lines | toggle (`showGridLines`, default false) + picker (`gridLineType`, default "thirds"): "Rule of thirds" / "Phi (golden ratio)" / "Grid" / "Diagonals" | :27-28,143-153 |
| Grid geometry | thirds at 1/3, 2/3; phi at 0.38196601 and 1−that; grid 8×8; diagonals corner-to-corner; stroke white @0.55 1 px over black @0.25 2.5 px; always shown while straightening | :162-201; DetailView.swift:328-332 |
| Disabled | whole content when no selection | :156 |

## 7. History panel (HistoryPanel.swift)

Header "History" chevron (`historyCollapsed`) + undo/redo icon buttons (disabled by
`canUndo`/`canRedo`; shortcuts live on the menu) :267-302. List newest-first, row = index (9 pt
tertiary, 16 pt wide) + label (`.caption`); current row accent @0.22 background; rows after
current (redo) tertiary; click → `jumpToHistory(index)` :304-342. Model rules: session-spec §10.

## 8. Menu bar (SwiftInvertApp.swift:40-151)

| Menu | Item | Shortcut | Action / enabled |
|---|---|---|---|
| File | Open Folder… | ⌘O | `chooseFolder()` |
| File | Choose Default Settings… | — | opens window `choose-default-settings` |
| File | Export… | ⌘E | `requestExportFromMenu()`; disabled no selection or exporting |
| File | Show in Finder | ⇧⌘R | reveal selection |
| Edit | Undo Edit | ⌘Z | `undo()`; enabled if `canUndo || hasUncommittedEdit` |
| Edit | Redo Edit | ⇧⌘Z | `redo()`; enabled if `canRedo` |
| Edit | Copy Adjustments | ⇧⌘C | needs selection |
| Edit | Paste Adjustments | ⇧⌘V | needs selection + clipboard |
| Edit | Paste Adjustments to Selection | ⌥⇧⌘V | needs multi-selection + clipboard |
| Edit | Reset All | ⌥⌘R | `resetSettings()` |
| View | Show Library | ⇧⌘L | toggle `libraryVisible` |
| View | Show Grid Lines | ⇧⌘G | toggle `showGridLines` |
| View | Zone Overlay | (⇧Z via key monitor) | toggle `showZoneOverlay` |
| View | HQ Preview | inline picker Off/Auto/On | `hqMode`; disabled no selection |
| View | Cycle HQ Preview | ⇧⌘P | `hqMode = hqMode.next` |
| Image | Previous / Next Image | (←/→ via key monitor) | `selectAdjacent(∓1)`; disabled if no files or export sheet up |
| Image | Rotate Left / Right | ⌘[ / ⌘] | `rotateCounterclockwise()/rotateClockwise()` |
| Image | Flip Horizontal | ⇧⌘H | `flipHorizontal()` |
| Image | Crop or Straighten | ⌘K | toggle `toolMode .crop` (checkmark) |
| Image | Clear Crop & Straighten | — | enabled if crop or angle set |
| Image | Crop for Analysis | ⇧⌘K | toggle `toolMode .analysisRegion` |
| Image | Clear Analysis Region | — | label "Analysis region cleared", `analysisRect = nil` |

Windows: "Interactive Histogram" (id `interactive-histogram`), "Choose Default Settings"
(id `choose-default-settings`, content-sized), "Default Settings Editor" (per
`ProfileEditRequest`) :155-169. Main window min 1000×700 :328.

## 9. Window key monitor (SwiftInvertApp.swift:236-313)

Local keyDown monitor, only for events in this model's window and only while no export sheet:

| Key (keyCode) | Condition | Action |
|---|---|---|
| ⇧Z (6 + shift only) | not editing text, has selection | toggle zone overlay |
| ⇧T (17 + shift only) | same | `toggleTestStrip()` |
| Esc (53) | test strip showing/building | `clearTestStrip()` |
| Esc | zone placement up | `clearZonePlacement()` |
| ↑ ↓ ← → (126/125/123/124) | not editing text, files non-empty | `selectAdjacent(−1 for ↑←, +1 for ↓→)` |
| Esc, no tool | profile-editor window | `profileEditorEscape = true` |
| Esc, `toolMode == .crop` | | `cancelCropMode()` |
| Esc / Return (36) / Enter (76), other tool | | `toolMode = .none` (Return in crop = commit via mode exit) |

"Editing text" = first responder is `NSTextView`. `onExitCommand` mirrors Esc :214-222.

## 10. Canvas control bar (DetailView.swift:102-191)

Left group (disabled without selection): rotate ⟲ (`rotate.left`) → `rotateCounterclockwise()`,
rotate ⟳ → `rotateClockwise()`, flip → `flipHorizontal()`, **HQ badge**, **Zones** badge.
Centre: `DensitometerReadout` (when selection). Right: "Canvas" + three colour swatches.

| HQ badge state | fill | stroke | text |
|---|---|---|---|
| off | primary @0.06 | secondary @0.4 | primary |
| auto | accent @0.18 | accent @0.55 | primary |
| on | accent @0.85 | accent | white |

Badge click = `hqMode.next` (off→auto→on→off); help strings :548-560 (threshold 2×). Zones badge:
fill accent @0.3 / stroke accent when on, else primary @0.06 / secondary @0.4; toggles
`showZoneOverlay` :138-154. Swatch: circle 16 pt, stroke accent 2 px when selected else secondary
@0.4 1 px :170-185.

| Canvas colour | value | label | Qt |
|---|---|---|---|
| `gray` | white 0.5 | "Gray" | (128,128,128) |
| `veryDarkGray` (default) | white 0.12 | "Very dark gray" | (31,31,31), default index 1 |
| `black` | black | "Black" | (0,0,0) |

AppModel.swift:266-288; qt/main.cpp:868-900. Persisted as UserDefaults `canvasColor` (raw
string) / QSettings `canvasColor` (index).

**Densitometer read-out** (DensitometerReadout.swift:58-87): `ZoneStrip` (11 cells 9×10 pt,
shades `encoded(ofZone:)`, probed cell outlined accent 1.5 px, armed cell orange; strip @0.35
opacity when idle) + monospaced 10 pt text: `"D %.2f · Zone %@"` (`Densitometry.zoneRoman`),
"hover to meter" when nil, `"Zone %@ → click photo"` while armed; fixed width 210. Probe maps the
hover point `u = x/imageFrame.width, v = y/imageFrame.height` and is active only in the plain
presentation (no tool, no straighten drag, no 0°-base preview, no strip, no placement)
DetailView.swift:236-240,311-321. Strip cell click → `armZonePlacementTarget(zone)`.

## 11. Canvas (DetailView.swift)

| Rule | Value | Source |
|---|---|---|
| Fit | aspect-fit inside container with 12 pt inset, from `displayAspect` (not bitmap px) | :200-209,219-223 |
| Zoom | `MagnifyGesture`: `zoom = clamp(baseZoom·magnification, 0.5, 8)`; on end, `zoom ≤ 1` animates pan to zero (easeOut 0.15) | :499-508 |
| Pan | drag only while `zoom > 1`, `pan = basePan + translation`; two-finger scroll wheel also pans (`pan += (dx, dy)`), only when no tool and `zoom > 1` | :384-390,510-517 |
| Double-click | `resetZoom()` (zoom 1 = fit, pan 0), animated 0.15 s | :392-394 |
| Reset zoom | on selection change and on entering any tool mode | :94-98,400-406 |
| Zoom report | `model.canvasZoom = zoom` (drives HQ auto) | :399 |
| Crop mode | fits the rotated bounding box of `frameSize`, scaleEffect 1, no pan; `CropBoxOverlay` (box = DetailView state, nil = follow committed) | :244-259,363-373 |
| Analysis tool | `SelectionOverlay` over the fitted frame: dims black @0.25, existing rect accent 1.5 px, drag rect white; default shown rect = centred 80 % (`defaultAnalysisBuffer 0.10`); commit needs width,height > 0.01; caption "Drag to select — Esc to cancel" | :374-378,487-497,565-631 |
| Overlays | "Analyzing…" pill bottom-left when `isAnalyzing`; export progress pill ("Exporting n of N", name, cancel ⨯); "Original conversion" capsule top while baseline; "Developing…" / "Couldn't develop this image" / "No image selected" states | :42-56,58-93,433-443 |
| Commit crop (mode exit) | constrain box; full-frame if `|center| < 0.005·inscribed.x` and both extents within 1 % → crop cleared ("Crop cleared"), else `cropRect` ("Crop") | :450-476 |

Qt canvas (qt/imageview.cpp): wheel zoom `factor = 1.0015^angleDelta`, zoom clamped 1…12, anchored
so the image point under the cursor stays put (:12,270-289); drag pans when zoom > 1 with pan
clamped to the overflow (:100-115,161-196); double-click toggles fit ↔ 100 % (`1/fit`, capped 12)
around the cursor (:248-266); tools force zoom 1 (:54-57).

## 12. Library (LibraryView.swift, AppModel.swift)

| Rule | Value | Source |
|---|---|---|
| Header | folder name (or "Library"), middle-truncated, spinner while scanning, folder button → `chooseFolder()`, sidebar-toggle button | LibraryView.swift:22-50 |
| Width | `libraryWidth` default 320, splitter drag clamped 200…560 (global-space drag), 7 pt handle | SwiftInvertApp.swift:175,332-352 |
| Tree scan (`buildTree`) | depth ≤ 8; `skipsHiddenFiles`; packages skipped; files = `RawDecoder.isRawFile` (ext set RawDecoder.swift:41-45); files and dirs each sorted `localizedStandardCompare` (natural); node dropped when no files and no surviving subfolders; `totalCount = files + Σ sub.totalCount` | AppModel.swift:581-600 |
| Flatten | depth-first: a folder's own files, then subfolders in order → `files`, `fileIndex` 1-based | :604-608,565-569 |
| After scan | selection kept if still present else first file; `selection == nil` → first | :571-574 |
| Folder row | chevron + `folder.fill` + name (callout medium) + `totalCount` (caption2 mono, tertiary); indent 14 pt/depth; click toggles `collapsedFolders` (session only on the Mac; Qt persists per root under QSettings `collapsed/<root with / → \|>` as a list of "a/b/c" identities) | LibraryView.swift:103-154; qt/main.cpp:278-316 |
| Qt row text | `"name — N"`, enabled not selectable | qt/main.cpp:265 |
| Grid | adaptive columns 120…180, spacing 8; cell aspect 3:2 fill, rounded 6; halo accent 2 px (3 px when current); index badge 9 pt bold on black @0.55 capsule bottom-left; hover help = filename | LibraryView.swift:11,166-197 |
| Thumbnail | embedded camera JPEG, max pixel 384 with EXIF transform, cached in memory | ThumbnailStore.swift:16-25 |
| Gestures | click → `select(url, additive: false)`; ⌘-click → additive (removes if already in a >1 multi-selection); ⇧-click → `selectRange(to:)` contiguous in `files` order from `selection` | LibraryView.swift:201-209; AppModel.swift:63-87 |
| Context menu | "Export Image…"/"Export N Images…" (selection-aware), Copy Adjustments (from clicked frame), Paste Adjustments [to N Images], Show in Finder | LibraryView.swift:213-243 |
| Keyboard | ↑/← previous, ↓/→ next (flat order); selection scrolls into view | SwiftInvertApp.swift:288-295; LibraryView.swift:73-77 |
| Sticky folder | UserDefaults `libraryFolder` written on every change, restored at launch if it exists | AppModel.swift:13-19,535-541 |

## 13. Persisted keys

| Store | Key | Type / default | Source |
|---|---|---|---|
| UserDefaults | `libraryFolder` | path string | AppModel.swift:15 |
| UserDefaults | `canvasColor` | "gray"/"veryDarkGray"/"black", default veryDarkGray | :284-288 |
| UserDefaults | `exportOptions` | JSON `ExportOptions` | Exporter.swift:72-83 |
| UserDefaults | `settingsProfiles`, `activeProfileID` | JSON `[SettingsProfile]`, UUID string | ProfileStore.swift:78-87,119-124 |
| UserDefaults | migration | if `libraryFolder` unset, copy `libraryFolder/canvasColor/exportOptions` from suite "NegSwift" | AppModel.swift:513-523 |
| @AppStorage | `adjustmentsCollapsed` `cropRotationCollapsed` `historyCollapsed` (false), `historyHeight` (150) | ControlsSidebar.swift:25-28 |
| @AppStorage | `libraryVisible` (true), `libraryWidth` (320), `showGridLines` (false), `gridLineType` ("thirds"), `showZoneOverlay` (false) | SwiftInvertApp.swift:8-10,175-177 |
| Qt QSettings ("SwiftInvert","qt") | `libraryFolder`, `canvasColor` (int), `collapsed/<root>`, `export/format|colorspace|quality|resize|maxLongEdge|toFolder|folder` | qt/main.cpp:294,315,323,869,1287-1353 |

Session-only (never persisted): `hqMode` (default `.auto`), test strip, zone placement, history.

## 14. Reset All / View Original / Start from Scratch semantics

- **Reset All** (header button, ⌥⌘R): `settings = DefaultProfile.settings` (the active profile,
  stock geometry included — crop/rotation/analysis rect are reset too, despite the button
  tooltip "keeps crops"); history label "Reset all", disarmed if nothing changed. AppModel.swift:939-948.
- **Start from Scratch**: `settings = ExposureSettings()` (stock, cancels the profile); label
  "Start from scratch". :952-957.
- **View Original** (hold): renders `baselineSettings()` = stock adjustments + current
  `analysisRect/cropRect/rotation/flipHorizontal` (NOT `fineRotation`), badge "Original
  conversion"; clears strip/placement on press. :1278-1299.

## 15. Qt mirror differences (qt/main.cpp `makeControlsPanel` :1071-1266)

Ranges, steps, defaults, direction conventions, disabled rules and gradient colours match the
Mac table exactly. Differences to be aware of:

1. Pre-process group is "View Original (hold)", "Crop && Straighten" (checkable, Ctrl+K),
   "Clear Analysis Region" — no Crop-for-Analysis tool row (the analysis tool is a menu action
   Ctrl+Shift+K), no Start from Scratch, no Test Strip, no Export… button in the panel (:1096-1117).
2. No Crop & Rotation section; the straighten slider lives in a crop bar with Apply/Clear/Cancel
   (:911-944); no grid-lines toggle.
3. Negative row text differs (§4). History list is a fixed 110 px list inside the panel; the
   first entry is labelled "Open" (Mac: "Original conversion"); cap 100 entries (:651).
4. Mixer picker labels "R/Y/G/B".
5. Shortcuts: Ctrl+O, Ctrl+K, Ctrl+Shift+K, Ctrl+Z/Ctrl+Shift+Z (jumpHistory ±1 — no flush of
   uncommitted edits), Ctrl+E, Ctrl+[ / Ctrl+], Ctrl+Shift+H, Ctrl+Shift+P, Escape; arrows in
   `keyPressEvent` (:764-798,473-480).
6. Canvas zoom model is wheel-based (§11) where the Mac is pinch-based; Qt double-click toggles
   fit ↔ 100 %, the Mac double-click only resets to fit. The Rust UI should take the Qt
   gestures (wheel-to-cursor, drag pan, double-click fit↔100 %) plus the Mac's pinch — PLAN §6 U1.
7. Tool canvas: Qt analysis tool renders uncropped AND substitutes `fineRotation 0`, writing
   `analysisRectFineRotation = 0` (:716-722); the Mac analysis tool renders at the current angle and
   stores `analysisRectFineRotation = fineRotation` (AppModel.swift:290-303). See session-spec §4.
