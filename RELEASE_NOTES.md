# Shade Editor 0.17.1

- Wire the previously-added Export Queue and TIFF Inspector backends into the actual application UI and export workflows.
- Expose Printer/RIP Soft Proof in Color Management with Output-class profile selection, proof intent and persistent enable state.
- Add `File > Inspect TIFF` plus a report window with Copy report and Explorer reveal.
- Route Face, Export All and Snapshot batch exports through the non-blocking queue with Cancel/Retry and safe atomic completion semantics.
- Expose filename and folder templates in Export All, including `{project}`, `{face}`, `{snapshot}`, `{source}` and `{date}`.
- Add a cached middle-mouse SOURCE preview: original TIFF samples rendered only through the TIFF embedded ICC, bypassing edits, assigned source ICC and Printer/RIP Soft Proof. Right mouse remains BEFORE using the current preview color-management setup.

# Shade Editor 0.17.0

- Add true printer/RIP Soft Proof with a separate Output-class proof ICC, proof rendering intent and project persistence; the transform remains preview-only and never enters TIFF export.
- Make project thumbnails use the exact same color-managed / soft-proof preview pipeline as the viewport.
- Add non-blocking Export Queue with Waiting / Processing / Done / Failed / Cancelled states, safe cancel semantics and Retry.
- Extend export naming with `{project}`, `{face}`, `{snapshot}`, `{date}`, `{source}` and nested Folder Templates while keeping legacy tokens compatible.
- Add `File > Inspect TIFF` read-only production diagnostics with Copy report: TIFF/BigTIFF, dimensions, bits, photometric, planar configuration, compression, predictor, ExtraSamples, channel/Spot order, Photoshop resources, ICC, DPI and estimated uncompressed size.
- Change History labels to Photoshop-style `Tool · Channel` naming.
- Re-validate the existing Safe Project Save / Recovery path: `.tmp` + flush/sync + atomic replace, `.bak` backup, rotating recovery version/checksum and corrupt-state rejection.

# Unreleased

- Replace generic `ICC: managed` metadata text with the active ICC profile description; the profile label is clickable and opens Color Management.
- Add project-owned non-destructive Preview Profile Assignment: use embedded ICC or assign a compatible ICC/ICM only for Shade Editor preview, with Rendering Intent and optional Black Point Compensation stored in `.shade`.
- Add searchable Windows ICC/ICM catalog with Project View-style typing, Up/Down selection and Enter assignment, plus Browse and compatibility checks against the active TIFF base color model.
- Keep assigned profiles strictly out of production export; TIFF samples, embedded ICC and Photoshop resources remain unchanged.
- Consolidate active Rust sources under canonical filenames and remove obsolete versioned implementations and stale one-off documentation.
- Update README/architecture/roadmap to reflect the actual Levels → Mixer → Curve order and current ICC preview boundary.

# Shade Editor 0.16.0

- Add ICC-aware preview color management using each TIFF's embedded RGB/CMYK/Gray profile and an sRGB display destination; rendering intent is selectable in Settings.
- Keep ICC conversion strictly in the preview path. Export continues to use the original TIFF samples and preserve embedded ICC/Photoshop metadata without applying display transforms.
- Composite declared Photoshop Spot separations after the base ICC transform using their existing DisplayInfo color/solidity; known Alpha channels remain excluded from the printing composite.
- Add per-channel Levels and Curve clipping estimates from preview working-space samples, with yellow/red warning indicators in the channel list and detailed percentages in Adjustments.
- Isolate color management behind a dedicated preview module so a future printer/RIP Soft Proof profile can use a proofing transform without changing production TIFF export.
- Keep the adjustment order Levels → Mixer → Curve and keep `.shade` schema v9 unchanged.

# Shade Editor 0.14.2

- Make the Project View Preview pane vertically scrollable while preserving its resizable width and 350px thumbnail cap.
- Project rows show Face count, total source bytes, active-Face pixel dimensions, and the eight newest Snapshot names without repeating the Face filename.
- Reformat Project View project/TIFF metadata into compact two-pair grids and display Snapshots two per row.
- Force white text on the selected accent-filled adjustment channel button.
- Keep 1-9 channel shortcuts active while the Curve graph owns keyboard focus without stealing digits from text/numeric editors.
- Move Export All Reveal folder beside Browse.

# Shade Editor 0.14.1

- Keep Export All Faces compact at a ~500px default width and prevent its text fields from expanding the dialog to the application width.
- Rename the user-facing Previous Shades workspace to Project View and restore the complete v0.13.2 preview metadata while keeping v0.14 relink/remove/reveal/lazy-thumbnail behavior.
- Make the Project View preview pane horizontally resizable and cap its embedded thumbnail display at 350x350px.
- Reflow the Adjustments header and use compact adaptive Levels/Mixer/Curve/Reset tabs so the sidebar stays usable at narrow widths while retaining modified-state indicators.
- Add Ctrl+N, Ctrl+E, Ctrl+Shift+E and G shortcuts for New, Export Face, Export All and Settings.
- Keep operation progress in the right toolbar, widen it, and render operation + stage inside the progress bar without a second detail line.

