# Backend module ownership

Issue: #396

The application must continue to ship as one standalone `ShadeEditor.exe`. The package library is an internal Rust compile-time/static-link boundary only; this cleanup must not introduce a runtime DLL, sidecar process, or other deployment dependency.

## Why this exists

`src/main.rs` and `src/lib.rs` currently declare a set of the same backend source modules. When both crates compile the same implementation file directly, Rust treats them as independent module/type domains and repeats compilation/codegen work.

Migration is intentionally incremental. Existing binary paths such as `crate::dpi::...` may temporarily remain as thin compatibility facades that re-export the canonical library-owned implementation. A facade is acceptable when it contains no backend implementation of its own and therefore does not create a second type domain.

## Inventory

| Module | Classification | Ownership status | Notes |
| --- | --- | --- | --- |
| `color_conversion` | shared backend/domain | duplicate implementation pending | High-risk/high-fan-out; migrate after low-risk pattern is proven. |
| `conversion_tiff` | shared TIFF/conversion backend | duplicate implementation pending | Preserve TIFF/BigTIFF and conversion contracts. |
| `custom_optimizer_config` | shared domain/config | library-owned | Binary module is a compatibility facade; implementation is `custom_optimizer_config_impl.rs`. |
| `dpi` | shared TIFF/domain utility | library-owned | Binary `dpi.rs` is a compatibility facade; implementation is `dpi_impl.rs`. |
| `export` | shared export backend | duplicate implementation pending | High fan-out; preserve row streaming and export semantics. |
| `export_recipe` | shared export/domain | duplicate implementation pending | Depends on `model`; do not migrate separately while `ShadeProject` still has duplicate ownership. |
| `model` | shared domain | duplicate implementation pending | Highest type-identity sensitivity; migrate only with focused validation. |
| `palette` | shared domain/config | library-owned | Binary `palette.rs` is a compatibility facade; implementation is `palette_impl.rs`. |
| `production_project` | shared production/domain | duplicate implementation pending | Preserve project compatibility and persistence semantics. |
| `safe_fs` | shared IO/safety backend | duplicate implementation pending | Preserve atomic/safe filesystem behavior and TIFF performance exports. |
| `source_tiff_writer` | shared TIFF backend | duplicate implementation pending | Preserve source metadata and write behavior. |
| `source_transparency` | shared source/domain | library-owned | Binary `source_transparency.rs` is a compatibility facade; implementation is `source_transparency_impl.rs`. |
| `tiff_output` | shared TIFF backend | duplicate implementation pending | Preserve output/conformance behavior. |
| `tiff_io` | shared TIFF backend | duplicate implementation pending | Contains crate-visible internals; requires API-boundary review before migration. |

## Migration rules

1. The canonical implementation for a migrated backend module is compiled by the package library exactly once.
2. Binary compatibility facades may only re-export library-owned public API; they must not contain duplicate backend logic or mirrored types.
3. GUI/orchestration-only code stays binary-owned.
4. Do not change persisted project, recipe, TIFF, ICC, queue, recovery, Test Stack, or conversion semantics as part of ownership migration.
5. Do not introduce dynamic linking or any additional runtime dependency for the main executable.
6. Each migration batch must pass the existing Windows Draft/full CI gates before the next high-risk domain is moved.

## Current migration sequence

1. `dpi`, `palette`, `source_transparency`, `custom_optimizer_config` — first self-contained ownership batch.
2. TIFF/export surfaces — migrate in dependency order after checking crate-visible helpers; keep `export_recipe` paired with/after `model` ownership migration.
3. `model`, `production_project`, `color_conversion` — migrate last because they have the broadest type/API fan-out.
