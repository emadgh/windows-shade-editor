# Color Conversion binary/library module boundary

Status: **Superseded by the canonical library ownership migration in #396**

Historical tracks: #84, #89, #131
Current cleanup: #396

## Historical context

During the initial Color Conversion implementation, Shade Editor intentionally compiled the same backend source files from two Rust crate roots:

- `src/main.rs` declared modules for the native GUI binary;
- `src/lib.rs` declared backend modules used by conformance/unit tests.

That avoided a broad application refactor while Color Conversion itself was still being built, but it also meant the same source produced separate binary-local and library-local Rust type domains. In particular, `ShadeProject`, `IccProfileIdentity`, `ProjectRole`, `ProductionProvenance`, and related conversion types could not safely cross the crate-root boundary without adapters.

The old document therefore required GUI conversion code to stay inside the binary-local type domain. That was a deliberate temporary compatibility decision, not the desired long-term architecture.

## Current decision

Issue #396 removes the duplicate backend compilation incrementally while preserving historical GUI paths.

For the `model` + `color_conversion` ownership batch:

1. The canonical project/domain model is compiled only by the package library from `src/model_impl.rs`.
2. The canonical Color Conversion domain is compiled only by the package library from `src/color_conversion_impl/mod.rs`.
3. `production_provenance` remains a child of the canonical Color Conversion module at `src/color_conversion_impl/production_provenance.rs`.
4. The binary `src/model.rs` and `src/color_conversion.rs` files are compatibility facades only. They publicly re-export the canonical library modules so existing GUI code can continue to use `crate::model::...` and `crate::color_conversion::...` paths.
5. No mirrored model, provenance, recipe, profile, or conversion structures are introduced.
6. No value conversion/adaptation layer exists between GUI and library types because there is now one Rust type domain for these shared structures.

## Why `model` and `color_conversion` move together

The modules are mutually type-sensitive:

- `model` stores Color Conversion domain types such as project role, linked project references, and production provenance;
- `color_conversion` consumes model-owned ICC profile identity data.

Migrating only one side would leave the binary with a mixture of library-owned and binary-local definitions and recreate the exact type-identity problem this cleanup is intended to remove. They therefore move as one bounded ownership batch.

## UI integration rule

GUI and orchestration code continues to use the stable binary paths:

```text
crate::model::...
crate::color_conversion::...
```

Those paths are facades, not independent implementations. UI code must not add alternate model/profile/conversion structures or copy backend logic locally.

Other conversion backend modules can continue to be migrated incrementally. A module that still compiles in the binary must consume the canonical `crate::model` and `crate::color_conversion` facade types; it must not recreate either domain.

## Test parity rule

Production conversion semantics remain implemented in library-owned backend code and covered by the existing unit/conformance suite. GUI wiring does not own alternate profile conversion, separation, constraint, provenance, TIFF-writing, or project-domain math.

## Distribution constraint

This is a Rust compile-time ownership change only. It does not introduce a runtime DLL, service, sidecar process, or alternate executable. The main product remains the standalone `ShadeEditor.exe`; the existing optional native Shell extension remains a separate concern.

## Consequences

- `ShadeProject`, `IccProfileIdentity`, project-role/linkage types, and production provenance have one canonical Rust type domain.
- Color Conversion GUI code can use the same types exercised by library/conformance tests.
- Duplicate compilation of `model` and `color_conversion` is removed from the application binary.
- Existing `crate::...` GUI paths remain stable through zero-logic compatibility facades.
- The earlier temporary duplicate-crate-root rule is no longer an architectural requirement for these two modules.
