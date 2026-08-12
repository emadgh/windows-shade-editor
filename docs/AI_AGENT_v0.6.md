# AI Agent Notes — Shade Editor v0.6

## DPI

`src/dpi.rs` owns physical-resolution parsing and fallback behavior. `DpiInfo::used_default` distinguishes a TIFF-derived physical DPI from the configured fallback. `AppSettings::default_dpi` defaults to 220. All TIFF opening paths pass the current setting to `dpi::read_dpi`. Export paths pass the same setting to `export_face_with_progress`; `export_v6.rs` writes the fallback resolution when source physical resolution is unavailable.

Do not reintroduce a hard-coded 72 DPI fallback in UI, Test Code sizing, or export.

## Channel palettes

`src/palette.rs` owns the palette data model and read-only international built-ins. `src/settings_v6.rs` owns the user's custom palette library and default-palette choice. `ShadeProject::channel_palette` in `src/model_v6.rs` is the portable per-project palette snapshot.

A palette is strictly a UI alias/color layer indexed by channel position. TIFF `channel_names`, sample order, Spot resources, Adjustment map keys, Mixer keys, Test Code target channel values, and export metadata must continue to use real source channel names.

UI code uses `channel_display_name(...)` and `channel_color(...)` to translate presentation only. If a palette has fewer slots than the active TIFF, fall back to the real TIFF channel name and deterministic fallback color.

Built-in palette IDs are `builtin:cmyk` and `builtin:rgb`; they are not stored in the editable custom list. The `builtin:auto` setting means new projects choose CMYK/RGB after the first Face is loaded. Once chosen, the concrete palette is stored in the `.shade` project.
