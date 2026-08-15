# Typed UI action boundary

Shade Editor's extracted egui modules render state and emit typed actions. Cross-domain operations are dispatched through `src/ui/actions.rs`, which delegates to the existing lifecycle/export/workflow safety paths.

## Current action domains

- `FaceUiAction` — rename/select/status/delete/relink.
- `NavigationUiAction` — project lifecycle, save/export, queue, inspector, settings/about/logs.
- `ProjectViewUiAction` — open/select/reveal/relink/remove Project View entries.
- `ExportQueueUiAction` — window state plus resume/pause/retry/cancel/clear/reveal intents for queued exports.
- `AdjustmentUiAction` — history navigation/clear, palette/channel/composite selection, settings persistence, preview invalidation and history commit side effects.

Export Queue presentation may read queue rows, progress, metrics and status counts, but mutation stays in the action dispatcher so persistence and atomic-export semantics remain owned by `ExportQueue`.

Presentation modules may still own local widget state and read application state. They should not duplicate production safety rules or directly invoke lifecycle/export/destructive operations when a typed action exists.

Adjustment controls still edit Levels/Curve/Mixer values locally. The typed boundary is intentionally limited to orchestration side effects so the UI does not duplicate history, rendering, settings-persistence or project mutation policy.
