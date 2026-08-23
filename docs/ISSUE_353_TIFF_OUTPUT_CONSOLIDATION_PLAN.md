# Issue #353 — TIFF Output/Storage Consolidation Plan

Target issue: **#353 — Verify and consolidate TIFF output core across Export, Snapshot and Converter**
Parent: #84
Related: #339, #340, #344
Milestone: `v0.21.x — Conversion Core & Workflow Stabilization`

## Handoff objective

Implement one canonical TIFF output/storage infrastructure for all overlapping mechanics while preserving the distinct domain semantics of:

- adjusted source Face Export;
- Snapshot Export and Export All;
- Test Stack composition;
- CMYK/DeviceLink/N-channel production conversion;
- conversion transaction recovery after the TIFF commit boundary.

All user-facing adjusted-source outputs (Face Export, Export All, Snapshot single/group and Test Stack) must be represented in the existing **Export Queue** in deterministic FIFO order. Production conversion must remain in **Conversion Queue** because it owns project creation/linking/recovery transactions, but its TIFF writer must use the same canonical transport/storage policies.

Do not push any branch or commit. Implement, test and commit locally only. GitHub CI/merge validation is deferred until the owner explicitly requests a push.

## Non-negotiable constraints

1. Source TIFFs remain immutable and can never be selected as an output destination.
2. No partial final TIFF may become visible after failure, panic or pre-commit cancellation.
3. Conversion cancellation remains valid only before the TIFF commit boundary.
4. A TIFF committed before a Production-project save/link failure must remain recoverable and must never be rendered again during recovery.
5. N-channel topology (5–12 inks), InkSet/NumberOfInks/InkNames and target-profile semantics remain explicit in the conversion adapter.
6. Preview/proof ICC settings must never leak into output writers.
7. Network/UNC destinations receive only a same-directory staged TIFF plus atomic final commit; large render spools remain local under `%LOCALAPPDATA%/ShadeEditor`.
8. Existing persistent queue files must deserialize safely. New fields require Serde defaults or an explicit queue schema migration.
9. Do not run a repository-wide formatter rewrite. Format only touched files and keep the diff scoped.

## Current output-path matrix

| Output path | UI/runtime entry | Queue | Render/encode path | Stage/commit path | Important semantics |
|---|---|---|---|---|---|
| Current Face Export | `main.rs::export_current_dialog` | `export_queue.rs` | `export.rs::export_face_with_progress_options` | `export.rs::temporary_export_path` → private `atomic_replace` | Adjustments, optional transport validation, source metadata preservation |
| Export All Faces | `main.rs` batch path | `export_queue.rs` FIFO | Same `export.rs` path per Face | Same private Export staging/replace | Name/folder templates, conflict policy, destination reservation |
| Single Snapshot Export | `ui/snapshots_panel.rs` and legacy `main.rs` path | `export_queue.rs` | Same `export.rs` path with immutable `ExportRecipe` | Same private Export staging/replace | Snapshot test-code provenance and export mark |
| Snapshot group export | `ui/snapshots_panel.rs` and legacy `main.rs` path | `export_queue.rs` FIFO | Same `export.rs` path per Snapshot | Same private Export staging/replace | Batch collision choice, immutable recipe capture |
| Test Stack intermediate renders | `test_stack.rs::export_test_stack_with_progress` | No queue item; foreground job wrapper | Calls normal `export.rs` once per selected Snapshot | Each intermediate render also stages/commits a temporary TIFF | Correct Snapshot render semantics, but unnecessary atomic publication for internal temporary files |
| Test Stack final TIFF | `ui/test_code_panel.rs::start_test_stack` | **Bypasses Export Queue** | Separate encoder in `test_stack.rs` | `staged_output_path` → `safe_fs::commit_staged_file` | Composed same-size raster, duplicate RGB/CMYK/Gray metadata writer |
| ICC Converter output | `icc_conversion_worker.rs` | `conversion_queue.rs` | `conversion_tiff.rs` | Conversion-specific stage, validation and `safe_fs` commit | Target ICC embedding, transaction commit boundary |
| DeviceLink output | Same worker/queue | `conversion_queue.rs` | Same conversion writer | Same conversion stage/validation/commit | DeviceLink is a transform; LinkClass profile is not embedded as output ICC |
| N-channel output | Same worker/queue | `conversion_queue.rs` | Dynamic 5–12 channel adapter in `conversion_tiff.rs` | Same conversion stage/validation/commit | Explicit InkSet/NumberOfInks/InkNames and channel order |

