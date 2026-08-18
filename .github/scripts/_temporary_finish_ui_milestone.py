from pathlib import Path


def replace(path: str, old: str, new: str, count: int = 1) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    actual = text.count(old)
    if actual != count:
        raise SystemExit(
            f"{path}: expected {count} occurrence(s), found {actual}: {old[:120]!r}"
        )
    p.write_text(text.replace(old, new, count), encoding="utf-8")


def patch_function(path: str, name: str, transform, required: bool = True) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    marker = f"    fn {name}("
    start = text.find(marker)
    if start < 0:
        if required:
            raise SystemExit(f"{path}: function {name} not found")
        return
    end = text.find("\n    fn ", start + len(marker))
    if end < 0:
        end = len(text)
    before = text[start:end]
    after = transform(before)
    if before == after:
        if required:
            raise SystemExit(f"{path}: function {name} produced no patch")
        return
    p.write_text(text[:start] + after + text[end:], encoding="utf-8")


# ---------------------------------------------------------------------------
# #171: Test Code is a Snapshot-test-export concern only.
# ---------------------------------------------------------------------------

def patch_start_export_all(seg: str) -> str:
    old = (
        "        let mut project = self.project.clone();\n"
        "        project.test_code.enabled = self.settings.export_all_test_code;\n"
    )
    if old not in seg:
        raise SystemExit("src/main.rs: legacy Export All Test Code assignment not found")
    return seg.replace(old, "        let project = self.project.clone();\n", 1)


patch_function("src/main.rs", "start_export_all", patch_start_export_all)


def patch_export_all_ui(seg: str) -> str:
    old = '''                changed |= ui
                    .checkbox(
                        &mut self.settings.export_all_test_code,
                        "Write Test Code on every exported Face",
                    )
                    .changed();
'''
    if old not in seg:
        raise SystemExit("src/main.rs: stale Export All Test Code checkbox not found")
    return seg.replace(old, "", 1)


patch_function("src/main.rs", "ui_export_all_window", patch_export_all_ui)

# Legacy duplicate Snapshot helpers, if still present, must use the Snapshot recipe.
for fn_name in ("export_snapshot_dialog", "export_snapshot_group_dialog"):
    def patch_snapshot_helper(seg: str) -> str:
        return seg.replace(
            "export_recipe::ExportRecipe::from_project(&project)",
            "export_recipe::ExportRecipe::from_snapshot_project(&project)",
        )

    patch_function("src/main.rs", fn_name, patch_snapshot_helper, required=False)

snapshot_panel = Path("src/ui/snapshots_panel.rs").read_text(encoding="utf-8")
if "ExportRecipe::from_project(&project)" in snapshot_panel:
    raise SystemExit(
        "src/ui/snapshots_panel.rs still contains normal from_project recipe for Snapshot export"
    )
if "ExportRecipe::from_snapshot_project(&project)" not in snapshot_panel:
    raise SystemExit("src/ui/snapshots_panel.rs lacks Snapshot recipe path")

# ---------------------------------------------------------------------------
# #172: immutable queue-time identity for successful Snapshot exports.
# ---------------------------------------------------------------------------
replace(
    "src/export_recipe.rs",
    "use serde::{Deserialize, Serialize};\n\nuse crate::model::{ChannelAdjustment, ShadeProject, TestCodeConfig};",
    "use serde::{Deserialize, Serialize};\nuse sha2::{Digest, Sha256};\n\nuse crate::model::{ChannelAdjustment, ShadeProject, TestCodeConfig};",
)
replace(
    "src/export_recipe.rs",
    "    pub fn materialize_project(&self) -> ShadeProject {\n",
    '''    /// Exact code frozen into this queued export. Empty means this recipe is uncoded.
    pub fn exported_test_code(&self) -> String {
        if self.test_code.enabled {
            self.test_code.text.trim().to_owned()
        } else {
            String::new()
        }
    }

    /// Stable identity of the exact adjustment payload consumed by the exporter.
    pub fn adjustment_sha256(&self) -> String {
        let bytes = serde_json::to_vec(&self.adjustments).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }

    pub fn materialize_project(&self) -> ShadeProject {
''',
)
replace(
    "src/export_recipe.rs",
    '        assert!(!recipe.test_code.enabled);\n        assert_eq!(recipe.test_code.text, "TEST-42");',
    '        assert!(!recipe.test_code.enabled);\n        assert_eq!(recipe.test_code.text, "TEST-42");\n        assert!(recipe.exported_test_code().is_empty());',
)
replace(
    "src/export_recipe.rs",
    "        assert!(recipe.test_code.enabled);\n        assert_eq!(recipe.test_code.text, expected);",
    "        assert!(recipe.test_code.enabled);\n        assert_eq!(recipe.test_code.text, expected);\n        assert_eq!(recipe.exported_test_code(), expected);\n        assert_eq!(recipe.adjustment_sha256().len(), 64);",
)

