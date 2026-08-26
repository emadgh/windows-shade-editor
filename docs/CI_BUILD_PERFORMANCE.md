# Windows CI build-performance baseline

Issue: #397

The CI performance work must preserve the production validation policy and the normal standalone `ShadeEditor.exe`. Build-speed experiments must not introduce a runtime Rust DLL, sidecar process, reduced-function release artifact, or skipped production validation.

## Baseline

Representative successful Windows runs from 2026-08-25 establish the initial comparison point.

### Draft validation

Run `32895969518`:

| Stage | Approximate time |
| --- | ---: |
| Total Draft job | 3m 11s |
| Rust toolchain setup | 25s |
| Cargo/target cache restore | 70s |
| `cargo check` | 12s |
| Library tests | 30s |
| TIFF benchmark fixture inspector | 32s |

The current cache restore is therefore one of the largest Draft costs and must be measured rather than assumed beneficial.

### Full merge gate

Run `32895839692`:

| Stage | Approximate time |
| --- | ---: |
| Total full job | 6m 28s |
| Cargo/target cache restore | 64s |
| Full Rust tests | 1m 11s |
| Release build | 3m 14s |
| Native Shell build/tests | 19s |

The release Rust build is the dominant full-gate stage even with the current whole-`target` cache.

## Phase 1: observability

The first #397 change adds lightweight in-workflow timing for:

- Cargo cache restore, exact-key hit state, and cache save when a new cache is produced;
- Draft `cargo check` and library tests;
- full Rust tests;
- release build;
- native Shell build/tests;
- timed Draft/full job totals including an explicit cache save when one occurs.

Metrics are emitted to both the GitHub Actions log and Job Summary where applicable. No validation step is removed, and the existing cache paths/key strategy is intentionally unchanged in this phase so subsequent configurations can be compared against the same baseline. The previous combined `actions/cache` step is expressed as explicit `restore` and successful-run `save` actions only so both transfer directions can be timed instead of hiding save work in an automatic post-step.

The job-total timer starts at the first workflow step; GitHub queue/runner-provisioning time is outside that measurement. Cache timing includes the small step-transition overhead around each cache action and is intended for relative comparisons between equivalent workflow revisions.

## Next comparison matrix

After Phase 1 is validated, compare representative Draft and full runs using the same Windows runner class and locked dependency graph:

1. current baseline: registry/git + whole `target` cache;
2. registry/git cache + `sccache`, without whole `target` transfer;
3. hybrid only if measurements justify the additional transfer/complexity.

Do not select a strategy from a single run. Prefer several comparable exact-head runs and compare medians, cache-hit behavior, transfer cost, Rust test time, and release-build time.

## Invariants

- Draft validation remains equivalent to the #346 policy.
- Ready PR/full gate retains full Rust tests, release build, native Shell tests, and property-schema validation.
- Pushes to `main` continue to package the normal executable artifact.
- Cache misses or corruption must fall back to normal compilation and cannot bypass tests.
- TIFF runtime-performance metrics remain separate from Rust/CI compilation metrics.
