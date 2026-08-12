# Shade Editor 0.4.1

Snapshot organization and channel-color UX refinement.

## Added / changed

- Snapshots now store creation timestamps in `.shade` schema v4.
- Snapshot list is grouped by local calendar day; each row shows its creation time.
- Snapshot, Face and Channel rows are full-width click targets with larger row heights.
- New snapshots automatically receive a unique `Test N` name. Rename validation rejects empty or duplicate names case-insensitively.
- Legacy snapshots created before schema v4 remain compatible and appear under `Earlier snapshots` because their original creation time was never stored.
- Channel Mixer source rows use each source channel's own accent for slider controls and labels when adjustment colorization is enabled.
- The selected-channel Adjustment panel gets a subtle border and internal group-border tint matching the active channel, making the current separation visually explicit.

## Retained

- Multi-channel CMYK/RGB + Photoshop Spot support.
- Snapshots, all-channel adjustments, DPI-aware Tahoma test code, Fit viewport, background preview rendering, operation progress, updater progress and application logs from 0.4.0.