## Confirmed duplication and drift risks

### Storage/transaction mechanics

- `export.rs` owns a private Win32 `MoveFileExW` implementation and a fixed `.tmp` sibling.
- `conversion_tiff.rs` owns `.conversion.tmp`, staged validation, replacement/if-absent behavior and cleanup.
- `test_stack.rs` owns `.test-stack.tmp`, cleanup and `safe_fs` commit.
- `safe_fs.rs` already owns the canonical fsync and atomic replace/if-absent primitives, but Export bypasses it.
- Fixed staged names can collide with stale files; uniqueness and stale-cleanup policy are not expressed once.

### Container/encoding policy

- The classic-TIFF safety threshold `4_000_000_000` is duplicated in `export.rs`, `conversion_tiff.rs` and `test_stack.rs`.
- Source BigTIFF detection is duplicated in Export and Test Stack.
- RGB/CMYK/Gray 8/16-bit encoder dispatch is duplicated between Export and Test Stack.
- LZW/default compression selection and horizontal predictor behavior are duplicated between Export and Test Stack.
- DPI, ResolutionUnit, Orientation, ICC, Photoshop resources and ImageSourceData writing are duplicated between Export and Test Stack.
- Conversion separately hard-codes LZW and has its own DPI/orientation/ICC configuration. Some of this is domain-specific; BigTIFF, compression declaration and resolution/orientation policy are not.

### Queue/routing behavior

- Normal, batch and Snapshot outputs are already queued in insertion order.
- Test Stack bypasses `ExportQueue`, cannot be paused/retried/recovered with the other exports, and is hidden from the queue list.
- Conversion correctly has a separate queue because one job spans TIFF commit, Production `.shade` save, source link and recovery state.
- Cross-queue destination reservation is coordinated at UI boundaries rather than through one reusable output-reservation policy.

## Target architecture

### 1. Canonical transport/storage module

Add `src/tiff_output.rs` and expose it from both `src/lib.rs` and the binary module list in `src/main.rs`.

Recommended API shape (names may change, responsibilities may not):

```rust
pub enum DestinationPolicy {
    ReplaceExisting,
    RequireAbsent,
}

pub struct TiffLayout {
    pub width: u32,
    pub height: u32,
    pub channels: usize,
    pub bit_depth: u8,
}

pub struct AtomicTiffOutput { /* destination + unique staged sibling */ }

pub fn canonical_destination(path: &Path) -> Result<PathBuf, String>;
pub fn layout_requires_bigtiff(layout: TiffLayout) -> bool;
pub fn source_is_bigtiff(path: &Path) -> Result<bool, String>;

impl AtomicTiffOutput {
    pub fn begin(destination: &Path, policy: DestinationPolicy) -> Result<Self, String>;
    pub fn staged_path(&self) -> &Path;
    pub fn commit_after<V>(self, validate: V) -> Result<(), String>
    where V: FnOnce(&Path) -> Result<(), String>;
}
```

Required behavior:

- canonical extension is `.tif`; `.tiff`, mixed-case extensions and missing extensions normalize before reservation and enqueue;
- staging is a unique sibling of the final destination, not a local spool and not a shared fixed filename;
- final commit delegates to `safe_fs::commit_staged_file` or `commit_staged_file_if_absent`;
- staged bytes are flushed/synced by `safe_fs` before publication;
- validation executes before commit;
- `Drop` or explicit failure cleanup removes uncommitted staging best-effort;
- destination parent creation and error wording live here;
- no writer may call `MoveFileExW`, `fs::rename`, hard-link or destination replacement directly after migration.

Keep local mmap/spool allocation separate from final staging. Spools are renderer implementation details and belong under local app data; the staged TIFF belongs beside the destination.

### 2. Shared source-topology encoder

Extract the identical RGB/CMYK/Gray mechanics from `export.rs` and `test_stack.rs` into a source-topology adapter, either inside `tiff_output.rs` or in a focused `source_tiff_writer.rs`.

It must own:

- classic TIFF vs BigTIFF encoder construction;
- RGB/CMYK/Gray 8/16-bit dispatch;
- LZW/preserve-supported-compression policy;
- predictor policy;
- rows-per-strip configuration;
- ExtraSamples declaration;
- DPI and ResolutionUnit;
- Orientation;
- source ICC, Photoshop Image Resources and ImageSourceData preservation;
- common errors and post-write validation hooks.

Use an explicit input spec rather than passing the entire project:

