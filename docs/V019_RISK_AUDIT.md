# v0.19 Interaction and Reliability Risk Audit

This audit accompanies the v0.19 QoL work. It distinguishes code/CI evidence from checks that require a real Windows/production environment.

## Fixed and regression-covered

- Keyboard focus leakage: editor shortcuts are routed through explicit input contexts; Curve Arrow keys remain owned by the Curve editor while text/modal contexts suppress inappropriate commands.
- Curve point lifecycle: optional midpoint Delete/Backspace and Home identity reset preserve point constraints and valid selection state.
- Face-removal stale preview risk: removing a Face invalidates all surviving render generations before a shifted index can accept an old worker result.
- Async Save/autosave race: save workers carry the serialized project revision and may clear dirty state only when no newer edit exists.
- Export Queue non-finite progress: NaN/Inf progress is sanitized before UI geometry/progress rendering; queue state transitions have regression coverage.
- Recent-project opening uses the centralized project lifecycle guard rather than bypassing Save/Discard/Cancel.

## Existing guards verified in code

- Snapshot and color-management preview invalidation use render generations. `poll_render` discards a completion when its generation no longer matches the current Face generation.
- Curve pointer interaction uses egui logical coordinates inside the allocated graph rectangle; TIFF physical DPI metadata is not used for Curve hit testing.
- Curve dragging does not maintain an application-level persistent drag latch; egui pointer response owns the drag lifecycle per frame.
- Numeric adjustment widgets use egui widget interaction; no application-level raw mouse-wheel mutation path was found for Levels/Curve numeric values.

## Requires manual environment validation

These are not marked as CI-passed because CI cannot truthfully reproduce the production environment:

- Move the application between Windows displays at 100%, 125%, 150% and 200% scaling and verify Curve point hit-testing/dragging after each DPI transition.
- Relink/restore projects and Faces through an actual UNC/network share, including temporary disconnect/reconnect behavior.
- Continue the existing production acceptance checks for Photoshop/RIP interpretation, >4 GiB BigTIFF and clean-workstation Shell integration.
