# Shade Editor 0.1.0

Initial native Windows version focused on ceramic-print shade matching.

## Included

- Multi-Face TIFF projects with `.shade` recipe files.
- CMYK plus additional/spot channel discovery.
- Composite and isolated-channel preview.
- Original/adjusted per-channel histogram.
- Levels, compact Curve, and dynamic N×N Channel Mixer for every channel.
- Optional export test code on a selected separation.
- Current-Face and all-Faces TIFF export.
- ICC Profile and Photoshop Image Resources preservation when present.
- Application Settings with automatic updates enabled by default and an option to disable them.
- About window with version, repository link, and manual update check.
- GitHub Release based self-updater modeled after GahYar's update flow.
- Windows x64 GitHub Actions build/release pipeline.

## Compatibility note

This is an engineering preview. Validate exported files with representative production Photoshop/RIP TIFFs before using version 0.1.0 in a production print workflow. The next TIFF milestone should add round-trip fixtures, fuller metadata preservation, and strip/tile streaming for very large artwork.
