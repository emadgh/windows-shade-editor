# Shade Editor Production Hardening Plan

This document is the tracked execution plan for the production-hardening work that follows v0.17.2. Work must be completed in phase order. A phase is not considered complete until its automated acceptance criteria pass in CI and its linked GitHub issue is updated.

Explicit exclusion: ICC payloads will **not** be embedded into `.shade` projects. External ICC references may store identity metadata (description/hash/profile identity) and may be relinked to equivalent installed profiles.

## Phase 0 — Production safety and project lifecycle

Issue: #26

- [x] Prevent Export Face / Export All / Snapshot export from ever targeting a source TIFF, including path aliases/case differences where Windows identity can be resolved.
- [x] Centralize destructive project transitions: New, Open, Previous/Project View open, Recovery replacement and Exit.
- [x] Use one Save / Discard / Cancel policy for dirty or never-saved projects.
- [x] Define transition behavior while Export Queue contains Waiting/Processing work.
- [x] Attach project identity to queued completion marks so completion can never mutate a different project.
- [x] Make destination reservations global across the complete queue.
- [x] Re-check destination conflicts and source/destination safety immediately before processing.
- [x] Add orchestration/integration smoke tests for required application feature wiring.

Acceptance: current project state and source TIFFs cannot be silently destroyed; queued work cannot cross-contaminate project state.

Validation: Windows `cargo fmt`, `cargo check` and full `cargo test` passed in Actions run `31839334247`; validated Phase 0 source commit is `a33079fdbc92f40bb595792e8e5e56750fab3337`.

## Phase 1 — Export queue semantics, persistence and transport coverage

Issue: #27

- [x] `{snapshot}` means Snapshot name; add `{testcode}` for effective test code; keep legacy tokens compatible.
- [x] Make cancellation semantics explicit and safe; no partial destination TIFF may survive cancellation/failure.
- [x] Persist recoverable queue recipes across restart.
- [x] Replace full `ShadeProject` queue clones with a compact immutable `ExportRecipe`.
- [x] Fingerprint source TIFFs at enqueue and verify before processing.
- [x] Add Gray TIFF export parity where supported by the TIFF transport.
- [x] Add queue persistence, fingerprint, cancellation, token and Gray transport tests.

Acceptance: queued recipes are stable, restart-safe, memory-bounded, source-aware and semantically unambiguous.

Validation: Windows `cargo fmt`, `cargo check` and full `cargo test` passed in Actions run `31840174261`; validated Phase 1 source commit is `906667b50face24e29f775cf9f21fd3553c97cfd`.

## Phase 2 — Color management completion (without ICC embedding)

Issue: #28

- [x] Add optional monitor/display ICC output transform after document/proof conversion.
- [x] Add optional gamut-warning visualization for printer/RIP soft proof.
- [x] Persist external ICC identity metadata (description/hash/profile identity) without embedding payloads.
- [x] Relink missing external ICC paths to matching installed profiles when identity matches.
- [x] Improve Windows installed-profile discovery using registered color-management sources with filesystem scan fallback.
- [x] Cache reusable ICC/profile inspection state where safe.
- [x] Add profile failure-mode tests: missing, corrupt, wrong color space, wrong device class.

Acceptance: preview can model source → printer/RIP proof → monitor, remains strictly preview-only, and external profiles survive path movement when an equivalent installed profile is available.

Validation: Windows `cargo fmt`, `cargo check` and full `cargo test` passed in Actions run `31841006134`; validated Phase 2 source commit is `12882d50d29e9d138b612e9b65fc95c0a8ebf769`. ICC payload embedding remains explicitly excluded.

## Phase 3 — TIFF diagnostics, backup restore and recovery hardening

Issue: #29

