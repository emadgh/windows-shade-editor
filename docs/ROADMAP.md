# Shade Editor production roadmap

## Current blocking validation

- Run no-adjustment `Validate face` round trips on representative production CMYK + Spot TIFFs in Photoshop and the production RIP.
- Confirm Spot type/order/name, Photoshop DisplayInfo/Solidity, embedded ICC preservation, Photoshop resources, DPI, predictor/compression and RIP interpretation.
- Production-test missing-Face relink behavior against moved project folders/storage roots.
- Production-test optional post-export validation on large CMYK + Spot artwork before considering default-on behavior.

## Backend follow-up

- Production-test BigTIFF output above 4 GiB and confirm Photoshop/RIP acceptance.
- Production-test bounded tiled/planar streaming with real artwork; synthetic fixtures remain CI coverage.
- Continue TIFF conformance coverage across compression, predictors, bit depth, ExtraSamples and Photoshop metadata.
- Add fixtures for preview-profile assignment failure modes (missing external ICC, wrong color space, corrupt profile).

## Color-management scope

Implemented now: embedded ICC preview, project-owned temporary ICC assignment, rendering intents, optional black-point compensation, searchable installed Windows profiles and sRGB preview output.

Implemented: printer/RIP proof-device transforms using a selected Output-class ICC. Still deferred: monitor-profile output transforms and production-specific gamut-alarm UX. Validate proof appearance against the real RIP/printer workflow before treating the screen as a contractual press match.

## Native Windows integration

- Validate `.shade` thumbnail/property handler install, Explorer cache/indexing, file association, upgrade and removal on a clean workstation.

## Explicitly out of scope

- More Snapshot metadata/features beyond the current workflow.
- Duplicate-content detection beyond current duplicate-reference behavior.
- Additional adjustment types until production transport/interchange validation is complete.
