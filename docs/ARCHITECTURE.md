# Shade Editor architecture

This document is the current hand-off map for developers and AI agents.

## Hard invariants

1. The application is a native Windows desktop program. Do not introduce WebView, Electron, Tauri web front-ends or browser-hosted UI.
2. Source TIFF Face files are immutable inputs. `.shade` stores recipes/references; export creates/replaces output TIFFs only through the export backend.
3. Never hard-code production adjustment logic to exactly four channels. RGB/CMYK base channels may be followed by zero or more additional/Spot channels.
4. Real TIFF channel names/order remain authoritative. Palette aliases are UI-only.
5. UI code must not become the TIFF parser, export engine or ICC engine. Keep IO/model/render/export/color-management code independently testable.
6. Preview ICC assignment must never leak into `export.rs`. Export preserves the source embedded ICC payload and operates on adjustment output samples, not screen RGB.
7. Do not claim Photoshop/RIP compatibility without round-trip testing on real production files.

## Active source layout

Legacy version-suffixed implementations were removed. Active modules have canonical names:

- `main.rs` — egui UI and application orchestration.
- `model.rs` — schema-v9 `.shade` model, Snapshots, adjustments, Test Code and project preview-color settings.
- `color_management.rs` — embedded/assigned ICC preview transforms and installed Windows profile discovery.
- `tiff_io.rs` — TIFF decode, channel discovery, Photoshop resources, Spot polarity and metadata.
- `render.rs` — preview adjustment pipeline, clipping estimates and RGB/Spot composition.
- `snapshot_preview_cache.rs` — bounded in-memory LRU for per-Snapshot/Face/display-mode preview textures and compact histogram/clipping state.
- `export.rs` — full-resolution production TIFF renderer/writer.
- `validation.rs` — production round-trip comparison.
- `settings.rs` — application-only preferences such as layout, diagnostics, palettes and export defaults.
- `previous_shades.rs` — Project View cache/search/inspection.
- `recovery.rs` — rotating recovery states.
- `update.rs` — isolated self-update subsystem.
- `workflow.rs` — missing-Face/relink UI helpers.

`lib.rs` exposes the production backend required by TIFF conformance tests. `Cargo.toml` explicitly builds `src/main.rs` as `ShadeEditor`.

## `.shade` model

Schema v9 remains the clean-break format. New fields that are safe to default can be added with `#[serde(default)]` without forcing a schema bump; incompatible semantic changes still require incrementing `SHADE_SCHEMA_VERSION`.

`ShadeProject::preview_color` is project-wide and contains:

- enabled/disabled color-managed preview;
- optional assigned ICC/ICM path (`None` means the TIFF embedded profile);
- rendering intent;
- optional black-point compensation.

These values are not part of Snapshot adjustment history because they describe the project viewing environment rather than a shade recipe. They are saved in `.shade` so reopening the project reproduces the preview setup when the referenced profile is available.

## Editing interaction invariants

- `Master` is a separate Levels/Curve adjustment layer, never a broadcast copy into channel controls.
- `~` toggles Master and selected-channel scope. Selecting any channel exits Master.
- A dirty active Snapshot must be resolved through Stay / Discard / Update before switching Snapshots or saving the project. Discard also removes the discarded redo branch from Snapshot history.
- Light/Pigment is an application display preference only. Pigment mirrors Curve coordinates and histogram bins for Photoshop-style ink interpretation while the stored Curve, render math and export recipe remain unchanged.

## Adjustment/render data flow

Production adjustment order:

```text
TIFF source samples
  -> Levels
  -> N×N Channel Mixer
  -> Curve
  -> export sample
```

Preview reuses the adjusted downsampled planes, then performs display conversion:

```text
adjusted base RGB/CMYK/Gray
  -> embedded ICC or assigned preview ICC
  -> LittleCMS intent (+ optional BPC)
  -> sRGB
  -> declared Photoshop Spot DisplayInfo composite
  -> egui texture
```

Solo-channel view intentionally remains an engineering separation view, not a colorimetric composite.

