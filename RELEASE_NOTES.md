# Shade Editor 0.3.0

Adds test-oriented adjustment workflow features while retaining the Photoshop TIFF/Spot fixes from 0.2.1.

## Added

- Adjustment scope switch: edit the selected channel or all channels.
- In `All channels` scope, Levels and Curve changes are broadcast to every channel.
- In `All channels` scope, Channel Mixer exposes every output row independently so the N×N separation matrix is not accidentally collapsed by copying one row to all outputs.
- `Reset all adjustments` restores Levels, Curve, enabled state, and an identity Channel Mixer for every channel.
- Snapshot panel for storing named adjustment test states inside the `.shade` project.
- Create, load/switch, inline rename, update, and delete Snapshots.
- Snapshot dirty indicator when current adjustments differ from the selected saved Snapshot.
- `.shade` schema v2 with backward-compatible loading of existing schema v1 projects.

## Snapshot behavior

Snapshots contain adjustment settings only. Creating a Snapshot captures the current adjustment map. Loading a Snapshot replaces the working adjustments with its stored values. Subsequent edits do not silently overwrite the Snapshot; use `Update` to save the new working state back into the selected Snapshot.

## Retained TIFF fixes

- Full RGB/CMYK + Photoshop ExtraSample/Spot decoding.
- Photoshop channel names and relevant image resources retained.
- ICC metadata preservation and RGB vs CMYK aware export.
- Large TIFF decoder-limit workaround and multi-channel histogram support.

## Compatibility note

This remains an engineering preview. Photoshop/RIP round-trip should be checked after export before production use. Strip/tile streaming is still planned for very large artwork to reduce peak RAM usage.
