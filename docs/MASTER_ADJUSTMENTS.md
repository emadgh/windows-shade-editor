# Master / All Channels adjustments

Shade Editor 0.18.3 replaces the previous destructive Levels/Curve "Broadcast to all" behavior with a Photoshop-style independent Master adjustment state.

## Production order

```text
TIFF source samples
  -> per-channel Levels
  -> Master Levels (All Channels)
  -> per-channel N×N Channel Mixer
  -> per-channel Curve
  -> Master Curve (All Channels)
  -> export sample
```

The same order is used by downsampled preview rendering and full-resolution TIFF export.

## Storage contract

Master state is stored in `ShadeProject.adjustments` under the reserved key `MASTER_ADJUSTMENT_KEY` (`__shade_editor_master__`). This keeps `.shade` schema version 9 compatible while allowing Snapshots, History and compact Export Queue recipes to preserve the Master state automatically.

The reserved Master entry is not a TIFF channel and must never be emitted as a channel name, Spot name or mixer output.

## Editing contract

- Master Levels and Master Curve are independent controls.
- Editing Master controls must never copy values into or overwrite per-channel Levels/Curve controls.
- Per-channel edits remain intact when Master controls are changed later.
- `Master enabled` bypasses the Master Levels/Curve pair only; it does not disable individual channels.
- Channel Mixer remains output-channel-specific. There is intentionally no Master Mixer.
- Master Curve uses a neutral aggregate histogram rather than the currently selected channel's accent/histogram.
- `~` / backtick selects All Channels and leaves Solo mode.

## Regression requirements

Tests must cover both preview and full-resolution export stacking and assert that applying Master Levels/Curve does not mutate channel-specific adjustment structures. History must describe Master changes as `All channels`, not expose the reserved storage key.
