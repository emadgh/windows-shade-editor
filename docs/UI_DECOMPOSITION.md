# UI decomposition

Shade Editor keeps the application root responsible for application lifecycle/orchestration while progressively moving cohesive egui rendering into `src/ui`.

## Current extracted UI modules

- `src/ui/adjustments.rs`: History, Channels/Histogram, Quick Relative Adjustments and adjustment editor composition.
- `src/ui/export_queue.rs`: Export Queue window presentation and queue interaction surface.
- `src/ui/status_bar.rs`: save-state and bottom status presentation.

The extraction is intentionally incremental. These modules are descendants of the crate root and extend `ShadeApp` with `pub(crate)` inherent methods, so they can reuse existing safety/controller methods without duplicating backend logic. Cross-cutting business rules remain in controllers/model/workflow modules.

This pass reduced `src/main.rs` from 8627 to 7458 lines. Future #40 passes should continue with Faces, Curve-specific UI and Recent/Project View, then replace direct cross-domain field access with typed UI actions where that meaningfully improves boundaries.