```rust
pub struct SourceTiffWriteSpec<'a> {
    pub source_path: &'a Path,
    pub metadata: &'a TiffMetadata,
    pub dpi: DpiInfo,
    pub force_lzw: bool,
    pub rows_per_strip: Option<u32>,
    pub software_tag: &'a str,
}
```

Pixel rendering remains outside this adapter. It should accept already-rendered `u8`/`u16` samples or a bounded strip source.

### 3. Explicit conversion adapter

Retain `ConversionTiffSpec` and the 4–12 channel encoder dispatch in `conversion_tiff.rs`. Do not force N-channel output through RGB/CMYK/Gray abstractions.

Migrate only overlapping mechanics:

- canonical BigTIFF threshold calculation from `tiff_output`;
- canonical LZW policy declaration;
- shared DPI/orientation tag helpers where byte semantics are identical;
- `AtomicTiffOutput` for staging, validation and replace/if-absent commit;
- canonical `.tif` destination normalization before capture/reservation.

Keep these conversion-specific:

- target ICC embedding versus DeviceLink non-embedding;
- CMYK versus N-channel InkSet semantics;
- NumberOfInks and NUL-separated InkNames;
- exact 5–12 channel topology;
- conversion spool and strip transformation;
- `verify_staged` checks for topology, compression, ICC and ink tags;
- transaction outcome and recovery boundary.

### 4. Export Queue operation model

