# Color Conversion binary/library module boundary

Status: **Accepted integration decision**

Tracks: #84, #89, #131

## Context

Shade Editor currently uses the same backend source files from two Rust crate roots:

- `src/main.rs` declares modules for the native GUI binary;
- `src/lib.rs` declares backend modules used by conformance/unit tests.

A Rust module compiled under the binary crate and the same file compiled under the library crate are different crate-local type domains. Passing `lib::model::ShadeProject` into code expecting `bin::model::ShadeProject` would therefore be invalid even though both types originated from `src/model.rs`.

Color Conversion must not introduce adapters or duplicate model structures merely to cross that boundary.

## Decision

For the current architecture, Color Conversion backend files follow the existing Shade Editor pattern:

1. The canonical implementation lives in normal source files such as `color_conversion.rs`, `conversion_workflow.rs`, `icc_conversion.rs`, etc.
2. `src/lib.rs` declares those modules so unit/conformance tests compile and exercise the canonical implementation in the library crate.
3. When GUI wiring begins, `src/main.rs` declares the same canonical module files in the binary crate and GUI code uses the binary-local `crate::model`, `crate::tiff_io`, and conversion types.
4. No conversion value containing model/profile types is passed across the binary/library crate boundary.
5. Logic is never copied into UI modules; only module declarations are duplicated, exactly as the project already does for existing backends such as `model`, `export`, `tiff_io`, `safe_fs`, and others.

This is a pragmatic compatibility decision, not a claim that duplicate crate roots are the ideal long-term architecture.

## Why not refactor the entire application now?

Making the binary consume all backend modules through the library crate would touch a large, production-hardened surface unrelated to Color Conversion. It would create unnecessary migration risk while the conversion subsystem itself is still being implemented.

A broader crate-boundary cleanup can be evaluated separately after Color Conversion is production-stable.

## UI integration rule

The first PR that wires Color Conversion into the native GUI must add the required conversion module declarations to `src/main.rs` and compile them against the binary-local backend types.

Examples of modules expected to be declared as they become used by the GUI:

```text
color_conversion
conversion_capabilities
conversion_preflight
conversion_workflow
conversion_presets
conversion_recipe
icc_conversion
nchannel_icc
separation_optimizer
conversion_analytics
```

Only modules actually present in `main` at that point should be declared.

## Test parity rule

A conversion behavior is not accepted merely because it works in a GUI-local code path. Production conversion semantics must remain implemented in canonical backend files that are also declared by `src/lib.rs` and covered by unit/conformance tests.

The GUI may own presentation state and orchestration, but it may not own alternate profile conversion, separation, constraint, provenance, or TIFF-writing math.

## Consequences

- No incompatible `ShadeProject`/`IccProfileIdentity` values cross crate roots.
- No broad crate refactor is required before Color Conversion UI work.
- The same backend source implementation is compiled for tests and GUI, reducing semantic drift.
- Module declarations are intentionally duplicated between crate roots, while implementation is not.
