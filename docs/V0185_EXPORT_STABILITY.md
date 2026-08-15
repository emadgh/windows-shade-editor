# Shade Editor v0.18.5 — Export Stability Contract

This release hardens the application against failures that occur while the UI remains active during a long-running export.

## Worker failure isolation

Production builds use Rust unwind semantics. Export, preview-render, and generic background workers are guarded so a Rust panic is converted into an application-level failure instead of aborting the whole Shade Editor process. Queue export failures become `Failed` rows; preview/background failures are surfaced through the normal error UI.

A global panic hook writes panic payload and source location to `%LOCALAPPDATA%\ShadeEditor\shade-editor.log` before the default panic hook runs. This log is the first diagnostic source if an unexpected worker failure is reported in production.

## Local export spool

The large raw disk-backed spool used for streaming TIFF export and memory mapping is always created under `%LOCALAPPDATA%\ShadeEditor\export-spool`. It is never memory-mapped directly from a UNC/network export folder.

The final TIFF temporary file remains beside the requested destination as `<final-name>.tmp`, preserving the existing atomic final replacement behavior. The local raw spool is deleted when export finishes or fails.

## Interactive queue contract

The Export Queue remains UI-thread-owned and communicates with its single active export worker through message passing. Queue rows may be opened/read, and new export recipes may be enqueued while another item is processing. Export recipes are immutable snapshots captured at enqueue time.

## Tonal display control

Curve/Histogram tonal direction is exposed as one toggle button. v0.18.5 corrects the user-facing Light/Pigment labels without changing the serialized settings enum, so existing settings remain compatible. The same control is available in tool panels and Settings > Color guides.

## Validation

The v0.18.5 validation pipeline requires locked Windows check/test/release build plus regression tests covering caught worker panics, local spool placement for a UNC-style destination, and queue read/enqueue activity while an item is marked Processing.
