# Shared Queue Core

Shade Editor has two independent job domains:

- **Export Queue** — snapshot/export jobs, source fingerprints, conflict policy, validation and stop-after-current behavior.
- **Conversion Queue** — immutable conversion captures, Production project disposition, transactional conversion cancellation and `NeedsRecovery` project recovery.

They intentionally remain separate domain queues. `queue_core` owns only infrastructure that has identical semantics in both domains.

## Shared responsibilities

`src/queue_core.rs` owns:

- stable monotonically allocated job IDs;
- active job identity;
- queue pause state;
- typed worker/event transport;
- persistence path and persistence-error state;
- the common persisted v1 envelope (`format_version`, `next_id`, `paused`, `items`);
- atomic JSON load/write helpers;
- common Waiting/Processing/Done/Failed/Cancelled lifecycle restore rules;
- finite/clamped progress normalization.

## Domain boundaries

The shared core does **not** own job payloads or business rules.

Export keeps:

- source fingerprinting;
- protected-source and destination reservation checks;
- overwrite/skip/auto-number conflict policy;
- export metrics;
- `stop_after_current` cancellation behavior;
- snapshot export marks/provenance.
- operation payloads for standard Face exports and compact captured Test Stack recipes;
- `.tif` destination normalization before reservation, persistence and worker execution;
- one FIFO queue row for a Test Stack, including its pause, retry, cancellation and restore behavior.

Conversion keeps:

- `ConversionJobCapture`;
- `ProductionProjectDisposition`;
- transaction phase/progress semantics;
- `ConversionCancellation` and safe commit boundaries;
- `NeedsRecovery` and project-only recovery records;
- Production project/provenance commit rules.

`NeedsRecovery` is deliberately **not** mapped into the common lifecycle. A restored recovery item remains `NeedsRecovery`; it cannot be flattened into Waiting or retried as a full conversion.

## Persistence compatibility

This refactor does not migrate or rename queue files.

Existing locations remain:

- `%LOCALAPPDATA%/ShadeEditor/export-queue.json`
- `%LOCALAPPDATA%/ShadeEditor/conversion-queue.json`

Both formats remain **version 1** and retain the same top-level JSON fields:

```text
format_version
next_id
paused
items
```

Domain item/status payloads are unchanged. In particular, Conversion's `needs_recovery` serialized status remains a Conversion-only value.

A Waiting or interrupted Processing item restored from a previous process still requires explicit operator resume. Processing is normalized to Waiting during restore. Completed rows continue to be omitted from restored working sets exactly as before.

A future incompatible persisted-shape change must increment the relevant queue format and define an explicit migration. The shared-core extraction itself is not such a change.

## Safety invariants

- Queue refactoring must not weaken destination reservation checks.
- Restored work must never auto-start.
- Conversion cancellation must continue respecting transactional commit boundaries.
- A committed TIFF that still needs Production-project recovery must remain recoverable and must not be re-run as a fresh conversion.
- Persistence remains atomic through `safe_fs::atomic_write`.
- Export Queue operation payloads are backward-compatible: old rows without the operation field
  deserialize as standard exports. Test Stack payloads store only immutable `ExportRecipe` values,
  grid dimensions and anchor—not a full project, thumbnail cache or snapshot history.
- Final export files are unique same-directory staged TIFFs committed through `safe_fs`; local
  render spools remain under application data and are never used as final-output staging.
