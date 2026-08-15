from pathlib import Path

root = Path(__file__).resolve().parents[2]


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    path.write_text(text.replace(old, new, 1), encoding="utf-8")

# Version consistency.
replace_once(root / "Cargo.toml", 'version = "0.18.6"', 'version = "0.19.0"', "Cargo version")
(root / "VERSION").write_text("0.19.0\n", encoding="utf-8")
replace_once(
    root / "Cargo.lock",
    'name = "windows-shade-editor"\nversion = "0.18.6"',
    'name = "windows-shade-editor"\nversion = "0.19.0"',
    "Cargo.lock root package version",
)

release_notes = root / "RELEASE_NOTES.md"
notes = release_notes.read_text(encoding="utf-8")
header = '''# Shade Editor 0.19.0

- Centralize keyboard/focus ownership so Curve/editor shortcuts no longer leak into text fields or modal workflows; keep Curve Arrow/Shift+Arrow editing while adding Delete/Backspace midpoint removal and Home identity reset.
- Add revision-safe smart `.shade` autosave after short edit inactivity for already-saved projects, while keeping the existing crash-recovery autosave as a separate layer and preserving Snapshot dirty guards.
- Add application-internal adjustment Copy/Paste plus a default-collapsed Relative Presets panel with cumulative Warmer, Cooler, Darker/Richer, Lighter, Redder and More beige actions and editable custom per-channel presets.
- Add persistent Accepted/Rejected Face workflow with right-click Accepted/Rejected/Delete actions, red Rejected treatment, selection warning, Rejected-last display grouping and Export All exclusion; direct rejected-Face export requires confirmation.
- Harden Face removal against stale background preview completion by invalidating generations after index shifts.
- Upgrade Export Queue QoL with persisted Pause/Resume for waiting work, Retry all failed, separate completed/failed clearing, finite progress sanitization, elapsed/ETA/approximate throughput and compact toolbar status.
- Add `File > Recent projects` backed by Project View history while preserving the centralized Save/Discard/Cancel lifecycle guard.
- Continue architecture decomposition with focused `input_router`, `adjustment_tools` and `project_autosave` modules instead of expanding the application shell with more cross-cutting logic.
- Add regression coverage for focus routing, Curve point lifecycle, relative preset accumulation, adjustment clipboard constraints, legacy Face status/defaults, rejected export filtering, autosave eligibility/revision safety and queue state/progress behavior.

'''
if notes.startswith("# Shade Editor 0.19.0"):
    raise SystemExit("0.19.0 release notes already exist")
release_notes.write_text(header + notes, encoding="utf-8")

readme = root / "README.md"
text = readme.read_text(encoding="utf-8")
anchor = "- Export Queue provides Waiting / Processing / Done / Failed states with safe cancel/retry controls.\n"
addition = anchor + "- Export Queue supports pause/resume for waiting work, batch retry/cleanup, compact progress with ETA/throughput, and restart-safe recovered jobs.\n- Faces can be marked Accepted or Rejected; Rejected Faces remain traceable in the project but are excluded from Export All by default.\n- Quick Relative Adjustments provide cumulative Warmer/Cooler/Richer/Lighter/Redder/Beige tuning plus editable custom presets without overwriting the current recipe.\n- Saved projects use revision-safe smart autosave while preserving Snapshot dirty-state protections and crash recovery.\n"
if anchor not in text:
    raise SystemExit("README feature anchor not found")
text = text.replace(anchor, addition, 1)
readme.write_text(text, encoding="utf-8")

# Record what was actually proven vs. what remains a real-environment check.
audit = '''# v0.19 Interaction and Reliability Risk Audit

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
'''
(root / "docs" / "V019_RISK_AUDIT.md").write_text(audit, encoding="utf-8")

# Integration invariant: all runtime edit paths must go through the revision-aware helpers.
for relative in ["src/main.rs", "src/workflow.rs"]:
    source = (root / relative).read_text(encoding="utf-8")
    for line_no, line in enumerate(source.splitlines(), start=1):
        stripped = line.strip()
        if stripped == "self.project_dirty = true;" and relative == "src/main.rs":
            # Allowed only inside mark_project_dirty(). Confirm nearby function name.
            before = "\n".join(source.splitlines()[max(0, line_no - 8):line_no])
            if "fn mark_project_dirty" not in before:
                raise SystemExit(f"direct dirty=true bypass at {relative}:{line_no}")
        if stripped == "self.project_dirty = false;" and relative == "src/main.rs":
            before = "\n".join(source.splitlines()[max(0, line_no - 8):line_no])
            if "fn mark_project_saved" not in before:
                raise SystemExit(f"direct dirty=false bypass at {relative}:{line_no}")
        if relative == "src/workflow.rs" and ("project_dirty = true" in stripped or "project_dirty = false" in stripped):
            raise SystemExit(f"workflow bypasses revision-aware dirty helpers at {relative}:{line_no}")

# Bootstrap cleanup: validated feature source remains, one-off integration machinery does not.
Path(__file__).unlink()
workflow = root / ".github" / "workflows" / "apply-v019-integration.yml"
if workflow.exists():
    workflow.unlink()
