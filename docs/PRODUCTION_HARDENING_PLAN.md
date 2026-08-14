# Shade Editor Production Hardening Plan

This document is the tracked execution plan for the production-hardening work that follows v0.17.2. Work must be completed in phase order. A phase is not considered complete until its automated acceptance criteria pass in CI and its linked GitHub issue is updated.

Explicit exclusion: ICC payloads will **not** be embedded into `.shade` projects. External ICC references may store identity metadata (description/hash/profile identity) and may be relinked to equivalent installed profiles.

## Phase 0 — Production safety and project lifecycle

Issue: #26

- [ ] Prevent Export Face / Export All / Snapshot export from ever targeting a source TIFF, including path aliases/case differences where Windows identity can be resolved.
- [ ] Centralize destructive project transitions: New, Open, Previous/Project View open, Recovery replacement and Exit.
- [ ] Use one Save / Discard / Cancel policy for dirty or never-saved projects.
- [ ] Define transition behavior while Export Queue contains Waiting/Processing work.
- [ ] Attach project identity to queued completion marks so completion can never mutate a different project.
- [ ] Make destination reservations global across the complete queue.
- [ ] Re-check destination conflicts and source/destination safety immediately before processing.
- [ ] Add orchestration/integration smoke tests for required application feature wiring.

Acceptance: current project state and source TIFFs cannot be silently destroyed; queued work cannot cross-contaminate project state.

## Phase 1 — Export queue semantics, persistence and transport coverage

Issue: #27

- [ ] `{snapshot}` means Snapshot name; add `{testcode}` for effective test code; keep legacy tokens compatible.
- [ ] Make cancellation semantics explicit and safe; no partial destination TIFF may survive cancellation/failure.
- [ ] Persist recoverable queue recipes across restart.
- [ ] Replace full `ShadeProject` queue clones with a compact immutable `ExportRecipe`.
- [ ] Fingerprint source TIFFs at enqueue and verify before processing.
- [ ] Add Gray TIFF export parity where supported by the TIFF transport.
- [ ] Add queue persistence, fingerprint, cancellation, token and Gray transport tests.

Acceptance: queued recipes are stable, restart-safe, memory-bounded, source-aware and semantically unambiguous.

## Phase 2 — Color management completion (without ICC embedding)

Issue: #28

- [ ] Add optional monitor/display ICC output transform after document/proof conversion.
- [ ] Add optional gamut-warning visualization for printer/RIP soft proof.
- [ ] Persist external ICC identity metadata (description/hash/profile identity) without embedding payloads.
- [ ] Relink missing external ICC paths to matching installed profiles when identity matches.
- [ ] Improve Windows installed-profile discovery using registered color-management sources with filesystem scan fallback.
- [ ] Cache reusable ICC/profile inspection state where safe.
- [ ] Add profile failure-mode tests: missing, corrupt, wrong color space, wrong device class.

Acceptance: preview can model source → printer/RIP proof → monitor, remains strictly preview-only, and external profiles survive path movement when an equivalent installed profile is available.

## Phase 3 — TIFF diagnostics, backup restore and recovery hardening

Issue: #29

- [ ] Run TIFF inspection off the UI thread.
- [ ] Use bounded parser limits rather than unlimited decoder limits.
- [ ] Expand TIFF report: byte order, SampleFormat, FillOrder, Orientation, strip/tile geometry, IFD/page count, InkSet, NumberOfInks, InkNames and richer Photoshop resource diagnostics.
- [ ] Add a concise `Warnings / RIP risks` section.
- [ ] If `.shade` load fails, detect/validate `.shade.bak` and provide explicit restore path.
- [ ] Migrate readable legacy recovery v1 state to checksummed v2 and rely on verified states thereafter.
- [ ] Extend malformed TIFF, backup restore and recovery migration tests.

Acceptance: inspector is non-blocking/read-only and project backup/recovery has a verified fallback path.

## Phase 4 — Architecture decomposition and integration regression coverage

Issue: #30

- [ ] Extract project lifecycle state/logic from `main.rs`.
- [ ] Extract export queue/orchestration state from `main.rs`.
- [ ] Extract color-management UI/controller state from `main.rs` where practical.
- [ ] Extract TIFF inspector controller state from `main.rs`.
- [ ] Replace scattered transition booleans with typed transition state.
- [ ] Add integration-level regression tests for New/Open/Exit guards and queue lifecycle.
- [ ] Add feature-wiring smoke tests so backend modules cannot silently exist without application entry points.

Acceptance: `ShadeApp` is primarily composition/UI routing and critical workflows have integration-level coverage.

## Phase 5 — Production transport validation and acceptance

Issue: #31

Automatable:
- [ ] Expand TIFF fixtures across compression, predictors, 8/16-bit, ExtraSamples, Spot metadata, tiled and planar paths.
- [ ] Add BigTIFF-oriented conformance/structure tests without committing multi-gigabyte binaries.
- [ ] Exercise missing-Face relink and large-file validation paths in CI where feasible.
- [ ] Keep native Shell extension build/tests/schema validation in the required CI path.
- [ ] Add a production acceptance checklist document with exact manual evidence fields.

External environment sign-off (cannot be truthfully automated without the actual applications/hardware):
- [ ] Photoshop round-trip on representative production CMYK + Spot TIFFs.
- [ ] Actual RIP/printer interpretation and proof comparison.
- [ ] >4 GiB BigTIFF acceptance by the production Photoshop/RIP versions.
- [ ] Clean-workstation Explorer thumbnail/property handler install, upgrade and uninstall.

Acceptance: all automatable checks are CI-covered; real-environment checks remain explicit and cannot be mistaken for completed automated validation.

## Completion policy

1. Execute phases in numeric order.
2. Keep the corresponding issue updated with implementation/validation evidence.
3. Do not mark external production checks complete without real evidence.
4. Do not publish GitHub Releases as part of this plan.
5. Production builds must come from validated `main` and be delivered as CI artifacts when requested.
