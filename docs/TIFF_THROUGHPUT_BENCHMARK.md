# TIFF throughput benchmark — Issue #374

This benchmark is the acceptance baseline for large ceramic Face Export and Production Color Conversion performance work. It measures the real production path; synthetic codec-only numbers are supplemental and must not replace these measurements.

## Environment

Record before every benchmark set:

- exact Shade Editor commit SHA;
- Windows version/build;
- CPU model and logical-core count;
- RAM capacity;
- source and destination drive model/type (SATA SSD or NVMe);
- filesystem;
- free disk space;
- whether source and destination are on the same physical drive;
- release profile (`opt-level`, LTO, codegen units);
- TIFF crate/backend and compression policy.

Primary acceptance runs must use a local SSD/NVMe. UNC/network-share results are a separate data set.

## Required fixtures

Keep fixture content private if production artwork cannot be committed. Record stable SHA-256 identities and topology so before/after runs use identical bytes.

At minimum use:

1. one 8-bit RGB/CMYK production-size TIFF around 200–300 MB;
2. one 16-bit production-size TIFF around 200–300 MB;
3. one representative 5–12 channel Production Color Conversion case;
4. one Test Stack case with at least 2x2 Snapshot composition.

For each fixture record dimensions, bit depth, samples/channel count, source compression, rows-per-strip/tile layout, ICC presence, Spot/N-channel topology, orientation, physical resolution and source file SHA-256.

## Capturing phase logs from the Windows release build

Shade Editor is a Windows GUI-subsystem executable, so benchmark evidence must not rely on an attached console. Set `SHADE_TIFF_PERF_LOG` to an explicit file before starting the release executable. Supplying the log path also enables instrumentation; `SHADE_TIFF_PERF=1` remains useful for development stderr output when no log file is configured.

```powershell
$log = Join-Path $PWD 'tiff-perf-release.log'
Remove-Item -LiteralPath $log -ErrorAction SilentlyContinue
$env:SHADE_TIFF_PERF_LOG = $log
.\target\release\ShadeEditor.exe
```

Run the required GUI operation(s), close the application after the measured set, and summarize the collected phase records with:

```powershell
.\scripts\summarize-tiff-perf.ps1 -Path $log
.\scripts\summarize-tiff-perf.ps1 -Path $log -CsvPath .\tiff-perf-release-summary.csv
```

The summarizer reports count, median phase time, p95 phase time and median MiB/s for every measured operation/phase. Keep the raw log with the benchmark record; the CSV is a convenience summary, not a replacement for raw evidence.

## Reproducible benchmark runner

Prefer the checked-in runner for acceptance measurements so profile selection, executable path, commit identity, binary SHA-256, log location and summary output are captured consistently.

Run warm-up and measured samples in separate runner invocations so the warm-up never enters the measured summary. The second invocation may use `-SkipBuild` because it reuses the exact profile binary built from the same clean checkout:

```powershell
.\scripts\run-tiff-perf-benchmark.ps1 `
  -Profile release `
  -LogPath .\bench\release-warmup.log

.\scripts\run-tiff-perf-benchmark.ps1 `
  -Profile release `
  -LogPath .\bench\release.log `
  -SkipBuild
```

Use the first GUI session only for the warm-up operation and close Shade Editor. In the second session run the same measured operation at least five times, then close the application. The runner starts each session with a fresh log, waits for Shade Editor to close, summarizes that session and writes a metadata sidecar.

Use the same two-session pattern for the throughput profile:

```powershell
.\scripts\run-tiff-perf-benchmark.ps1 `
  -Profile release-throughput `
  -LogPath .\bench\release-throughput-warmup.log

.\scripts\run-tiff-perf-benchmark.ps1 `
  -Profile release-throughput `
  -LogPath .\bench\release-throughput.log `
  -SkipBuild
```

Use `-SkipBuild` only when the exact requested profile has already been built from the same checked-out commit. The runner rejects tracked working-tree modifications before recording evidence. `-PlanOnly` prints the resolved profile, target, executable, log/summary/metadata paths and Cargo build arguments without building or launching the application; CI uses that mode as a regression check for the runner itself.

After both measured runs, compare the generated summary CSV files with the regression gate:

