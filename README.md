# Shade Editor

Native Windows shade editor for multi-channel TIFF artwork used in digital ceramic printing.

Shade Editor keeps source TIFF faces unchanged and stores color-adjustment recipes in a small `.shade` project beside the artwork. The first version is intentionally focused on shade matching rather than general image editing.

## Current features

- Native Rust desktop UI; no WebView or Electron runtime.
- Open multiple TIFF files as project **Faces** and switch between them.
- Dynamic channel list for CMYK plus additional channels/spot separations.
- Composite preview and isolated single-channel preview.
- Per-channel histogram, including original and adjusted preview.
- Per-channel **Levels**.
- Per-channel **Curve**.
- N×N **Channel Mixer**, including spot channels as inputs and outputs.
- Non-destructive `.shade` projects with portable relative TIFF paths where possible.
- Optional test-code raster in the selected output channel.
- Export current Face or all Faces as TIFF.
- Preserve source ICC profile and Photoshop Image Resources when available.
- Preserve extra TIFF samples when exporting RGB/CMYK plus additional channels.
- Parse Photoshop Spot display color/Solidity for preview and distinguish declared Spot vs Alpha channels.
- Stream normal chunky strip TIFF preview/export with bounded working buffers.
- Atomic same-directory TIFF export replacement to avoid partial destination files.
- Automatic GitHub Release update checking/downloading with SHA-256 asset verification, enabled by default and disableable in Settings.
- About window with application version and manual update check.

## `.shade` projects

A `.shade` file contains project state and adjustment recipes, not TIFF pixel data. Keep it beside the Face files for the most portable layout:

```text
Moonstone/
├─ moonstone-face1.tif
├─ moonstone-face2.tif
├─ moonstone-face3.tif
└─ moonstone-test-1.shade
```

The format is versioned JSON. Shade Editor 0.10 uses schema version 9; pre-production v1-v8 migration support was intentionally removed in v0.9. Source paths are saved relative to the `.shade` file when possible.

## Color pipeline

For each channel, the first version processes:

```text
Source sample -> Levels -> Curve -> Channel Mixer -> Export sample
```

The mixer is dynamic: every discovered channel can contribute to every output channel. This avoids hard-coding color tools to four CMYK channels.

The on-screen composite is an engineering preview and is not yet a press/RIP color proof. Original TIFF files remain authoritative and are never modified in place.

## TIFF compatibility scope

The current production-oriented path targets 8-bit and 16-bit RGB/CMYK TIFF with optional additional samples/Spot Channels. It reads the embedded ICC profile and Photoshop Image Resources and copies them to exported files. Photoshop/RIP interoperability must still be validated against representative production TIFFs before relying on a new build in a production print workflow. See `docs/PRODUCTION_VALIDATION.md`.

## Build

Requirements:

- Windows 10/11 x64
- Stable Rust toolchain with `x86_64-pc-windows-msvc`
- Visual Studio Build Tools / MSVC C++ build tools

```powershell
cargo test --target x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc
```

The executable is created at:

```text
target\x86_64-pc-windows-msvc\release\ShadeEditor.exe
```

## Releases and automatic updates

GitHub Actions builds Windows artifacts on pull requests and `main`. Pushing a version tag such as `v0.1.0` also publishes a GitHub Release containing `ShadeEditor.exe`.

The application checks `emadgh/windows-shade-editor` Releases. With automatic updates enabled, a newer `ShadeEditor.exe` is downloaded to the temporary directory. The user is then offered **Restart and update**, which replaces the current executable after the app closes and relaunches it. Automatic checking/downloading can be disabled in **Settings**; manual checks remain available from **About**.

## Project structure

```text
src/
├─ main.rs       Native UI and application orchestration
├─ model.rs      .shade schema and color-adjustment model
├─ tiff_io.rs    TIFF decode, channel discovery, Photoshop resource parsing
├─ render.rs     Non-destructive preview render pipeline
├─ export.rs     Full-resolution adjustment and TIFF export
├─ settings.rs   Persistent application settings
└─ update.rs     GitHub Release updater
```

See `docs/ARCHITECTURE.md` for extension points and constraints for future development.

## License

MIT License. Copyright © 2026 Emad Ghasemi.
