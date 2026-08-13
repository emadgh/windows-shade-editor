# Shade Editor architecture

This document is the hand-off map for future developers and AI agents.

## Design constraints

1. The application is a native Windows desktop program. Do not introduce WebView, Electron, Tauri web front-ends, or browser-hosted UI.
2. Source TIFF Face files are immutable inputs. `.shade` stores recipes and references; exporting creates new TIFF files.
3. Never hard-code color adjustments to exactly four channels. The base workflow is CMYK plus zero or more additional/spot channels.
4. Channel names are stable project keys where possible. Any future channel-ID migration must be schema-versioned.
5. UI code must not become the TIFF parser or color engine. Keep IO/model/render/export modules independently testable.
6. Do not claim Photoshop/RIP compatibility without round-trip tests using real production files.

## Modules

### `model.rs`
Owns the `.shade` project schema and adjustment domain model.

- `ShadeProject`: project root and schema version.
- `FaceRef`: source-file reference and display label.
- `ChannelAdjustment`: per-output-channel settings.
- `Levels`: black/gamma/white input and output mapping.
- `Curve`: compact v1 curve representation.
- `MixerRow`: N-input coefficient row plus constant for one output channel.
- `TestCodeConfig`: export raster settings.

`ShadeProject::ensure_channels` is the central invariant builder. Every discovered channel receives an adjustment and an identity mixer row.

Schema v9 is the current clean-break project format. Future schema changes should increment `SHADE_SCHEMA_VERSION`; add migration only when backward compatibility is explicitly required.

### `tiff_io.rs`
Owns TIFF decoding and source metadata discovery.

Current responsibilities:

- decode 8/16-bit TIFF samples;
- normalize samples to internal `u16`;
- account for planar configuration;
- discover CMYK base channels and extra channels;
- retain ICC tag 34675;
- retain Photoshop Image Resources tag 34377;
- parse Photoshop channel-name resources 1006/1045;
- build downsampled preview planes and histograms.

Normal chunky strip TIFFs are decoded strip-by-strip for preview. Planar/tiled inputs retain the full-decode compatibility path. Keep the public `PreviewFace` boundary so UI code remains independent from decoding strategy.

### `render.rs`
Pure preview processing.

Pipeline:

1. normalize each source channel;
2. Levels;
3. Curve;
4. N×N mixer;
5. convert adjusted planes to preview RGBA or isolated-channel grayscale.

The base RGB/CMYK composite conversion remains an engineering approximation, but Photoshop DisplayInfo 1077 Spot colors and Solidity are honored when present. Full ICC color-managed proofing should still live behind this module rather than leaking conversion into UI code.

### `export.rs`
Full-resolution destructive renderer for newly-created output files only.

It applies the same adjustment order as `render.rs`, can rasterize test code into a selected separation, and writes CMYK plus TIFF ExtraSamples. ICC and Photoshop resource payloads are copied from the source when present.

Current hardening:

- chunky strip inputs stream through preview and export;
- output preserves approved resolution/orientation/ICC/Photoshop metadata and source lossless compression/predictor intent;
- output is committed with same-directory atomic replacement;
- Photoshop DisplayInfo 1077 is parsed for Spot display semantics.

Remaining production validation:

- reopen representative no-adjustment exports in Photoshop and the actual RIP to confirm Spot semantics/order;
- maintain regression fixtures or metadata baselines for each production TIFF family;
- treat the preview as engineering simulation, not a press proof.

### `settings.rs`
Persists app-level preferences under `%LOCALAPPDATA%\ShadeEditor\settings.json`.

`auto_update` defaults to `true`. Keep manual update checking available even when automatic updates are disabled.

### `update.rs`
Self-update subsystem modeled after GahYar but isolated from the UI.

- checks GitHub `releases/latest` for `emadgh/windows-shade-editor`;
- compares version tags with the Cargo package version;
- downloads the `ShadeEditor.exe` release asset with WinHTTP;
- validates basic Windows executable shape;
- stages it in the temp directory;
- replaces/relaunches through a short PowerShell helper after the current process exits.

The updater must never replace the running executable while a project is still open. Auto-update downloads in the background, but install is an explicit **Restart and update** action.

Tagged Releases publish `ShadeEditor.exe.sha256`; the updater requires and verifies that SHA-256 digest before staging the executable. A future code-signing step can add publisher/authenticode verification on top.

### `main.rs`
Application orchestration and egui UI.

Major surfaces:

- toolbar: project/open/save/export/settings/about;
- left panel: Faces;
- center: active Face viewport;
- right panel: Channels, histogram, Levels/Curve/Mixer, Test Code;
- Settings window: automatic update and preview preferences;
- About window: app/version/repository/update status.

Keep expensive processing out of paint callbacks as the project grows. `RuntimeFace::dirty` is the current invalidation mechanism.

## Data flow

```text
TIFF source
  -> tiff_io::decode/load_preview
  -> RuntimeFace preview planes
  -> render::adjusted_planes
  -> viewport/histogram

.shade
  <-> model::ShadeProject

TIFF source + ShadeProject
  -> export::export_face
  -> adjusted TIFF output
```

## Release flow

`.github/workflows/build-windows.yml` checks, tests and builds the x64 MSVC target. On a `v*` tag it publishes `ShadeEditor.exe` as a GitHub Release asset. The updater expects that exact asset name.

When changing executable names, repository names or release naming, update both the workflow and `src/update.rs` in the same change.