# Queue completion gets provenance from the immutable ExportRecipe only after success.
replace(
    "src/export_queue.rs",
    '''#[derive(Clone, Debug)]
pub struct ExportQueueCompletion {
    pub id: u64,
    pub project_session_id: u64,
    pub result: Result<String, String>,
    pub mark: Option<ExportQueueMark>,
}
''',
    '''#[derive(Clone, Debug)]
pub struct SnapshotExportProvenance {
    pub test_code: String,
    pub adjustment_sha256: String,
    pub destination: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ExportQueueCompletion {
    pub id: u64,
    pub project_session_id: u64,
    pub result: Result<String, String>,
    pub mark: Option<ExportQueueMark>,
    pub provenance: Option<SnapshotExportProvenance>,
}
''',
)
replace(
    "src/export_queue.rs",
    '''        result: Result<String, String>,
        mark: Option<ExportQueueMark>,
    },
''',
    '''        result: Result<String, String>,
        mark: Option<ExportQueueMark>,
        provenance: Option<SnapshotExportProvenance>,
    },
''',
)
replace(
    "src/export_queue.rs",
    '''                ExportQueueEvent::Finished {
                    id,
                    project_session_id,
                    result,
                    mark,
                } => {
''',
    '''                ExportQueueEvent::Finished {
                    id,
                    project_session_id,
                    result,
                    mark,
                    provenance,
                } => {
''',
)
replace(
    "src/export_queue.rs",
    '''                    completions.push(ExportQueueCompletion {
                        id,
                        project_session_id,
                        result,
                        mark,
                    });
''',
    '''                    completions.push(ExportQueueCompletion {
                        id,
                        project_session_id,
                        result,
                        mark,
                        provenance,
                    });
''',
)

queue_path = Path("src/export_queue.rs")
queue_text = queue_path.read_text(encoding="utf-8")
queue_text = queue_text.replace(
    '''                            result: Ok("Skipped · destination already exists".to_owned()),
                            mark: None,
                        });''',
    '''                            result: Ok("Skipped · destination already exists".to_owned()),
                            mark: None,
                            provenance: None,
                        });''',
    1,
)
queue_text = queue_text.replace(
    '''                result: Err(err),
                mark: None,
            });''',
    '''                result: Err(err),
                mark: None,
                provenance: None,
            });''',
    1,
)
queue_path.write_text(queue_text, encoding="utf-8")

replace(
    "src/export_queue.rs",
    '''        thread::spawn(move || {
            let mark = spec.mark.clone();
            let result = worker_guard::catch_result("Export worker", || {
''',
    '''        thread::spawn(move || {
            let mark = spec.mark.clone();
            let provenance = mark.as_ref().map(|_| SnapshotExportProvenance {
                test_code: spec.recipe.exported_test_code(),
                adjustment_sha256: spec.recipe.adjustment_sha256(),
                destination: spec.destination.clone(),
            });
            let result = worker_guard::catch_result("Export worker", || {
''',
)
replace(
    "src/export_queue.rs",
    '''            let mark = result.as_ref().ok().and(mark);
            let _ = tx.send(ExportQueueEvent::Finished {
                id,
                project_session_id: session_id,
                result,
                mark,
            });
''',
    '''            let mark = result.as_ref().ok().and(mark);
            let provenance = result.as_ref().ok().and(provenance);
            let _ = tx.send(ExportQueueEvent::Finished {
                id,
                project_session_id: session_id,
                result,
                mark,
                provenance,
            });
''',
)

# Assert every Finished constructor now contains provenance.
queue_text = queue_path.read_text(encoding="utf-8")
for chunk in queue_text.split("ExportQueueEvent::Finished {")[1:]:
    body = chunk.split("}", 1)[0]
    if "provenance" not in body:
        raise SystemExit("src/export_queue.rs: Finished event without provenance")

# Durable project history; missing fields deserialize safely from existing projects.
replace(
    "src/model.rs",
    '''pub struct SnapshotExportRecord {
    pub face_key: String,
    pub folder: String,
    pub exported_at_unix_ms: i64,
}
''',
    '''pub struct SnapshotExportRecord {
    pub face_key: String,
    pub folder: String,
    pub exported_at_unix_ms: i64,
    #[serde(default)]
    pub test_code: String,
    #[serde(default)]
    pub adjustment_sha256: String,
    #[serde(default)]
    pub destination: String,
}
''',
)
replace(
    "src/model.rs",
    '''    pub fn record_snapshot_export(
        &mut self,
        id: u64,
        face_key: String,
        folder: String,
        exported_at_unix_ms: i64,
    ) -> bool {
        let Some(snapshot) = self.snapshots.iter_mut().find(|snapshot| snapshot.id == id) else {
            return false;
        };
        snapshot
            .exports
            .retain(|record| record.face_key != face_key);
        snapshot.exports.push(SnapshotExportRecord {
            face_key,
            folder,
            exported_at_unix_ms,
        });
        true
    }
''',
    '''    pub fn record_snapshot_export(
        &mut self,
        id: u64,
        face_key: String,
        folder: String,
        exported_at_unix_ms: i64,
    ) -> bool {
        self.record_snapshot_export_with_identity(
            id,
            face_key,
            folder,
            exported_at_unix_ms,
            String::new(),
            String::new(),
            String::new(),
        )
    }

    /// Append a committed Snapshot export to durable history. Do not replace
    /// older records: previously used codes/states remain auditable.
    pub fn record_snapshot_export_with_identity(
        &mut self,
        id: u64,
        face_key: String,
        folder: String,
        exported_at_unix_ms: i64,
        test_code: String,
        adjustment_sha256: String,
        destination: String,
    ) -> bool {
        let Some(snapshot) = self.snapshots.iter_mut().find(|snapshot| snapshot.id == id) else {
            return false;
        };
        snapshot.exports.push(SnapshotExportRecord {
            face_key,
            folder,
            exported_at_unix_ms,
            test_code,
            adjustment_sha256,
            destination,
        });
        true
    }
''',
)

