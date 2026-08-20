# Development workflow

Shade Editor keeps strict production and persistence invariants, but normal code iteration should stay fast.

## Branches and pull requests

- Develop features and bug fixes on dedicated branches.
- Link meaningful bugs/features to an Issue when appropriate.
- Keep a PR in **Draft** while implementation is still changing materially.
- Mark the PR **Ready for review** only when it is intended to become a merge candidate.
- Do not merge temporary diagnostic/helper workflows or branches.

## Validation policy

Use the smallest validation scope that proves the change while coding.

### During implementation

- Run focused unit/integration tests for the module or behavior being changed.
- Add a regression test when fixing a demonstrated bug or protecting an important invariant.
- Do not add tests merely to mirror individual lines, getters, labels, or implementation details.
- A Draft PR is repository-level sanity validation, not a release gate.

Draft CI runs:

1. `cargo check --locked --target x86_64-pc-windows-msvc`
2. `cargo test --locked --target x86_64-pc-windows-msvc --lib`

### Merge candidate

The exact final non-Draft PR head must pass the full Windows merge gate before merge:

1. repository version identity validation;
2. full locked Rust test suite;
3. locked Windows release build;
4. native Shell extension build/tests;
5. Shell property-schema XML validation.

Release artifact packaging is not required on PRs. It runs after a push to `main` (and for manual workflow dispatch).

## Versioning

Do not bump the patch version during normal implementation. Bump it once the PR train is ready to merge.

The final version identity must match in exactly:

- root `Cargo.toml`;
- the root `windows-shade-editor` package entry in `Cargo.lock`;
- root `VERSION`.

## Project safety invariants

Tests should concentrate on behavior where regression would be costly, especially:

- Source `.shade` remains explicit-save; never background-save it.
- Production conversion/recovery must fail closed on stale or mismatched identities.
- Existing conversion provenance/history must not be silently destroyed.
- Replacement/re-conversion is an explicit transaction, not generic append/overwrite.
- Output/project SHA-256 and compatibility contracts must remain deterministic where captured.

Refactors that do not change behavior do not automatically require new tests. UI wording/layout changes normally need targeted compile/behavior validation, not an additional full release pipeline during Draft iteration.
