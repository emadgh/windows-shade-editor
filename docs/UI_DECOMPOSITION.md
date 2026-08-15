# UI decomposition

Issue #40 is implemented as incremental, build-gated extractions rather than a one-shot rewrite.

## Focused UI modules

- `src/ui/input_router.rs` — typed keyboard/focus context classification.
- `src/ui/curve_editor.rs` — Curve point state, graph interaction and Curve controls.
- `src/ui/adjustments.rs` — History, Channels/Histogram, relative presets and adjustment composition.
- `src/ui/faces.rs` — Face list/status/context-menu presentation; relink/loading logic remains in `workflow.rs`.
- `src/ui/export_queue.rs` — Export Queue presentation and queue interaction surface.
- `src/ui/status_bar.rs` — save-state and bottom status presentation.
- `src/ui/project_navigation.rs` — app menu/Recent Projects and Project View presentation.

The modules extend `ShadeApp` only where application-level orchestration is still required. Production TIFF/export/model safety remains in the existing controller/model/workflow boundaries; this refactor does not duplicate those rules in UI modules.

### Measured reductions

Second pass: `src/main.rs` 7458 -> 6347 lines; `src/workflow.rs` 803 -> 597 lines.

Further architecture work should focus on typed UI action return values for cross-domain mutations and narrower Project View/controller state, rather than moving code merely to reduce line counts.
