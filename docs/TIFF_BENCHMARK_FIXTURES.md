# TIFF benchmark fixture manifests — Issue #374

Before comparing TIFF performance runs, record the exact fixture bytes and TIFF topology outside the measured GUI session. This prevents SHA-256 work from contaminating benchmark wall time and makes baseline/candidate comparisons reproducible.

Use the checked-in Windows example:

```powershell
cargo run --release --example tiff_benchmark_fixture -- "D:\fixtures\production-face.tif" > .\bench\production-face.fixture.json
```

The manifest records schema version, file size, SHA-256, dimensions, bit depth, SamplesPerPixel/base-channel topology, color model, channel names, compression/predictor/orientation, ICC and Photoshop resource sizes, Classic/BigTIFF, strip/tile storage, planar configuration, rows-per-strip/chunk geometry, streamability flags, raw logical raster bytes and physical-resolution/DPI transport state.

Generate one manifest per benchmark fixture before warm-up. Keep the JSON beside the raw performance log, summary CSV and benchmark metadata sidecar. Baseline and candidate results are comparable only when the fixture SHA-256 and intended topology are identical.

Minimum #374 acceptance set:

1. one representative 8-bit RGB/CMYK TIFF around 200–300 MB;
2. one representative 16-bit TIFF around 200–300 MB;
3. one representative 5–12 channel Production Conversion fixture;
4. the source fixtures used for a 2x2 Test Stack measurement.

Do not regenerate or rewrite a source fixture between baseline and candidate measurements. If a fixture must change, treat it as a new benchmark series rather than combining the samples.