```powershell
.\scripts\compare-tiff-perf.ps1 `
  -BaselineCsv .\bench\release.summary.csv `
  -CandidateCsv .\bench\release-throughput.summary.csv `
  -CsvPath .\bench\profile-comparison.csv
```

Do not enable a hard throughput threshold until a representative local-SSD/NVMe baseline has been recorded. Once a justified threshold exists, `-MinimumMedianMiBPerSec` and `-MaxThroughputRegressionPercent` can make severe regressions fail closed.

## Run protocol

1. Build the exact commit in `--release` mode.
2. Set `SHADE_TIFF_PERF_LOG` to a new/empty log file for that build/profile.
3. Close unrelated heavy disk/CPU workloads.
4. Run one warm-up operation; do not include it in the median. Use a separate warm-up log/session.
5. Run the same operation at least five measured times in a fresh measured session/log.
6. Record every phase line and total wall time.
7. Report median and p95 total time plus median effective MiB/s.
8. Do not mix local-drive and network-drive results.
9. For build-profile/codec comparisons, change one variable at a time.

OS cache state must be stated. If both warm-cache and cold-cache behavior are measured, report them as separate series rather than averaging them together.

## Required phase accounting

The diagnostic report uses stable labels from `src/tiff_performance.rs`. Map production work to these phases where applicable:

- `source_identity` — source SHA/integrity verification;
- `inspect_decode` — TIFF topology inspection and source decode;
- `adjustment_render` — saved Shade adjustments / overlay rendering;
- `source_spool_write` — adjusted-source scratch raster write;
- `source_spool_flush` — scratch mapping/buffer flush and any sync;
- `color_transform` — ICC/DeviceLink/Custom production transform;
- `output_spool_write` — converted raw output scratch raster write;
- `output_spool_flush` — output scratch flush and any sync;
- `compression_encode` — final TIFF compression/container encoding;
- `staged_validation` — staged TIFF topology/metadata verification;
- `final_durability` — staged final-document `sync_all`/durability barrier;
- `atomic_publication` — final same-volume rename/write-through publication;
- `output_identity` — committed-output SHA generation;
- `route_migration_verification` — extra migration checkpoint/hash/swap verification.

A phase that does not apply should be absent rather than reported as artificial zero work.

## Result table

Record one row per measured run before calculating summary statistics.

| Commit | Operation | Fixture SHA | Input MiB | Output MiB | Total s | Effective MiB/s | Encode s | Temp I/O s | Hash s | Final durability s | Notes |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| | | | | | | | | | | | |

`Effective MiB/s` for an operation is based on the primary logical raster/output bytes stated for that test, not on undocumented aggregate storage traffic. Phase-level byte counts should be used to show the true I/O multiplier separately.

## Comparisons required by #374

### Release optimization

Compare the current production profile against `opt-level = 3` on the same fixtures. Record executable size as a secondary metric; raster throughput is the primary metric for the workstation application.

The branch provides an independent throughput profile so this comparison does not change the normal release build before evidence exists:

```powershell
cargo build --release
cargo build --profile release-throughput
```

Use a separate `SHADE_TIFF_PERF_LOG` file for each profile and compare the two summaries. Do not combine their samples into one log summary.

### Buffering

Compare the historical default `BufWriter` capacity against measured larger capacities. The current #374 branch includes a 1 MiB shared TIFF encoder buffer as an experiment; retain it only if real fixture measurements show benefit without a compatibility regression.

### Compression/backend

At minimum compare the current LZW implementation with any production-safe alternatives under consideration. Record output size and compatibility evidence together with throughput.

### Architecture pass count

For each implementation revision record the number of complete logical-raster/full-file passes. A faster implementation that still hides redundant full-file passes must not be presented as the final architectural fix.

## Correctness gate

Every optimized path must still satisfy Issue #353 before its performance numbers are accepted:

- no partial final TIFF;
- cancellation wins until the real commit boundary;
- final staged output remains validated and durably published;
- source artwork remains immutable;
- ICC, N-channel/Spot, DPI/orientation and BigTIFF semantics remain correct;
- route-migration crash recovery remains deterministic.

Performance runs that bypass these guarantees are diagnostic experiments only and are not merge evidence.