# Shade Editor 0.14.0

- Export All workspace with destination field, folder TIFF warning, template naming, overwrite/skip/auto-number policies, and optional Explorer reveal after export.
- Project View promoted to a Project Browser with Remove from history, relink for missing .shade files, row-level latest Snapshot/active Face details, Explorer reveal, lazy row rendering, and a bounded thumbnail LRU cache.
- Project thumbnails now persist thumbnail_version and encoded_bytes metadata alongside width/height.
- Adjustment panel shows per-channel modified state and the total number of modified channels.

# Shade Editor 0.13.2

- Upgrade embedded project thumbnails to 512px PNG with bilinear resampling, high PNG compression, and RGB encoding when alpha is fully opaque.
- Cache a compact 72px Project View list thumbnail plus Face count and source bytes for fast history rows, including offline entries.
- Use the `.shade` filename when a project still carries the default `Untitled Shade` name, and normalize the name on the next successful Save/Quick Save.
- Project View now supports Enter to open, Up/Down navigation while Search has focus, first-result selection while searching, an Open shade folder action, and a Snapshot list in the preview pane.
- Export all omits Test Code by default; a persistent Export & storage setting can explicitly enable Test Code for all Face exports.
- Adjustments defaults to All channels, moves the enable toggle into the Editing header, swaps the All channels/channel controls, and uses larger, spaced Levels/Mixer/Curve tabs.

# Shade Editor 0.13.1

- Index Snapshot names, Snapshot IDs and effective Test Code values in the persistent Project View cache.
- Project View search can now find projects by a specific Snapshot/Test code without reopening `.shade` files during search.
- Existing Project View history is migrated once when cached Snapshot metadata is missing and the `.shade` file is available.
- Opening, Save and Quick Save refresh the cached Snapshot/Test index immediately so renamed/new tests become searchable at once.
- Search results show the matching Snapshot name/code when a Snapshot term produced the match.

# Shade Editor 0.13.0

- Add TIFF drag-and-drop directly into the Faces list.
- Add Quick Save for unsaved projects, creating a unique `.shade` beside the active/source TIFF without a Save dialog.
- Remind operators after TIFF export when the active Snapshot/Test state still has unsaved `.shade` project changes.
- Add persistent Project View history with search, sorting, embedded thumbnail/metadata preview, and missing-file retention.
- Reserve `Load all shades from system` for the next Everything Search provider integration.

# Shade Editor v0.12.1

- Moved export/storage controls into a dedicated Settings section.
- Added persistent LZW export compression control; enabled by default.
- Added Settings buttons for bundled Windows Shell install/uninstall scripts and a clear separate-package message when the shell folder is missing.
- Reworked About shortcuts into readable groups.
- Error toolbar messages now auto-expire quickly and can be dismissed.
- Widened operation/update progress bars and moved long operation details below the progress label.

# Shade Editor v0.12.0

- Preserves BigTIFF container format when exporting a BigTIFF source.
- Automatically switches to BigTIFF when the uncompressed output layout approaches the 32-bit offset ceiling of classic TIFF.
- Uses the same Levels / Curve / Channel Mixer / Test Code and Photoshop metadata preservation pipeline for classic TIFF and BigTIFF outputs.
- Adds regression coverage for a real BigTIFF identity export plus large-layout format selection without allocating a huge test image.
- `.shade` schema remains v9.
- Adds a native x64 Windows Shell extension for `.shade` files without loading the editor UI runtime inside Explorer.
- Explorer thumbnails use the PNG embedded in schema-v9 `.shade` files through `IThumbnailProvider` and Windows Imaging Component.
- A read-only `IPropertyStore` exposes Face count, active Face, physical/pixel dimensions, DPI, bit depth, color model, channel counts, source TIFF name, source bytes, and save time.
- Ships a custom Windows Property System schema plus an elevated installer for COM/property-handler registration and per-user `.shade` file association.
- Shade Editor accepts a `.shade` path as its first command-line argument so Explorer double-click opens that project.
- Native parser and COM/WIC regression tests validate schema-v9 metadata, custom properties, and embedded thumbnail decoding.

# Shade Editor v0.11.1

