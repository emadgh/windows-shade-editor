# Shade Editor 0.4.0

Precision, test-code, viewport, progress and diagnostics release.

## Added

- `Fit` control beside Zoom to fit the active face into the visible viewport while keeping overscroll/panning available.
- TIFF DPI is shown in the active-face information bar; missing physical resolution is explicitly reported.
- Test Code moved to the left sidebar below Snapshots.
- Blank Test Code automatically uses the active Snapshot name.
- Test Code defaults to top-left with a 1 cm physical edge margin.
- Test Code uses Windows Tahoma and point sizes converted to pixels using the TIFF DPI.
- Histogram colorization setting with channel-specific colors.
- Levels, Curve and Channel Mixer colorization setting with channel-specific accents.
- Active adjusted histogram can be displayed behind the Curve graph.
- Persistent application log with a `Logs` button in the top-right toolbar.
- Background operation progress in the top-right toolbar for TIFF loading, project saving, exporting and preview rendering.
- Update download progress is integrated into the top-right toolbar.

## Precision changes

- Curve midpoint is now relative to the current black/white output endpoints. A midpoint of 0.5 remains a straight line when either endpoint moves; schema-v2 absolute midpoint values migrate to the new relative representation.
- Levels gamma remains relative to the output black/white range and the calculated gamma midpoint is shown in the UI.
- Preview recalculation runs off the UI thread and is deferred while the pointer is held down, so slider clicks/drags settle at the actual pointer-selected value instead of being displaced by a blocking preview render.
- `.shade` schema is now v3 with backward migration from existing projects.

## Update and diagnostics

- GitHub `releases/latest` returning HTTP 404 because no Release has been published is treated as “no update available”, not an application error.
- Update controls no longer occupy a banner below the main toolbar; they are compact controls on the toolbar’s right side.
- Errors are written to the application log and shown temporarily near the top-right progress area.
- The About window remains informational; update actions live in the main toolbar.

## Retained workflow

- One/two-column tools sidebar.
- Centered image canvas with horizontal/vertical scrolling and overscroll.
- Selected-channel and All-channels adjustment scopes.
- Reset all adjustments and named Snapshots.
- RGB/CMYK + Photoshop ExtraSample/Spot decoding, ICC/Photoshop resource preservation and multi-channel export.