Extend `ExportQueueSpec` with a Serde-compatible operation payload:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ExportOperation {
    Face { recipe: ExportRecipe },
    TestStack(TestStackExportRecipe),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestStackExportRecipe {
    pub snapshots: Vec<ExportRecipe>,
    pub rows: usize,
    pub columns: usize,
    pub anchor: TestStackAnchor,
}
```

Important: capture compact immutable `ExportRecipe` values for selected Snapshots. Do **not** persist a full `ShadeProject`, thumbnails, preview caches or unrelated snapshot history in the queue.

Compatibility strategy:

- either retain the existing `recipe` field and add `#[serde(default)] test_stack: Option<...>`;
- or add a tagged enum plus a custom/default deserializer that maps old persisted rows to `Face`.

The first strategy has lower migration risk for `export-queue.json`.

At enqueue time:

- normalize destination to `.tif` before path-key generation;
- fingerprint the immutable source;
- reserve the normalized destination;
- validate Test Stack layout and recipe count;
- preserve FIFO insertion order;
- display Test Stack as one queue row, not one row per internal Snapshot render.

At execution time:

- Face jobs follow the existing Export path;
- Test Stack jobs materialize one minimal project per captured Snapshot recipe and call a Test Stack API that accepts captured projects/recipes directly;
- intermediate Test Stack renders must write to unique internal temp files and must not appear as final queue rows;
- queue progress maps all intermediate renders, composition, final write and optional validation into one monotonic `0.0..=1.0` range;
- retry uses the same immutable capture;
- restored jobs require explicit Resume, consistent with current queue behavior.

### 5. Test Stack pipeline refactor

Add a lower-level entry point to `test_stack.rs` that accepts already-captured Snapshot projects/recipes. Keep the existing public ID-based function as a compatibility wrapper for library tests/callers.

Suggested split:

```rust
pub fn export_test_stack_with_progress(/* existing ID-based API */) -> Result<(), String> {
    let projects = materialize_snapshot_projects(...)?;
    export_captured_test_stack_with_progress(..., &projects, ...)
}

pub fn export_captured_test_stack_with_progress(
    source: &Path,
    destination: &Path,
    projects: &[ShadeProject],
    /* layout/options/progress */
) -> Result<(), String>;
```

Then:

- replace the duplicate final TIFF encoder with the shared source-topology writer;
- replace the private `.test-stack.tmp` commit path with `AtomicTiffOutput`;
- use a direct internal-render API if practical so intermediate Snapshot TIFFs are not atomically published as though they were final output;
- if direct internal rendering is too invasive for this issue, retain intermediate TIFFs but make them unique, local and cleanup-guarded; document this as an intentional temporary difference.

### 6. Queue boundaries and reservations

Do not merge Conversion Queue into Export Queue in this issue. Their lifecycle semantics differ:

- Export Queue completion means one final TIFF is complete.
- Conversion Queue completion may mean TIFF committed plus Production project/link committed, or TIFF committed with recovery required.

Instead:

- use the same normalized path-key helper for both queues;
- ensure Export Queue rejects destinations reserved by Conversion Queue and vice versa;
- keep the existing single-active-output policy unless concurrency is explicitly redesigned;
- optionally present a combined read-only “Output activity” summary later, outside #353.

## Implementation sequence

### Phase 0 — Baseline and audit artifact

1. Confirm a clean worktree.
2. Run the existing relevant tests before editing and record failures/environment blockers.
3. Add the current-path matrix above to `docs/ARCHITECTURE.md` or retain this document as the audit artifact.
4. Record exact intentional differences versus accidental duplication.

Exit criteria: baseline results captured; no code change mixed with unknown pre-existing failures.

### Phase 1 — Shared storage transaction

1. Add `tiff_output.rs` with layout policy, canonical destination and atomic staging guard.
2. Unit-test replace, require-absent, validation failure, writer failure, stale/unique staging and cleanup.
3. Delegate final publication exclusively to `safe_fs`.
4. Add compile-time/source-contract tests preventing new private atomic replace implementations in TIFF writers.

Exit criteria: new module is independently tested; no production writer migrated yet.

### Phase 2 — Migrate normal Export

1. Replace `temporary_export_path`, `remove_stale_temp` and private `atomic_replace` in `export.rs`.
2. Preserve existing progress mapping and destination-overwrite behavior.
3. Move BigTIFF calculation into the shared policy.
4. Keep local export spool logic unchanged.
5. Add failure tests proving an existing destination survives render/validation failure.

Exit criteria: all existing TIFF conformance tests pass with no metadata/sample regression.

### Phase 3 — Migrate Test Stack shared mechanics

1. Extract/reuse source-topology writer from Export.
2. Route final Test Stack output through shared storage transaction.
3. Remove duplicate BigTIFF/header and metadata encoding helpers from `test_stack.rs`.
4. Add captured-project/recipe API needed by the queue.
5. Preserve cell composition and code-corner behavior exactly.

Exit criteria: Test Stack output matches source dimensions/topology/metadata and exposes no partial final file.

### Phase 4 — Queue Test Stack

1. Add compact, backward-compatible Test Stack payload to Export Queue.
2. Change `ui/test_code_panel.rs::start_test_stack` from `launch_job` to `enqueue_for_project`.
3. Show one ordered queue row with pause/retry/cancel-after-current semantics.
4. Persist/restore safely and require explicit Resume after restart.
5. Normalize all Export Queue destinations to `.tif` before reservation.

Exit criteria: Current Face, Export All, Snapshot single/group and Test Stack all appear in deterministic Export Queue order.

### Phase 5 — Migrate Converter storage policy

1. Replace conversion-private staging/cleanup/commit with `AtomicTiffOutput`.
2. Keep conversion `verify_staged` as the pre-commit validator.
3. Use shared layout/BigTIFF policy and common resolution/orientation helpers.
4. Preserve replace-versus-require-absent semantics from `CapturedOutputPolicy`.
5. Re-run recovery tests around TIFF-committed/project-save-failed states.

Exit criteria: ICC, DeviceLink and N-channel outputs retain exact topology/metadata and transaction recovery behavior.

### Phase 6 — Cross-path regression suite and documentation

1. Add equivalent-output comparisons for Export, Test Stack and CMYK Converter where semantics overlap.
2. Add failure/cancellation tests at each pre-commit boundary.
3. Update `docs/ARCHITECTURE.md`, `docs/QUEUE_CORE.md` and production validation docs.
4. Remove obsolete duplicated helpers/constants only after all call sites migrate.
5. Run the complete local validation matrix.

Exit criteria: issue acceptance checklist below is fully evidenced.

## Required regression tests

### Shared policy/storage tests

- classic TIFF below threshold;
- BigTIFF at/above threshold and arithmetic overflow fails safe to BigTIFF;
- source BigTIFF preservation for adjusted-source and Test Stack outputs;
- `.tiff`, `.TIFF` and extensionless destinations normalize to `.tif` before reservation;
- replace-existing success;
- require-absent conflict leaves both existing destination and staged data safe;
- write failure leaves no destination when none existed;
- write/validation failure preserves the previous destination;
- panic/cancellation before commit leaves no staged sibling after guard cleanup;
- UNC-style destination staging remains beside destination while mmap spool remains local.

### Metadata/transport equivalence tests

For semantically equivalent outputs compare:

- width and height;
- TIFF versus BigTIFF selection;
- compression tag;
- predictor tag;
- bit depth and SampleFormat;
- SamplesPerPixel and ExtraSamples;
- PhotometricInterpretation;
- RowsPerStrip;
- X/Y resolution and ResolutionUnit;
- Orientation;
- ICC payload bytes;
- Photoshop Image Resources (34377);
- ImageSourceData (37724);
- channel names/order and Spot display metadata where applicable.

### Conversion-specific tests

- CMYK 8-bit and 16-bit;
- DeviceLink output does not embed the LinkClass profile;
- standard Output ICC embeds exact target ICC bytes;
- N-channel 5, 6, 7, 8, 9, 10, 11 and 12 channel dispatch;
- InkSet, NumberOfInks and exact NUL-separated InkNames;
- staged topology/ICC/compression verification failure blocks commit;
- transactional replace versus require-absent;
- cancellation before commit;
- recovery after TIFF commit but before Production project/source-link completion.

### Queue tests

- mixed enqueue order: Face → Snapshot → Test Stack → Face remains FIFO;
- Test Stack is exactly one visible queue row;
- queued Test Stack captures immutable recipes even if the project changes later;
- normalized destinations reserve one path regardless of `.tif`/`.tiff` case;
- retry preserves the original capture;
- restored legacy Face rows deserialize as standard jobs;
- restored Test Stack rows require explicit Resume;
- failed Test Stack shows error and no final partial TIFF;
- Export and Conversion queues cannot reserve equivalent Windows paths concurrently.

## Local validation commands

Run from the repository root on Windows. Do not push.

```powershell
git status --short --branch
git diff --check
cargo test --lib safe_fs
cargo test --lib tiff_output
cargo test --lib tiff_conformance_tests
cargo test --lib test_stack
cargo test --bin ShadeEditor export_queue
cargo test --lib conversion_tiff
cargo test --lib conversion_queue
cargo test --lib conversion_recovery
cargo test --all-targets
cargo check --all-targets
```

If Cargo attempts network access, first diagnose whether the lockfile/cache is incomplete. Network/VPN failure is an environment blocker, not a passing validation. Do not report success unless the command exits zero.

After targeted formatting, verify scope:

```powershell
git diff --name-only
git diff --stat
git diff --check
git status --short --branch
```

Do not run a repository-wide formatter if it rewrites unrelated files. Format only changed Rust files and inspect the diff immediately.

## Suggested local commit sequence

Create commits only after each phase is green. Do not push.

1. `refactor(tiff): add canonical output transaction and container policy`
2. `refactor(export): share source TIFF writer with Test Stack`
3. `feat(export-queue): queue captured Test Stack exports`
4. `refactor(conversion): use canonical TIFF staging and commit policy`
5. `test(tiff): cover cross-path metadata and failure boundaries`
6. `docs(architecture): document canonical TIFF output matrix`

Avoid one large mixed commit. Each commit should compile and should not contain unrelated formatting.

## Acceptance checklist

- [ ] A checked-in matrix identifies every output path, writer, stage, validator and commit primitive.
- [ ] Export no longer owns a private Win32 atomic-replace implementation.
- [ ] Export, Test Stack and Converter use one staging/cleanup/fsync/commit abstraction.
- [ ] BigTIFF threshold arithmetic is implemented once.
- [ ] Export and Test Stack share RGB/CMYK/Gray 8/16-bit encoder and metadata policy.
- [ ] Converter retains explicit 4–12 channel and profile semantics while reusing common mechanics.
- [ ] All adjusted-source outputs, including Test Stack, appear in Export Queue in FIFO order.
- [ ] Production conversion remains transactionally safe in Conversion Queue.
- [ ] `.tif` is canonical before queue reservation and persistence.
- [ ] Equivalent outputs pass metadata/transport comparisons.
- [ ] Failure and cancellation tests prove no partial final TIFF is exposed.
- [ ] Existing Snapshot export marks/provenance are preserved.
- [ ] Existing conversion recovery tests remain green.
- [ ] Full local test/check suite exits zero.
- [ ] Changes are committed locally in scoped commits.
- [ ] Nothing is pushed to GitHub.

## Stop conditions for the implementing model

Stop and report rather than guessing if any of these occur:

- representative production fixtures reveal conflicting metadata semantics not covered here;
- changing queue persistence would make existing `export-queue.json` unreadable;
- a proposed common encoder would weaken N-channel tag validation;
- cancellation would become possible after TIFF commit but before recovery state is recorded;
- an unrelated dirty worktree appears;
- local tests cannot run because dependencies/toolchains are unavailable;
- completing the work would require a push, PR, issue mutation or GitHub-side validation.
