# Shade Editor 0.2.1

Engineering fix based on a real Photoshop CMYK TIFF containing two spot channels.

## Fixed

- Decode RGB/CMYK TIFF `ExtraSamples` instead of silently receiving only the base RGB/CMYK samples from image-tiff.
- Preserve Photoshop spot-channel pixel data for files where `SamplesPerPixel` is larger than the base color-channel count.
- Keep the source TIFF immutable: the decoder workaround is a read-only in-memory metadata overlay and never changes the source file.
- Added regression coverage that writes a synthetic CMYK + 2 ExtraSamples TIFF and verifies that all six samples decode byte-for-byte.
- Retains the v0.2 fixes for Photoshop channel names, ICC/ImageResources metadata, RGB vs CMYK export, channel-specific histograms and stacked adjustment panels.

## Verified production sample

The supplied validation TIFF is 720×1280, 8-bit CMYK, LZW + horizontal predictor, with `SamplesPerPixel = 6`, two unspecified ExtraSamples, and Photoshop channel names `purpol` and `bgreen`. The previous decoder returned exactly four samples per pixel; this build explicitly routes RGB/CMYK + extras through full multiband decoding.

## Compatibility note

This remains an engineering preview. Photoshop/RIP round-trip should be checked after export before using the build in production. Strip/tile streaming is still planned for very large artwork to reduce peak RAM usage.
