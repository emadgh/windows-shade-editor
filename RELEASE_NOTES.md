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