# The app writes provenance only after queue completion success.
def patch_poll_queue(seg: str) -> str:
    old = '''            if let Some(mark) = completion.mark {
                self.project.record_snapshot_export(
                    mark.snapshot_id,
                    mark.face_key,
                    mark.folder.to_string_lossy().into_owned(),
                    unix_ms_now(),
                );
                self.mark_project_dirty();
            }
'''
    new = '''            if let Some(mark) = completion.mark {
                if let Some(provenance) = completion.provenance {
                    self.project.record_snapshot_export_with_identity(
                        mark.snapshot_id,
                        mark.face_key,
                        mark.folder.to_string_lossy().into_owned(),
                        unix_ms_now(),
                        provenance.test_code,
                        provenance.adjustment_sha256,
                        provenance.destination.to_string_lossy().into_owned(),
                    );
                } else {
                    self.project.record_snapshot_export(
                        mark.snapshot_id,
                        mark.face_key,
                        mark.folder.to_string_lossy().into_owned(),
                        unix_ms_now(),
                    );
                }
                self.mark_project_dirty();
            }
'''
    if old not in seg:
        raise SystemExit("src/main.rs: poll_export_queue mark block not found")
    return seg.replace(old, new, 1)


patch_function("src/main.rs", "poll_export_queue", patch_poll_queue)

# Central Update / Ctrl+Enter guard uses the historical code, not mutable UI text.
workflow_path = Path("src/workflow.rs")
workflow = workflow_path.read_text(encoding="utf-8")
old = '''    let exported = app
        .project
        .snapshots
        .iter()
        .find(|snapshot| snapshot.id == active_id)
        .is_some_and(|snapshot| !snapshot.exports.is_empty());
    let dirty = !app.project.active_snapshot_matches();
    let mut reusing_exported_code = false;

    if exported && dirty {
'''
new = '''    let exported_record = app
        .project
        .snapshots
        .iter()
        .find(|snapshot| snapshot.id == active_id)
        .and_then(|snapshot| snapshot.exports.iter().max_by_key(|record| record.exported_at_unix_ms))
        .cloned();
    let exported = exported_record.is_some();
    let dirty = !app.project.active_snapshot_matches();
    let mut reusing_exported_code = false;

    if exported && dirty {
'''
if old not in workflow:
    raise SystemExit("src/workflow.rs: exported guard block not found")
workflow = workflow.replace(old, new, 1)
old = '''        let current_code = app.project.effective_test_code_text();
        let description = format!(
            "'{snapshot_name}' has already been exported with Test Code '{current_code}'.\\n\\nYes = Create a NEW Snapshot + Test Code (recommended)\\nNo = Reuse the SAME exported Test Code and update this Snapshot\\nCancel = Keep the exported Snapshot unchanged\\n\\nReusing the same code does not bypass the separate file overwrite confirmation during the next export."
        );
'''
new = '''        let current_code = app.project.effective_test_code_text();
        let exported_code = exported_record
            .as_ref()
            .map(|record| record.test_code.trim())
            .filter(|code| !code.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| current_code.clone());
        let description = format!(
            "'{snapshot_name}' has already been exported with Test Code '{exported_code}'.\\n\\nYes = Create a NEW Snapshot + Test Code (recommended)\\nNo = Reuse the SAME exported Test Code and update this Snapshot\\nCancel = Keep the exported Snapshot unchanged\\n\\nReusing the same code does not bypass the separate file overwrite confirmation during the next export."
        );
'''
if old not in workflow:
    raise SystemExit("src/workflow.rs: exported code dialog block not found")
workflow = workflow.replace(old, new, 1)
workflow = workflow.replace(
    "                let new_code = next_test_code(&current_code);",
    "                let new_code = next_test_code(&exported_code);",
    1,
)
old = '''            rfd::MessageDialogResult::No => {
                reusing_exported_code = true;
            }
'''
new = '''            rfd::MessageDialogResult::No => {
                if !exported_code.is_empty() {
                    app.project.test_code.enabled = true;
                    app.project.test_code.text = exported_code;
                }
                reusing_exported_code = true;
            }
'''
if old not in workflow:
    raise SystemExit("src/workflow.rs: reuse code branch not found")
workflow = workflow.replace(old, new, 1)
workflow_path.write_text(workflow, encoding="utf-8")

print("#171/#172 milestone patch applied")
