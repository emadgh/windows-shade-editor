# Shade Editor production validation

Run this checklist before promoting a new Shade Editor build into the ceramic printing workflow.

## 1. No-adjustment Photoshop round trip

Use a representative production TIFF with CMYK/RGB base channels plus the real Spot Channels used by the printer.

1. Open the source in Photoshop and record channel order, names, Spot/Alpha type, display color, Solidity, dimensions, bit depth, DPI, and ICC profile.
2. Open the TIFF in Shade Editor, make no adjustments, export it, and reopen the export in Photoshop.
3. Confirm the same base color mode, total channel count, channel order and names.
4. Confirm every expected extra channel is still a true Spot Channel rather than an Alpha channel.
5. Confirm Photoshop Spot display colors and Solidity values are unchanged.
6. Confirm pixel dimensions, bit depth, physical DPI/orientation and ICC profile are unchanged.
7. Compare individual separation pixels. A no-adjustment export must be sample-identical apart from explicitly documented test-code pixels.

## 2. Metadata/resources

For TIFF families that depend on Adobe metadata, compare these tags/resources between source and export:

- SamplesPerPixel (277), BitsPerSample (258), PhotometricInterpretation (262), PlanarConfiguration (284)
- ExtraSamples (338)
- Compression (259), Predictor (317), Orientation (274)
- X/YResolution (282/283), ResolutionUnit (296)
- Photoshop Image Resources (34377), including 1006/1045 channel names and 1077 DisplayInfo
- ICC profile (34675)
- Photoshop ImageSourceData (37724), when present

The export intentionally writes `Software = Shade Editor`; that difference is expected.

## 3. Adjustment parity

Create a controlled adjustment recipe that changes one channel at a time:

- Levels only
- Curve only
- Channel Mixer only
- a Spot output driven by another Spot/base input

Verify the same mathematical result in the exported separation and confirm unrelated channels remain unchanged.

## 4. RIP validation

Import the no-adjustment export into the actual production RIP. Confirm separation count/order/names and verify no channel is silently merged, dropped, converted to process color, or interpreted as Alpha/mask data.

Then run one low-risk controlled print test. Shade Editor's viewport is an engineering simulation; press/RIP output remains authoritative.

## 5. Failure policy

Do not promote a build if any production TIFF family fails the no-adjustment round trip. Keep the source TIFF immutable, retain the failing source/export pair, and record the TIFF tags plus Photoshop resource IDs so the compatibility path can be regression-tested.
