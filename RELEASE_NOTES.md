# Shade Editor 0.3.1

Layout and viewport refinement release on top of the 0.3.0 adjustment/snapshot workflow.

## Added

- Tools sidebar can switch between one-column and two-column layouts.
- Two-column layout keeps Channels + Histogram in the first column and Adjustments + Test Code in the second column.
- Sidebar layout preference is persisted in application settings and can also be toggled directly from the tools panel.
- The two-column tools panel uses a wider minimum width while remaining resizable.
- Image viewport now centers the active face on a larger virtual canvas.
- Horizontal and vertical scrolling remain available around the centered image.
- 180 px of overscroll margin is provided beyond the image/canvas edges so the user can pan slightly past the artwork boundaries.
- Viewport recenters when opening/changing a face and when zoom changes.

## Retained 0.3 workflow

- Selected-channel and All-channels adjustment scopes.
- Reset all adjustments to identity/defaults.
- Named adjustment Snapshots stored inside `.shade` projects.
- Full RGB/CMYK + Photoshop ExtraSample/Spot decoding and multi-channel histogram support.

## Compatibility note

This remains an engineering preview. Photoshop/RIP round-trip should be checked after export before production use. Strip/tile streaming is still planned for very large artwork to reduce peak RAM usage.
