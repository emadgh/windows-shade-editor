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
