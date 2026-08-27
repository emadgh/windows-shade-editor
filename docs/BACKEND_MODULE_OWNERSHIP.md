# Backend module ownership

Issue: #396

The application must continue to ship as one standalone `ShadeEditor.exe`. The package library is an internal Rust compile-time/static-link boundary only; this cleanup must not introduce a runtime DLL, sidecar process, or other deployment dependency.

## Why this exists

`src/main.rs` and `src/lib.rs` historically declared a set of the same backend source modules. When both crates compile the same implementation file directly, Rust treats them as independent module/type domains and repeats compilation/codegen work.

Migration is intentionally incremental. Existing binary paths such as `crate::dpi::...` may remain as thin compatibility facades that re-export the canonical library-owned implementation. A facade is acceptable when it contains no backend implementation of its own and therefore does not create a second type domain.

## Inventory

| Module | Classification | Ownership status | Notes |
| --- | --- | --- | --- |
| `color_conversion` | shared backend/domain | library-owned | Binary `color_conversion.rs` is a zero-logic facade. Canonical implementation is `color_conversion_impl/mod.rs`; provenance stays a canonical child module. |
| `conversion_tiff` | shared TIFF/conversion backend | library-owned | Implementation is `conversion_tiff_impl.rs`; its crate-private `lzw_strip_writer` remains inside the library boundary. |
| `custom_optimizer_config` | shared domain/config | library-owned | Binary module is a compatibility facade; implementation is `custom_optimizer_config_impl.rs`. |
| `dpi` | shared TIFF/domain utility | library-owned | Binary `dpi.rs` is a compatibility facade; implementation is `dpi_impl.rs`. |
| `export` | shared export backend | duplicate implementation pending | Final high-risk root migration: child row-streaming module, crate-private crop type, TIFF/source-writer dependencies, and source-scanning acceptance guards require a focused batch. |
| `export_recipe` | shared export/domain | library-owned in current batch | Binary `export_recipe.rs` is a zero-logic facade; canonical implementation is `export_recipe_impl.rs` and consumes the canonical model domain. |
| `model` | shared domain | library-owned | Binary `model.rs` is a zero-logic facade; implementation is `model_impl.rs`. Migrated with Color Conversion to preserve one type domain. |
| `palette` | shared domain/config | library-owned | Binary `palette.rs` is a compatibility facade; implementation is `palette_impl.rs`. |
| `production_project` | shared production/domain | library-owned in current batch | Binary `production_project.rs` is a zero-logic facade; canonical implementation is `production_project_impl.rs` and consumes canonical model + Color Conversion types. |
| `safe_fs` | shared IO/safety backend | library-owned | Binary module is a facade; implementation is `safe_fs_impl.rs`. Its `staging` and TIFF performance modules are canonical library-owned children. |
| `source_tiff_writer` | shared TIFF backend | library-owned | Implementation is `source_tiff_writer_impl.rs`; migrated with `conversion_tiff` so the private shared LZW writer stays crate-local. |
| `source_transparency` | shared source/domain | library-owned | Binary `source_transparency.rs` is a compatibility facade; implementation is `source_transparency_impl.rs`. |
| `tiff_output` | shared TIFF backend | library-owned | Binary module is a facade; implementation is `tiff_output_impl.rs` and consumes canonical `safe_fs`. |
| `tiff_io` | shared TIFF backend/type domain | library-owned | Binary module is a facade; implementation is `tiff_io_impl.rs`, giving metadata/decode/streaming types one canonical domain. |

## Additional duplicate removed inside the library

`safe_fs` already owns `staging.rs` as a child module. The previous library root also declared `pub mod staging;`, compiling the same source file a second time inside the library. The root now re-exports `safe_fs::staging` instead, so there is one canonical staging registry while existing `crate::staging::...` library paths remain valid.

## TIFF IO boundary review

The first full binary test exposed one intentional crate boundary that source-only inventory did not surface reliably: the binary TIFF inspector calls `declared_ink_names`, which is `pub(crate)` inside the canonical TIFF IO implementation. That visibility worked only while `tiff_io` was also compiled inside the binary crate.

The parser itself remains byte-identical and crate-private. `tiff_io_inspection::declared_ink_names` is a narrow public forwarding API in the library, and the binary compatibility facade re-exports it at the historical `crate::tiff_io::declared_ink_names` path. No raw-tag parsing logic is duplicated, and no TIFF metadata/decode type is mirrored or converted.

This preserves TIFF decoding, Photoshop spot-channel handling, planar/tiled streaming, metadata extraction, InkNames validation, and polarity behavior; only crate/module ownership and the necessary cross-crate inspection surface change.

## Conversion/source TIFF writer boundary

`source_tiff_writer` directly consumes `conversion_tiff::lzw_strip_writer::LzwStripWriter`. That child module is deliberately `pub(crate)` and should not become part of the public API merely to support an ownership refactor. Therefore `conversion_tiff` and `source_tiff_writer` move into the package library together. Their existing implementation blobs are reused unchanged, while the binary files become public-API re-export facades.

This keeps the shared LZW strip writer private, removes duplicate writer compilation, and ensures `source_tiff_writer` consumes the same canonical `dpi`, `safe_fs`, `tiff_io`, and `tiff_output` types as conversion output.

## Model + Color Conversion boundary

`model` and `color_conversion` are mutually type-sensitive: the project model stores project-role/linkage/provenance values defined by Color Conversion, while Color Conversion consumes model-owned ICC profile identity data. Moving only one module would leave a mixed binary/library type graph and recreate the type-identity problem #396 is removing.

They therefore migrate together. Their former implementation blobs are preserved as canonical library sources at `model_impl.rs` and `color_conversion_impl/mod.rs`; the historical root files become zero-logic binary compatibility facades. `production_provenance` moves with its parent module so there is no stale second source path.

The historical decision documented in `COLOR_CONVERSION_CRATE_BOUNDARY.md` to compile Color Conversion separately in both crate roots was explicitly temporary. That rule is now superseded for `model` and `color_conversion` without changing GUI `crate::model` / `crate::color_conversion` call sites.

## Export recipe + Production project boundary

With `model` and `color_conversion` canonicalized, the remaining recipe and Production project builders no longer need binary-local copies of those type domains. Their original implementation blobs are preserved unchanged as `export_recipe_impl.rs` and `production_project_impl.rs`, while the historical root files are zero-logic facades.

This batch intentionally stops before `export`. The export backend has a crate-private crop type, a row-streaming child module, and acceptance/benchmark checks that inspect source paths, so moving it independently keeps the final high-risk visibility and guard adjustments reviewable.

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
3. `tiff_io` — merged focused type-domain prerequisite batch.
4. `conversion_tiff` + `source_tiff_writer` — merged writer batch; both implementations are library-owned together to preserve the private LZW dependency.
5. `model` + `color_conversion` — merged high-sensitivity type-domain batch.
6. `export_recipe` + `production_project` — current bounded batch, now consuming the canonical model/Color Conversion domains.
7. `export` — final focused duplicate-root migration after this batch validates, with its child module, visibility boundary, and source-scanning guards handled explicitly.
