# Shade Editor 0.4.2

Channel selection / solo-preview interaction refinement.

## Changed

- Clicking a different channel now selects it for editing while keeping the composite preview.
- Clicking the already-selected channel toggles that channel's monochrome Solo Preview on/off.
- Channel rows show an outline square (`□`) normally and a filled square (`■`) when that channel is soloed.
- Selecting a different channel while another channel is soloed automatically returns the viewport to Composite before editing the new channel.
- Channel Mixer `Constant` is visually separated from the source-channel coefficients with additional spacing and a divider.

## Retained

- Full-row Face / Channel / Snapshot selection, dated Snapshot groups, unique Snapshot names and per-channel adjustment color cues from 0.4.1.
- Multi-channel RGB/CMYK + Photoshop Spot TIFF support, background rendering, progress, updater, logs, DPI-aware test code and Fit viewport.