- [x] Run TIFF inspection off the UI thread.
- [x] Use bounded parser limits rather than unlimited decoder limits.
- [x] Expand TIFF report: byte order, SampleFormat, FillOrder, Orientation, strip/tile geometry, IFD/page count, InkSet, NumberOfInks, InkNames and richer Photoshop resource diagnostics.
- [x] Add a concise `Warnings / RIP risks` section.
- [x] If `.shade` load fails, detect/validate `.shade.bak` and provide explicit restore path.
- [x] Migrate readable legacy recovery v1 state to checksummed v2 and rely on verified states thereafter.
- [x] Extend malformed TIFF, backup restore and recovery migration tests.

Acceptance: inspector is non-blocking/read-only and project backup/recovery has a verified fallback path.

Validation: Windows `cargo fmt`, `cargo check` and full `cargo test` passed in Actions run `31841665133`; validated Phase 3 source commit is `3258881c277e89b8080a4f5a2d3e9dbd9bc30bd8`.

## Phase 4 — Architecture decomposition and integration regression coverage

Issue: #30

- [x] Extract project lifecycle state/logic from `main.rs`.
- [x] Extract export queue/orchestration state from `main.rs`.
- [x] Extract color-management UI/controller state from `main.rs` where practical.
- [x] Extract TIFF inspector controller state from `main.rs`.
- [x] Replace scattered transition booleans with typed transition state.
- [x] Add integration-level regression tests for New/Open/Exit guards and queue lifecycle.
- [x] Add feature-wiring smoke tests so backend modules cannot silently exist without application entry points.

Acceptance: `ShadeApp` is primarily composition/UI routing and critical workflows have integration-level coverage.

Validation: Windows `cargo fmt`, `cargo check` and full `cargo test` passed in Actions run `31843574891`; validated Phase 4 source commit is `24b424725ab246a309960237dca163ae027e7325`.

## Phase 5 — Production transport validation and acceptance

Issue: #31

Automatable:
- [x] Expand TIFF fixtures across compression, predictors, 8/16-bit, ExtraSamples, Spot metadata, tiled and planar paths.
- [x] Add BigTIFF-oriented conformance/structure tests without committing multi-gigabyte binaries.
- [x] Exercise missing-Face relink and large-file validation paths in CI where feasible.
- [x] Keep native Shell extension build/tests/schema validation in the required CI path.
- [x] Add a production acceptance checklist document with exact manual evidence fields.

Validation: Windows `cargo fmt`, locked `cargo check` and full locked `cargo test` passed in Actions run `31863506989`; validated automated Phase 5 source commit is `ef23cf16873d4cb8de55dfe7b832301110f22c1a`. The temporary Phase 5 validation workflow was removed after the successful run.

External environment sign-off (cannot be truthfully automated without the actual applications/hardware):
- [ ] Photoshop round-trip on representative production CMYK + Spot TIFFs.
- [ ] Actual RIP/printer interpretation and proof comparison.
- [ ] >4 GiB BigTIFF acceptance by the production Photoshop/RIP versions.
- [ ] Clean-workstation Explorer thumbnail/property handler install, upgrade and uninstall.

Manual evidence for these four items is tracked in `docs/PRODUCTION_ACCEPTANCE_CHECKLIST.md` and remains intentionally open until operator reports are supplied.

Acceptance: all automatable checks are CI-covered; real-environment checks remain explicit and cannot be mistaken for completed automated validation.

Final v0.18.0 clean-tree validation: Actions run `31864112759` passed `rustfmt --check`, locked Windows `cargo check`, the full locked test suite, version consistency checks, and explicit temporary-file absence checks. The validated promotion source commit is `41bdb49c5e53cf6c7e3a39b8e3bfc81ffc20eb4b`.

## Completion policy

1. Execute phases in numeric order.
2. Keep the corresponding issue updated with implementation/validation evidence.
3. Do not mark external production checks complete without real evidence.
4. Do not publish GitHub Releases as part of this plan.
5. Production builds must come from validated `main` and be delivered as CI artifacts when requested.