- Fixes physical TIFF DPI detection: XResolution/YResolution are TIFF RATIONAL values and are now decoded as numerator/denominator instead of incorrectly requesting DOUBLE tags.
- Honors TIFF 6.0's default ResolutionUnit of inches when tag 296 is omitted.
- Adds a TIFF export conformance matrix covering Uncompressed, LZW, PackBits, Deflate, horizontal Predictor, 16-bit CMYK, CMYK + Spot ExtraSamples, ICC, Photoshop resources 34377/37724, and physical DPI.
- `.shade` schema remains v9.
# Shade Editor v0.11.0

- Extended bounded-memory TIFF decoding to tiled and planar 8/16-bit RGB/CMYK layouts.
- Preview now samples arbitrary TIFF coding regions instead of requiring full-width strips.
- Export uses a random-access disk-backed spool for tiled/planar sources while keeping the proven sequential strip path for normal Photoshop TIFFs.
- Crash recovery now rotates the latest three states and automatically falls back to an older valid state if the newest recovery JSON is damaged.
- Added planar-strip and edge-tile regression fixtures for the new region decoder.

# Shade Editor 0.10.2

Production export correctness and preview/Test Code workflow fixes.

- Fixes corrupt LZW/Deflate/PackBits files produced by the strip-streaming exporter. image-tiff 0.11.x activates compression in `write_data()` but not direct `write_strip()` calls; Shade Editor now streams adjustments to a bounded disk-backed spool and memory-maps it into the library's correct compressed writer path.
- Adds a regression test starting from a valid six-channel CMYK + 2 ExtraSamples LZW source with horizontal Predictor and fully decodes the exported TIFF byte-for-byte. Horizontal Predictor is intentionally omitted on export when ExtraSamples exist because image-tiff's built-in encoder predictor stride covers only the base RGB/CMYK samples; LZW compression itself is preserved.
- Before/After right-click now uses the actual image interaction rectangle and egui clipping instead of comparing screen pointer coordinates with the ScrollArea's content-relative viewport, so it works correctly while zoomed/scrolled.
- Settings now has **Rebuild previews** next to Preview max dimension to reload all open Faces at the newly selected preview size.
- Test Code can target **All channels** (the new default) or one selected channel. All-channel mode writes the same rasterized code to every separation using each channel's correct ink polarity.
- Adds a maintained roadmap document for the remaining production work.
- `.shade` schema remains v9.

# Shade Editor 0.10.1

Production round-trip validator.

- Adds **Validate face** beside Export actions. It creates a no-adjustment TIFF through the exact production export backend, re-decodes both source and export, and writes JSON + Markdown validation reports.
- Validation checks decoded sample equality, dimensions, bit depth, color model/channel order, compression/predictor/orientation, physical DPI, ICC, Photoshop Image Resources 34377, ImageSourceData 37724, and parsed Photoshop Spot display metadata.
- The validator uses a fresh identity project and disables Test Code so the result is a true transport/interchange check independent of the current shade recipe.
- Adds regression coverage proving the validator exercises the real six-channel export backend.
- `.shade` schema remains v9. Photoshop/RIP application-level interpretation remains an external production gate even when the automated report passes.

# Shade Editor 0.10.0

Production interchange hardening and real Photoshop Spot display metadata.

- Parses Photoshop Image Resource 1077 DisplayInfo for extra channels, including Spot-vs-Alpha kind, display color, and Solidity.
- Composite preview uses the TIFF's Photoshop Spot display color/Solidity when available; known Alpha channels no longer receive a fake ink tint.
- Channel rows distinguish Spot, Alpha, and undeclared Extra channels; Spot hover information includes Solidity.
- Export preserves source lossless compression intent (LZW/Deflate/PackBits/uncompressed), horizontal predictor, orientation, DPI, ICC, Photoshop Image Resources, and ImageSourceData. Unknown/lossy source compression falls back to lossless LZW.
- TIFF exports are written to a same-directory temporary file and atomically replace the destination only after successful completion, preventing half-written production files.
- GitHub Release updates now require a companion ShadeEditor.exe.sha256 asset; downloaded executables are SHA-256 verified before staging.
- Windows CI creates the checksum in both build artifacts and tagged GitHub Releases.
- Adds production Photoshop/RIP validation guidance and regression coverage for the production-shaped DisplayInfo 1077 payload.

# Shade Editor 0.9.0

Production-workflow foundation: adjustment History, crash recovery, Before/After comparison, and a clean .shade v9 schema.

- Photoshop-style adjustment Undo/Redo shortcuts: Ctrl+Alt+Z and Ctrl+Shift+Z, plus a clickable History panel. Only adjustment edits participate; Face operations, Snapshot operations, and Palette changes are intentionally excluded.
- Adjustment drag/keyboard edits are coalesced into useful history states instead of recording every render frame.
- Dirty active Snapshots retain the existing marker but now get a stronger selected background, border, and visual emphasis.
- Hold the right mouse button over the image viewport to temporarily show the unadjusted Before view. Space remains available for viewport panning.
- Recovery autosaves dirty projects every two minutes to LOCALAPPDATA without marking the project saved. On restart the app offers Recover or Discard recovery.
- Successful manual Save clears the recovery copy; Save and exit still waits for the background save to complete.
- .shade schema v9 is intentionally a clean break. All v1-v8 migration code was removed and the loader accepts schema v9 only.
- TIFF preview/export streaming improvements are part of the same v0.9 release backend work.

