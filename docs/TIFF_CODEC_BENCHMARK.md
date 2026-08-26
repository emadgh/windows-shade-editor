# TIFF streaming codec benchmark matrix — Issue #374

This procedure compares compression codecs on the same bounded strip pipeline without changing Shade Editor's production default. `main` remains LZW until representative workstation measurements and Photoshop/Durst/RIP compatibility evidence justify a different transport.

`scripts/run-tiff-codec-benchmark.ps1` performs codec experiments only inside a detached Git worktree at the exact benchmark commit. For Deflate variants it replaces the two shared direct-strip compressor calls in `src/lzw_strip_writer.rs` and the matching TIFF Compression tag declaration in both source-topology and Conversion writer families. The production checkout is never edited.

The codec runner deliberately leaves TIFF encoder buffering at the production default. Buffer capacity is measured independently by `scripts/run-tiff-buffer-benchmark.ps1` so codec and buffer effects are not conflated.

## Scope

Codec comparisons are valid for the direct bounded-strip paths:

- default LZW Face Export from a row-streamable TIFF source;
- direct TIFF Production Conversion, including representative 5–12 channel output.

Do not mix Test Stack, tiled/reorder fallback, non-LZW source-preservation fallback or other paths with this codec series. Those paths have different pass counts and would make the comparison invalid.

## Fixed conditions

Before the series:

1. record the source fixture manifest with `tiff_benchmark_fixture`;
2. use one exact commit and keep the fixture SHA-256 unchanged;
3. keep profile, production buffer capacity, rows/strip, storage device and cache/warm-up policy unchanged;
4. use the same operation and adjustment/conversion recipe for every codec;
5. keep warm-up logs separate from measured logs.

For the first codec decision, use `release-throughput` so codec cost is not confounded with the `opt-level` experiment.

## Recommended matrix

```powershell
.\scripts\run-tiff-codec-benchmark.ps1 `
  -Profile release-throughput `
  -Codec lzw `
  -WarmupLogPath .\bench\codec-lzw-warmup.log `
  -LogPath .\bench\codec-lzw.log

.\scripts\run-tiff-codec-benchmark.ps1 `
  -Profile release-throughput `
  -Codec deflate-fast `
  -WarmupLogPath .\bench\codec-deflate-fast-warmup.log `
  -LogPath .\bench\codec-deflate-fast.log

.\scripts\run-tiff-codec-benchmark.ps1 `
  -Profile release-throughput `
  -Codec deflate-balanced `
  -WarmupLogPath .\bench\codec-deflate-balanced-warmup.log `
  -LogPath .\bench\codec-deflate-balanced.log
```

Run at least five measured repetitions per codec on each representative fixture. Keep one codec per runner invocation so every metadata sidecar has an unambiguous executable SHA-256 and `streaming_codec` identity.

## Evidence to record

For every candidate, retain:

- raw performance log and summary CSV;
- runner metadata sidecar with commit/profile/codec/buffer-policy/executable SHA-256;
- source fixture manifest;
- output file size and output fixture manifest;
- phase median/p95 and median MiB/s;
- successful reopen/inspection in Shade Editor;
- Photoshop compatibility result;
- representative Durst/RIP compatibility result before any production-default change.

Compare summary CSVs with `scripts/compare-tiff-perf.ps1`. A smaller file is not sufficient evidence if compression time or end-to-end throughput regresses materially.

## Correctness gate

All #353 guarantees remain mandatory. Reject a codec candidate if it changes or weakens:

- staged validation and atomic final publication;
- cancellation through the pre-commit boundary;
- source immutability or route-migration recovery;
- ICC, N-channel/InkNames, DPI/orientation, ExtraSamples/Photoshop resources or BigTIFF semantics;
- deterministic Production project recovery after TIFF commit.

The codec experiment is measurement-only. Do not change the production LZW default from this procedure alone; record the representative workstation evidence and compatibility decision in #374 first.