Assigned ICC is an **input/source-profile override for preview**. Printer/RIP Soft Proof is a separate project-owned output-device ICC using a LittleCMS proofing transform. Both remain display-only and are forbidden inputs to `export.rs`.

## Snapshot preview cache

Repeated Snapshot comparison is session-local and render-cached. After the first successful render of a clean saved Snapshot, the adjusted preview texture plus compact histogram/clipping/color-status state is cached by Snapshot ID, Face index and Composite/Solo display mode. Switching back to a cached state reuses that immutable texture immediately instead of re-running the adjustment and color-management pipeline.

Dirty uncommitted edits never replace the saved Snapshot cache entry. `Update` explicitly refreshes the active Snapshot's cache with its latest committed render. Project/session transitions, Face topology or relink changes, preview rebuilds and ICC/display-preview invalidation clear runtime cache state. The cache is memory-only, never serialized into `.shade`, and is bounded to 32 entries / approximately 256 MiB with LRU eviction.

## ICC profile catalog

`color_management::installed_profiles()` scans the standard Windows color-profile directory (`%WINDIR%\System32\spool\drivers\color`) for `.icc`/`.icm`, opens each valid profile with LittleCMS and records its description, base color space and device class. UI assignment is allowed only when the profile color space matches the active TIFF base model. Browse permits valid compatible profiles outside the system directory.

The Color Management window follows Project View's search/navigation behavior: typing focuses/updates search, Up/Down changes the compatible selection and Enter assigns it.

## TIFF / Spot rules

`tiff_io.rs` retains ICC tag 34675, Photoshop Image Resources 34377 and ImageSourceData 37724. Photoshop DisplayInfo resource 1077 drives declared Spot display color/Solidity. Known Alpha channels are not composite printing inks.

Photoshop Spot samples are normalized internally to ink-coverage polarity and converted back for export. Do not change this contract without fixtures and production validation.

## DPI

`dpi.rs` owns physical-resolution parsing and fallback. `AppSettings::default_dpi` defaults to 220 DPI. `DpiInfo::used_default` distinguishes source DPI from fallback. Do not introduce a 72-DPI fallback in UI, Test Code or export.

## Channel palettes

`palette.rs` owns built-in palettes; `settings.rs` owns custom palette library/default choice; `ShadeProject::channel_palette` stores the project snapshot. Palette names/colors are presentation only. TIFF names/order, adjustment keys, mixer keys, Test Code channel IDs and export metadata always use real source channel names.

## Production export boundary

`export.rs` applies full-resolution adjustments and Test Code, preserves approved metadata, uses bounded streaming/spooling paths, selects/preserves BigTIFF when required and commits output atomically. Preview ICC settings are forbidden inputs to this module.

## Remaining production validation

- Reopen identity exports in Photoshop and the production RIP and confirm Spot type/order/name, DisplayInfo, ICC, DPI and press interpretation.
- Keep regression fixtures/baselines for each production TIFF family.
- Validate large BigTIFF and tiled/planar production artwork.
- Validate Windows Shell install/upgrade/removal on clean workstations.


## Export queue

`export_queue.rs` owns queued TIFF exports independently from UI state. Every queue item captures an immutable clone of the project/snapshot recipe at enqueue time. The existing export backend still writes a temporary TIFF and atomically replaces the destination only after successful completion. Waiting items cancel immediately; a Processing cancel is a safe stop-after-current request so the atomic export is never interrupted into a partial destination.

## Production TIFF inspector

`tiff_inspect.rs` is read-only. It combines decoded TIFF metadata with raw transport tags for diagnostics and never loads an inspected file into the editing project.


## Export worker isolation (v0.18.5)

Production Release builds use unwind semantics so a panic in a background worker cannot abort the entire GUI process. Export Queue workers convert panics into normal Failed completions and the global panic hook records details in `%LOCALAPPDATA%/ShadeEditor/shade-editor.log`. The large raw streaming spool is always created under the local ShadeEditor application-data directory before memory mapping; UNC/network destinations only receive the final temporary TIFF and atomic commit. This keeps the memory-mapped backing file local while preserving bounded-RAM export and network output support.
