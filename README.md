# Shade Editor

Native Windows shade editor for multi-channel TIFF artwork used in digital ceramic printing.

Shade Editor keeps source TIFF Faces immutable and stores non-destructive shade recipes and project preview settings in a `.shade` file beside the artwork. The application is native Rust/egui; there is no WebView/Electron runtime.

## Current features

- Open multiple TIFF files as project **Faces** and switch between them.
- Dynamic channel model for RGB/CMYK plus additional/Spot separations.
- Composite preview and isolated separation preview.
- Per-channel histogram, Levels, Curve and N×N Channel Mixer.
- Preview clipping diagnostics for Levels/Curve.
- Photoshop Spot DisplayInfo color/Solidity parsing; declared Alpha channels are excluded from the printing composite.
- ICC-aware preview with embedded-profile support and non-destructive **preview profile assignment**.
- Per-Face **Production Source ICC assignment** is stored separately from preview settings, hash-verified, color-space checked and consumed by Color Conversion preflight without changing source pixels.
- Production **Target Setup** validates Output ICC/DeviceLink class, exact profile identity, CMYK/5C–12C topology, authoritative channel order, output precision and a non-destructive TIFF destination before a recipe can become ready.
- Schema-v9 projects can persist explicit Source/Production roles, reciprocal project links and exact per-Face conversion provenance; legacy projects remain Standalone by default.
- The conversion backend can stage and verify bounded-memory 8/16-bit CMYK or 5C–12C TIFF/BigTIFF output with target ICC and standard ink-topology tags before an atomic destination commit. Its current strip path is uncompressed; Photoshop-specific spot metadata and real RIP acceptance are still validation gates.
- Conversion jobs have an immutable, serialization-safe capture and an explicit transaction contract: cancellation is safe before TIFF commit, while a project-save failure after commit returns recoverable output/project state instead of deleting the production TIFF. Pixel-worker and persistent queue wiring are still in progress.
- True printer/RIP **Soft Proof** using an output-device ICC proofing transform; proof settings remain preview-only.
- Hold the middle mouse button over the viewport for a cached original-source preview using only the TIFF embedded ICC; right mouse remains the current-color-management BEFORE preview.
- Searchable installed Windows ICC/ICM profile list, keyboard navigation, rendering intent and optional black-point compensation.
- Assigned preview/proof profiles are saved in `.shade`; TIFF ICC bytes and source/export samples are never changed by them.
- Color-managed project thumbnails use the same assigned profile and printer/RIP soft-proof transform as the viewport.
- Export Queue provides Waiting / Processing / Done / Failed states with safe cancel/retry controls.
- Export Queue supports pause/resume for waiting work, batch retry/cleanup, compact progress with ETA/throughput, and restart-safe recovered jobs.
- Faces can be marked Accepted or Rejected; Rejected Faces remain traceable in the project but are excluded from Export All by default.
- Quick Relative Adjustments provide cumulative Warmer/Cooler/Richer/Lighter/Redder/Beige tuning plus editable custom presets without overwriting the current recipe.
- Saved projects use revision-safe smart autosave while preserving Snapshot dirty-state protections and crash recovery.
- Export filename/folder templates support `{project}`, `{face}`, `{snapshot}`, `{date}` and `{source}`.
- `File > Inspect TIFF` reports production transport metadata and can copy a diagnostic report.
- Optional test-code raster in one separation or all separations.
- Export current Face or all Faces with production-oriented metadata preservation.
- BigTIFF selection/preservation, bounded strip/tile/planar processing and atomic destination replacement.
- Production round-trip validator for sample and critical TIFF/Photoshop metadata comparison.
- Persistent Project View, Snapshots, adjustment history, recovery and Windows Explorer Shell integration.

## `.shade` projects

A `.shade` file contains project state, adjustment recipes, preview color settings, cached metadata and a compact thumbnail. It does not contain TIFF pixel data.

```text
Moonstone/
├─ moonstone-face1.tif
├─ moonstone-face2.tif
├─ moonstone-face3.tif
└─ moonstone-test-1.shade
```

Schema version 9 is the current clean project format. New optional fields use Serde defaults so compatible metadata can be added without changing the schema number. Source TIFF paths are stored relative to the `.shade` file when possible. Preview ICC and per-Face Production Source ICC references are distinct; neither embeds profile payload bytes.

## Adjustment pipeline

For every channel the production adjustment order is:

```text
Source sample -> Levels -> Channel Mixer -> Curve -> Export sample
```

The mixer is dynamic: every discovered channel can contribute to every output channel. No production adjustment is hard-coded to four channels.

## Preview color management

Composite preview uses a separate display-only pipeline:

```text
Adjusted base RGB/CMYK/Gray samples
  -> embedded ICC OR assigned preview ICC
  -> selected rendering intent (+ optional BPC)
  -> sRGB preview
  -> Photoshop Spot DisplayInfo composite
```

Click the ICC/profile name beside the active Face metadata to open **Color Management / Preview Profile**. The window lists compatible ICC/ICM profiles from the Windows color-profile directory, supports search plus Up/Down/Enter navigation, and also allows browsing to another `.icc`/`.icm` file.

`Use embedded profile` returns the project to the TIFF profile. Assigning another profile reinterprets the base channel values only for Shade Editor's preview. It does **not** assign/write that profile into the TIFF and export does not consume the preview transform.

When a printer/RIP output ICC is selected and Soft Proof is enabled, Shade Editor uses a LittleCMS proofing transform before the sRGB display conversion. This is still display-only: no proof profile is embedded into or applied to exported TIFF samples.

## TIFF compatibility scope

The production-oriented path targets 8-bit and 16-bit RGB/CMYK TIFF with optional additional samples/Spot Channels. ICC tag 34675, Photoshop Image Resources 34377 and ImageSourceData 37724 are retained where supported. Photoshop/RIP interoperability must still be validated with representative production TIFFs; see `docs/PRODUCTION_VALIDATION.md`.

## Build and test

Requirements:

- Windows 10/11 x64
- Stable Rust toolchain with `x86_64-pc-windows-msvc`
- Visual Studio Build Tools / MSVC C++ tools

```powershell
cargo check --target x86_64-pc-windows-msvc
cargo test --target x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc
```

Executable:

```text
target\x86_64-pc-windows-msvc\release\ShadeEditor.exe
```

The repository CI uploads validation/build artifacts; project development does not require publishing GitHub Releases.

## Project structure

```text
src/
├─ main.rs              Native UI and application orchestration
├─ model.rs             .shade schema and adjustment/project model
├─ color_management.rs  ICC preview transform and Windows ICC catalog
├─ tiff_io.rs           TIFF decode/channel/Photoshop metadata discovery
├─ conversion_tiff.rs   Atomic CMYK/N-channel conversion TIFF writer
├─ conversion_transaction.rs  Immutable job/commit/recovery contract
├─ render.rs            Non-destructive preview render pipeline
├─ export.rs            Full-resolution TIFF export
├─ validation.rs        Production round-trip validation
├─ settings.rs          Application-only persistent preferences
├─ previous_shades.rs   Project View history/index
├─ recovery.rs          Crash recovery
├─ update.rs            Update subsystem
└─ workflow.rs          Missing-Face/relink workflow helpers
```

See `docs/ARCHITECTURE.md` for invariants and extension points.

## License

MIT License. Copyright © 2026 Emad Ghasemi.
