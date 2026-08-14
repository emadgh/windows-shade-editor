from pathlib import Path

p = Path("src/main.rs")
t = p.read_text(encoding="utf-8")

# The primary migration intentionally does broad, readable replacements. These
# cover fluent/multiline field accesses that do not contain the literal `self.field`.
old_import = "use project_lifecycle::{\n    ProjectLifecycleController, ProjectTransition, TransitionRequest,\n};"
new_import = "use project_lifecycle::{\n    BackupRestoreCandidate, ProjectLifecycleController, ProjectTransition, TransitionRequest,\n};"
if t.count(old_import) != 1:
    raise RuntimeError(f"BackupRestoreCandidate import: expected 1, found {t.count(old_import)}")
t = t.replace(old_import, new_import, 1)

replacements = [
    ("self\n            .export_queue", "self\n            .export\n            .queue"),
    ("self\n            .tiff_inspection", "self\n            .inspector\n            .inspection"),
    ("self\n            .icc_profile_selected", "self\n            .color\n            .selected"),
]
for old, new in replacements:
    count = t.count(old)
    if count != 1:
        raise RuntimeError(f"multiline routing {old!r}: expected 1, found {count}")
    t = t.replace(old, new, 1)

# BTreeSet is no longer owned by main after ExportController extraction.
t = t.replace(
    "use std::collections::{BTreeMap, BTreeSet, VecDeque};",
    "use std::collections::{BTreeMap, VecDeque};",
    1,
)

p.write_text(t, encoding="utf-8")
