# Shade Editor production roadmap

This file tracks only work that is still intentionally in scope. Snapshot expansion and stronger duplicate-content detection were explicitly removed from the plan.

## Current blocking validation

- Run a real no-adjustment `Validate face` round trip on production CMYK + Spot TIFFs in Photoshop and the production RIP.
- Confirm Spot type/order/name, Photoshop display color/Solidity, ICC, Photoshop resources, DPI, predictor/compression, and press/RIP interpretation.
- Production-test the v0.10.3 missing-Face relink workflow against moved project folders and changed storage roots.
- Production-test automatic post-export validation on large CMYK + Spot artwork before considering default-on behavior.

## Backend follow-up

- Extend bounded streaming to tiled and planar TIFF layouts. Normal chunky strip TIFFs already use the streaming pipeline.
- Rotate crash recovery through the latest three recovery states instead of keeping only one recovery file.
- Continue TIFF conformance regression coverage across compression, predictors, bit depth, ExtraSamples, and Photoshop metadata.

## Native Windows integration

- Windows Explorer `.shade` thumbnail provider using the embedded project PNG.
- Windows Property Handler exposing physical/pixel dimensions, DPI, bit depth, channel/Face counts, and save metadata.

## Explicitly out of scope

- More Snapshot features (notes/status/favorites/comparison/sorting).
- More duplicate detection beyond the current duplicate-reference behavior.
- Additional adjustment types until production transport and workflow validation are complete.
