# Backend module ownership

Issue: #396

The application must continue to ship as one standalone `ShadeEditor.exe`. The package library is an internal Rust compile-time/static-link boundary only; this cleanup must not introduce a runtime DLL, sidecar process, or other deployment dependency.

## Why this exists

`src/main.rs` and `src/lib.rs` currently declare a set of the same backend source modules. When both crates compile the same implementation file directly, Rust treats them as independent module/type domains and repeats compilation/codegen work.

Migration is intentionally incremental. Existing binary paths such as `crate::dpi::...` may temporarily remain as thin compatibility facades that re-export the canonical library-owned implementation. A facade is acceptable when it contains no backend implementation of its own and therefore does not create a second type domain.

## Inventory

| Module | Classification | Ownership status | Notes |
| --- | --- | --- | --- |
| `color_conversion` | shared backend/domain | duplicate implementation pending | High-risk/high-fan-out; migrate after lower-level shared types are canonical. |
| `conversion_tiff` | shared TIFF/conversion backend | duplicate implementation pending | Depends on canonical `dpi`, `safe_fs`, `tiff_io`, and `tiff_output`; migrate after this `tiff_io` batch validates. |
| `custom_optimizer_config` | shared domain/config | library-owned | Binary module is a compatibility facade; implementation is `custom_optimizer_config_impl.rs`. |
| `dpi` | shared TIFF/domain utility | library-owned | Binary `dpi.rs` is a compatibility facade; implementation is `dpi_impl.rs`. |
| `export` | shared export backend | duplicate implementation pending | High fan-out; preserve row streaming and export semantics. |
| `export_recipe` | shared export/domain | duplicate implementation pending | Depends on `model`; do not migrate separately while `ShadeProject` still has duplicate ownership. |
| `model` | shared domain | duplicate implementation pending | Highest type-identity sensitivity; migrate only with focused validation. |
| `palette` | shared domain/config | library-owned | Binary `palette.rs` is a compatibility facade; implementation is `palette_impl.rs`. |
| `production_project` | shared production/domain | duplicate implementation pending | Preserve project compatibility and persistence semantics. |
| `safe_fs` | shared IO/safety backend | library-owned | Binary module is a facade; implementation is `safe_fs_impl.rs`. Its `staging` and TIFF performance modules are canonical library-owned children. |
| `source_tiff_writer` | shared TIFF backend | duplicate implementation pending | Depends on `conversion_tiff::lzw_strip_writer` plus canonical TIFF types; migrate with/after `conversion_tiff`. |
| `source_transparency` | shared source/domain | library-owned | Binary `source_transparency.rs` is a compatibility facade; implementation is `source_transparency_impl.rs`. |
| `tiff_output` | shared TIFF backend | library-owned | Binary module is a facade; implementation is `tiff_output_impl.rs` and consumes canonical `safe_fs`. |
| `tiff_io` | shared TIFF backend/type domain | library-owned in current batch | Binary module is a facade; implementation remains byte-identical in `tiff_io_impl.rs`, giving metadata/decode/streaming types one canonical domain. |

## Additional duplicate removed inside the library

`safe_fs` already owns `staging.rs` as a child module. The previous library root also declared `pub mod staging;`, compiling the same source file a second time inside the library. The root now re-exports `safe_fs::staging` instead, so there is one canonical staging registry while existing `crate::staging::...` library paths remain valid.

## TIFF IO boundary review

The first full binary test exposed one intentional crate boundary that source-only inventory did not surface reliably: the binary TIFF inspector calls `declared_ink_names`, which is `pub(crate)` inside the canonical TIFF IO implementation. That visibility worked only while `tiff_io` was also compiled inside the binary crate.

The parser itself remains byte-identical and crate-private. `tiff_io_inspection::declared_ink_names` is a narrow public forwarding API in the library, and the binary compatibility facade re-exports it at the historical `crate::tiff_io::declared_ink_names` path. No raw-tag parsing logic is duplicated, and no TIFF metadata/decode type is mirrored or converted.

This preserves TIFF decoding, Photoshop spot-channel handling, planar/tiled streaming, metadata extraction, InkNames validation, and polarity behavior; only crate/module ownership and the necessary cross-crate inspection surface change.

## Migration rules

1. The canonical implementation for a migrated backend module is compiled by the package library exactly once.
2. Binary compatibility facades may only re-export library-owned public API; they must not contain duplicate backend logic or mirrored types.
3. GUI/orchestration-only code stays binary-owned.
4. Do not change persisted project, recipe, TIFF, ICC, queue, recovery, Test Stack, or conversion semantics as part of ownership migration.
5. Do not introduce dynamic linking or any additional runtime dependency for the main executable.
6. Each migration batch must pass the existing Windows Draft/full CI gates before the next high-risk domain is moved.

## Current migration sequence

1. `dpi`, `palette`, `source_transparency`, `custom_optimizer_config` — merged first self-contained ownership batch.
2. `safe_fs`, root `staging`, `tiff_output` — merged second storage-boundary batch.
3. `tiff_io` — current focused type-domain batch.
4. `conversion_tiff` + `source_tiff_writer` — migrate together/serially after `tiff_io`; their crate-private LZW writer dependency must remain inside the library boundary.
5. `model`, `export_recipe`, `export`, `production_project`, `color_conversion` — migrate in coordinated higher-fan-out batches because shared domain types cross their public APIs.
