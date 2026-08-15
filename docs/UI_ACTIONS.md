# Typed UI action boundary

Shade Editor's extracted egui modules render state and emit typed actions. Cross-domain operations are dispatched through `src/ui/actions.rs`, which delegates to the existing lifecycle/export/workflow safety paths.

## Current action domains

- `FaceUiAction` — rename/select/status/delete/relink.
- `NavigationUiAction` — project lifecycle, save/export, queue, inspector, settings/about/logs.
- `ProjectViewUiAction` — open/select/reveal/relink/remove Project View entries.

Presentation modules may still own local widget state and read application state. They should not duplicate production safety rules or directly invoke lifecycle/export/destructive operations when a typed action exists.
