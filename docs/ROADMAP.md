# Shade Editor production roadmap

This file tracks only work that is still intentionally in scope. Snapshot expansion and stronger duplicate-content detection were explicitly removed from the plan.

## Current blocking validation

- Run a real no-adjustment `Validate face` round trip on production CMYK + Spot TIFFs in Photoshop and the production RIP.
- Confirm Spot type/order/name, Photoshop display color/Solidity, ICC, Photoshop resources, DPI, predictor/compression, and press/RIP interpretation.

## Next production workflow

- Relink missing Faces: Locate file, Locate folder, batch resolution of missing sources, and replacement verification against `.shade` cached metadata.
- Complete keyboard workflow: Ctrl+S, Ctrl+Shift+S, F Fit, 1-9 channel selection, S Solo, Ctrl+Enter Update Snapshot, Curve point arrow-key nudging and Shift+Arrow larger steps. Existing Ctrl+Alt+Z / Ctrl+Shift+Z history shortcuts stay unchanged.
- Optional automatic post-export validation summary for normal Export face / Export all, reusing the existing validator and showing a compact verified/failed status.

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
