# Shade Editor production roadmap

This file tracks only work that is still intentionally in scope. Snapshot expansion and stronger duplicate-content detection were explicitly removed from the plan.

## Current blocking validation

- Run a real no-adjustment `Validate face` round trip on production CMYK + Spot TIFFs in Photoshop and the production RIP.
- Confirm Spot type/order/name, Photoshop display color/Solidity, ICC, Photoshop resources, DPI, predictor/compression, and press/RIP interpretation.
- Production-test the v0.10.3 missing-Face relink workflow against moved project folders and changed storage roots.
- Production-test automatic post-export validation on large CMYK + Spot artwork before considering default-on behavior.

## Backend follow-up

- Production-test BigTIFF export on >4 GiB ceramic artwork and confirm Photoshop/RIP acceptance.
- Production-test bounded tiled/planar TIFF streaming against real Photoshop/RIP assets; synthetic planar-strip and tiled-edge fixtures are covered in CI.
- Production-test the three-state recovery rotation and corrupted-latest fallback on Windows.
- Continue TIFF conformance regression coverage across compression, predictors, bit depth, ExtraSamples, and Photoshop metadata.

## Native Windows integration

- Implemented in v0.12: native `.shade` Explorer thumbnail provider using the embedded project PNG.
- Implemented in v0.12: read-only Windows Property Handler exposing cached project/Face metadata.
- Remaining Shell validation: clean-workstation install, thumbnail cache behavior, Details columns/search indexing, file association, upgrade, and removal while Explorer may have the DLL loaded.

## Explicitly out of scope

- More Snapshot features (notes/status/favorites/comparison/sorting).
- More duplicate detection beyond the current duplicate-reference behavior.
- Additional adjustment types until production transport and workflow validation are complete.
