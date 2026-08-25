# TIFF encoder buffer benchmark matrix — Issue #374

This procedure measures only the TIFF encoder `BufWriter` capacity while keeping the benchmark commit, profile, fixture bytes, storage, cache state and production code path constant.

The production checkout is never edited. `scripts/run-tiff-buffer-benchmark.ps1` creates a detached Git worktree at the exact current commit, changes the `TIFF_ENCODER_BUFFER_BYTES` declaration in both `src/source_tiff_writer.rs` and `src/conversion_tiff.rs` only inside that isolated worktree, builds the selected profile and removes the worktree in `finally`.

Use the same fixture and operation for every variant. Record a fixture manifest before the benchmark series. Supply `-WarmupLogPath` so the runner opens a warm-up session first and then a fresh measured session using the **same compiled executable**. The warm-up log is never included in the measured summary, and the metadata sidecar records the same executable SHA-256 used for both sessions.

Recommended matrix for #374:

```powershell
.\scripts\run-tiff-buffer-benchmark.ps1 -Profile release-throughput -BufferBytes 65536   -WarmupLogPath .\bench\buffer-64k-warmup.log  -LogPath .\bench\buffer-64k.log
.\scripts\run-tiff-buffer-benchmark.ps1 -Profile release-throughput -BufferBytes 262144  -WarmupLogPath .\bench\buffer-256k-warmup.log -LogPath .\bench\buffer-256k.log
.\scripts\run-tiff-buffer-benchmark.ps1 -Profile release-throughput -BufferBytes 1048576 -WarmupLogPath .\bench\buffer-1m-warmup.log   -LogPath .\bench\buffer-1m.log
.\scripts\run-tiff-buffer-benchmark.ps1 -Profile release-throughput -BufferBytes 4194304 -WarmupLogPath .\bench\buffer-4m-warmup.log   -LogPath .\bench\buffer-4m.log
```

Use the warm-up GUI session for exactly one warm-up operation, close Shade Editor, then run at least five identical measured operations in the second GUI session and close it. Keep fixture SHA, profile, cache policy and drive conditions constant across all four variants.

The runner accepts 8 KiB through 64 MiB. Each measured run records `tiff_encoder_buffer_bytes`, commit SHA, profile/target, executable SHA-256, optional warm-up log path and measured session timestamps in its metadata sidecar. Use `-PlanOnly` to verify the requested value, warm-up/log paths and the two source declarations without creating a worktree or compiling.

After measured runs, compare their summary CSV files with `scripts/compare-tiff-perf.ps1`. Do not choose a new production default from one run: use at least five measured repetitions per variant and compare median/p95 time, median MiB/s, output size and compatibility results. The current production default remains 1 MiB until representative 200–300 MB local-SSD/NVMe evidence justifies a change.

All #353 correctness gates remain mandatory. A buffer result is invalid if it changes TIFF metadata/topology, cancellation/commit behavior, source immutability or route-migration recovery semantics.