# Shade Editor 0.8.1

Physical Face dimensions and safeguards against accidental state loss.

- File information now shows physical dimensions in centimeters between bit depth and pixel dimensions, calculated from the TIFF pixel size and effective DPI.
- Duplicate TIFF references remain allowed but every duplicated Face row is highlighted and marked with its duplicate count.
- Closing the application with unsaved project changes now offers Save and exit, Discard and exit, or Stay. Save and exit waits for the existing background Save job to complete successfully before closing.
- Switching away from an active Snapshot with edits that have not been written back using Update now asks whether to Stay editing or Discard changes and switch.
- `.shade` schema remains v8.

# Shade Editor 0.8.0

Optional midpoint Curve editing, deterministic Snapshot naming, and richer self-contained project files.

- Curve starts with Black/White endpoints only. Double-click near the rendered line to add the midpoint at the calculated on-line position; double-click the midpoint to remove it.
- Existing schema v7 projects migrate with their midpoint enabled so their rendering is unchanged.
- Remaining UI icon glyphs are replaced by vector painter geometry or plain ASCII text, including Snapshot checks, Channel Solo indicators, and palette swatches.
- Face information is ordered as filename, bit depth, pixel dimensions, DPI, color model, and channel count.
- Snapshot names follow the first Snapshot's trailing numeric sequence (for example XN-A1-1, XN-A1-2, XN-A1-3).
- .shade schema v8 embeds a PNG project thumbnail (max 256 px) and cached file/project metadata: face count, active face, source dimensions, bit depth, color model, channels, DPI, file size, and source modified time.
- Thumbnail generation runs in the existing background Save job.

# Shade Editor 0.7.1

Compact Adjustment layout controls.

- Adds an optional Compact Curve editor setting that hides the selected-point label, Input / Output numeric fields, and helper text while keeping all three graph points directly draggable.
- In Stacked mode, Levels, Curve, and Channel Mixer Reset actions now live on the same row as their foldout headers instead of at the bottom of each tool.
- Reset all is moved to the right side of the Editing channel header.
- Tabs mode keeps a contextual Reset action beside the tool tabs.
- All-channels Stacked mode uses the same header-level Reset behavior for Levels, Curve, and Mixer.
- No `.shade` schema change; the compact Curve choice is an application setting.

# Shade Editor 0.7.0

Direct three-point Curve editing.

- Curve is edited directly on the graph using exactly three draggable points: Black, Midpoint and White.
- Midpoint can move horizontally (Input) and vertically (Output).
- Black and White endpoints are draggable in both axes as well, while point order is constrained.
- Curve sliders were removed. The selected point exposes compact Photoshop-style Input / Output numeric fields in two columns.
- Input / Output fields use a 0-255 display scale while processing remains normalized internally.
- Broadcast and every per-channel Curve foldout use the same direct editor.
- Existing `.shade` files migrate to schema v7: old relative midpoint output becomes an absolute middle control point and midpoint input starts centered between the prior input endpoints.

# Shade Editor 0.6.1

Font-independent icon compatibility fix.

- Snapshot Export icons are now vector geometry drawn by `egui::Painter`.
- Export-success checkmarks are now vector geometry and no longer depend on a Unicode glyph.
- The same reusable vector widget is used for per-Snapshot, per-day and all-Snapshot actions.
- No icon font needs to be installed on Windows.

# Shade Editor 0.5.0

Snapshot export workflow and expanded Curve controls.

## Snapshot export

- Compact export actions are available for every Snapshot, every day group, and the whole Snapshots panel.
- Snapshot export always targets the active Face and reuses the exact same TIFF/Test Code backend as Export face.
- A single Snapshot uses a Save dialog; day/all exports use a destination folder and export every selected Snapshot there.
- Successful exports are marked with a check. Clicking the check opens the latest export folder.
- Export history is stored per Snapshot + Face in `.shade` schema v5, but never locks or disables re-export.

## Curve

- Curve now has Input black and Input white endpoints in addition to output endpoints and relative midpoint.
- In All channels, Broadcast remains available and copies its Curve to every channel.
- Each channel also has its own collapsed full Curve panel for independent refinement after Broadcast.
- With four channels the Curve section therefore shows one Broadcast Curve plus four channel foldouts.

## Adjustment layout

- In Stacked mode, Levels, Curve and Channel Mixer can each be collapsed/expanded independently.
