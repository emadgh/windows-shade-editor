from pathlib import Path
import re


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


root = Path(__file__).resolve().parents[2]
queue_path = root / "src" / "export_queue.rs"
main_path = root / "src" / "main.rs"
recent_path = root / "src" / "previous_shades.rs"
queue = queue_path.read_text(encoding="utf-8")
main = main_path.read_text(encoding="utf-8")
recent = recent_path.read_text(encoding="utf-8")

queue = replace_once(queue, "use std::time::UNIX_EPOCH;", "use std::time::{Duration, Instant, UNIX_EPOCH};", "queue time imports")

old_item = '''    pub requires_resume: bool,\n    spec: QueuedExportSpec,\n}\n'''
new_item = '''    pub requires_resume: bool,\n    started_at: Option<Instant>,\n    spec: QueuedExportSpec,\n}\n'''
queue = replace_once(queue, old_item, new_item, "queue item timing")

# Every runtime item starts without a processing timestamp; start_next owns it.
queue = queue.replace("                requires_resume,\n                spec,", "                requires_resume,\n                started_at: None,\n                spec,")
queue = queue.replace("            requires_resume: false,\n            spec: QueuedExportSpec {", "            requires_resume: false,\n            started_at: None,\n            spec: QueuedExportSpec {")
if queue.count("started_at: None") < 3:
    raise SystemExit("expected timing initialization for restored/enqueue paths")

old_persisted = '''struct PersistedQueue {\n    format_version: u32,\n    next_id: u64,\n    items: Vec<PersistedQueueItem>,\n}\n'''
new_persisted = '''struct PersistedQueue {\n    format_version: u32,\n    next_id: u64,\n    #[serde(default)]\n    paused: bool,\n    items: Vec<PersistedQueueItem>,\n}\n'''
queue = replace_once(queue, old_persisted, new_persisted, "persisted pause flag")

old_queue_fields = '''    active_id: Option<u64>,\n    stop_after_current: bool,\n    tx: mpsc::Sender<ExportQueueEvent>,\n'''
new_queue_fields = '''    active_id: Option<u64>,\n    stop_after_current: bool,\n    paused: bool,\n    tx: mpsc::Sender<ExportQueueEvent>,\n'''
queue = replace_once(queue, old_queue_fields, new_queue_fields, "queue paused state")
queue = replace_once(queue, "            stop_after_current: false,\n            tx,", "            stop_after_current: false,\n            paused: false,\n            tx,", "queue paused init")
queue = replace_once(queue, "        queue.next_id = persisted.next_id.max(1);", "        queue.next_id = persisted.next_id.max(1);\n        queue.paused = persisted.paused;", "queue paused load")
queue = replace_once(queue, "            next_id: self.next_id,\n            items,", "            next_id: self.next_id,\n            paused: self.paused,\n            items,", "queue paused persist")

# Sanitized progress protects egui from NaN/Inf geometry regressions.
queue = replace_once(queue, "                        item.progress = fraction.clamp(0.0, 1.0);", "                        item.progress = if fraction.is_finite() {\n                            fraction.clamp(0.0, 1.0)\n                        } else {\n                            0.0\n                        };", "finite queue progress")

# Processing timestamp lifecycle.
queue = replace_once(queue, "                        item.progress = 1.0;", "                        item.progress = 1.0;\n                        item.started_at = item.started_at.take();", "finish keeps elapsed snapshot")
# The previous line would discard the timestamp; replace with a dedicated elapsed field-free strategy: keep timestamp.
queue = queue.replace("                        item.started_at = item.started_at.take();\n", "")
queue = replace_once(queue, "        self.items[index].progress = 0.0;\n        self.items[index].detail = \"Starting\".to_owned();", "        self.items[index].progress = 0.0;\n        self.items[index].started_at = Some(Instant::now());\n        self.items[index].detail = \"Starting\".to_owned();", "queue start timestamp")
queue = replace_once(queue, "        self.items[index].status = ExportQueueStatus::Processing;\n        self.active_id = Some(id);\n        let tx = self.tx.clone();", "        self.items[index].status = ExportQueueStatus::Processing;\n        self.items[index].started_at = Some(Instant::now());\n        self.active_id = Some(id);\n        let tx = self.tx.clone();", "preflight timestamp")

# Pause is global for future Waiting work; current atomic export is never interrupted.
queue = replace_once(queue, "    pub fn has_pending(&self) -> bool {\n        self.pending_count() > 0\n    }\n", '''    pub fn has_pending(&self) -> bool {\n        self.pending_count() > 0\n    }\n\n    pub fn is_paused(&self) -> bool {\n        self.paused\n    }\n\n    pub fn set_paused(&mut self, paused: bool) -> bool {\n        if self.paused == paused {\n            return false;\n        }\n        self.paused = paused;\n        self.persist();\n        true\n    }\n\n    pub fn status_counts(&self) -> (usize, usize, usize, usize, usize) {\n        let mut waiting = 0;\n        let mut processing = 0;\n        let mut done = 0;\n        let mut failed = 0;\n        let mut cancelled = 0;\n        for item in &self.items {\n            match item.status {\n                ExportQueueStatus::Waiting => waiting += 1,\n                ExportQueueStatus::Processing => processing += 1,\n                ExportQueueStatus::Done => done += 1,\n                ExportQueueStatus::Failed => failed += 1,\n                ExportQueueStatus::Cancelled => cancelled += 1,\n            }\n        }\n        (waiting, processing, done, failed, cancelled)\n    }\n\n    pub fn retry_all_failed(&mut self) -> usize {\n        let mut reserved = self.reserved_destination_keys();\n        let mut retried = 0;\n        for item in &mut self.items {\n            if item.status != ExportQueueStatus::Failed {\n                continue;\n            }\n            let key = path_safety::path_key(&item.destination);\n            if reserved.contains(&key) {\n                item.error = Some(\"Destination is reserved by another queued export.\".to_owned());\n                continue;\n            }\n            reserved.insert(key);\n            item.status = ExportQueueStatus::Waiting;\n            item.requires_resume = false;\n            item.progress = 0.0;\n            item.started_at = None;\n            item.detail.clear();\n            item.error = None;\n            retried += 1;\n        }\n        if retried > 0 {\n            self.persist();\n        }\n        retried\n    }\n\n    pub fn clear_completed(&mut self) -> usize {\n        let before = self.items.len();\n        self.items.retain(|item| item.status != ExportQueueStatus::Done);\n        let removed = before - self.items.len();\n        if removed > 0 {\n            self.persist();\n        }\n        removed\n    }\n\n    pub fn clear_failed(&mut self) -> usize {\n        let before = self.items.len();\n        self.items.retain(|item| item.status != ExportQueueStatus::Failed);\n        let removed = before - self.items.len();\n        if removed > 0 {\n            self.persist();\n        }\n        removed\n    }\n''', "queue QoL methods")

metrics_anchor = '''    pub fn active_summary(&self) -> Option<(f32, String)> {\n        let id = self.active_id?;\n        let item = self.items.iter().find(|item| item.id == id)?;\n        let text = if item.detail.trim().is_empty() {\n            item.label.clone()\n        } else {\n            format!(\"{} · {}\", item.label, item.detail)\n        };\n        Some((item.progress, text))\n    }\n'''
metrics_new = '''    pub fn active_summary(&self) -> Option<(f32, String)> {\n        let id = self.active_id?;\n        let item = self.items.iter().find(|item| item.id == id)?;\n        let mut text = if item.detail.trim().is_empty() {\n            item.label.clone()\n        } else {\n            format!(\"{} · {}\", item.label, item.detail)\n        };\n        if let Some(metrics) = self.metrics_text(id) {\n            text.push_str(\" · \');\n            text.push_str(&metrics);\n        }\n        Some((finite_progress(item.progress), text))\n    }\n\n    pub fn metrics_text(&self, id: u64) -> Option<String> {\n        let item = self.items.iter().find(|item| item.id == id)?;\n        if item.status != ExportQueueStatus::Processing {\n            return None;\n        }\n        let progress = finite_progress(item.progress);\n        let elapsed = item.started_at.map(|at| at.elapsed()).unwrap_or_default();\n        let mut parts = vec![format_duration(elapsed)];\n        if progress > 0.01 && elapsed.as_secs_f64() > 0.25 {\n            let total_seconds = elapsed.as_secs_f64() / progress as f64;\n            let eta = Duration::from_secs_f64((total_seconds - elapsed.as_secs_f64()).max(0.0));\n            parts.push(format!(\"~{} left\", format_duration(eta)));\n            let source_bytes = item\n                .spec\n                .source_fingerprint\n                .as_ref()\n                .map(|fingerprint| fingerprint.size_bytes)\n                .or_else(|| std::fs::metadata(&item.source).ok().map(|metadata| metadata.len()));\n            if let Some(source_bytes) = source_bytes {\n                let equivalent = source_bytes as f64 * progress as f64 / elapsed.as_secs_f64();\n                if equivalent.is_finite() && equivalent > 0.0 {\n                    parts.push(format!(\"~{:.1} MB/s\", equivalent / 1_048_576.0));\n                }\n            }\n        }\n        Some(parts.join(\" · \"))\n    }\n\n    pub fn compact_status(&self) -> Option<String> {\n        if let Some(id) = self.active_id {\n            let item = self.items.iter().find(|item| item.id == id)?;\n            let index = self.items.iter().position(|row| row.id == id).unwrap_or(0) + 1;\n            let total = self.items.len().max(1);\n            let percent = (finite_progress(item.progress) * 100.0).round() as u32;\n            let mut text = format!(\"Exporting {index}/{total} · {percent}%\");\n            if let Some(metrics) = self.metrics_text(id) {\n                text.push_str(\" · \');\n                text.push_str(&metrics);\n            }\n            return Some(text);\n        }\n        let waiting = self\n            .items\n            .iter()\n            .filter(|item| item.status == ExportQueueStatus::Waiting && !item.requires_resume)\n            .count();\n        if self.paused && waiting > 0 {\n            Some(format!(\"Queue paused · {waiting} waiting\"))\n        } else if waiting > 0 {\n            Some(format!(\"Queue · {waiting} waiting\"))\n        } else {\n            None\n        }\n    }\n'''.replace("text.push_str(\\\" · \\\');", "text.push_str(\" · \" );")
queue = replace_once(queue, metrics_anchor, metrics_new, "queue active metrics")

# Reset timestamps when retrying/cancelling pre-start rows.
queue = replace_once(queue, "                item.detail = \"Cancelled before processing\".to_owned();\n                true", "                item.detail = \"Cancelled before processing\".to_owned();\n                item.started_at = None;\n                true", "cancel timestamp")
queue = replace_once(queue, "        item.progress = 0.0;\n        item.detail.clear();", "        item.progress = 0.0;\n        item.started_at = None;\n        item.detail.clear();", "retry timestamp")
queue = queue.replace("                item.detail = \"Cancelled before processing\".to_owned();\n                changed = true;", "                item.detail = \"Cancelled before processing\".to_owned();\n                item.started_at = None;\n                changed = true;")

# Global pause prevents the next waiting row from starting; a processing row finishes atomically.
queue = replace_once(queue, "        if self.active_id.is_none() && !self.stop_after_current {", "        if self.active_id.is_none() && !self.stop_after_current && !self.paused {", "pause start_next guard")

# Helpers stay pure/testable.
helper_anchor = '''fn queue_persistence_path() -> PathBuf {\n'''
helpers = '''fn finite_progress(value: f32) -> f32 {\n    if value.is_finite() {\n        value.clamp(0.0, 1.0)\n    } else {\n        0.0\n    }\n}\n\nfn format_duration(duration: Duration) -> String {\n    let total = duration.as_secs();\n    let minutes = total / 60;\n    let seconds = total % 60;\n    format!(\"{minutes:02}:{seconds:02}\")\n}\n\nfn queue_persistence_path() -> PathBuf {\n'''
queue = replace_once(queue, helper_anchor, helpers, "queue metrics helpers")

# Regression tests for queue pause, batch retry and non-finite progress.
test_insert = r'''

    #[test]
    fn paused_queue_does_not_start_waiting_work_until_resumed() {
        let mut queue = ExportQueue::new();
        queue.enqueue(spec("paused.tif"));
        assert!(queue.set_paused(true));
        assert!(queue.poll().is_empty());
        assert!(queue.active_id.is_none());
        assert_eq!(queue.pending_count(), 1);
        assert!(queue.set_paused(false));
        let _ = queue.poll();
        assert!(queue.active_id.is_some());
    }

    #[test]
    fn retry_all_failed_only_requeues_failed_rows() {
        let mut queue = ExportQueue::new();
        let failed = queue.enqueue(spec("failed.tif"));
        let cancelled = queue.enqueue(spec("cancelled.tif"));
        queue.items.iter_mut().find(|item| item.id == failed).unwrap().status = ExportQueueStatus::Failed;
        queue.items.iter_mut().find(|item| item.id == cancelled).unwrap().status = ExportQueueStatus::Cancelled;
        assert_eq!(queue.retry_all_failed(), 1);
        assert_eq!(queue.items.iter().find(|item| item.id == failed).unwrap().status, ExportQueueStatus::Waiting);
        assert_eq!(queue.items.iter().find(|item| item.id == cancelled).unwrap().status, ExportQueueStatus::Cancelled);
    }

    #[test]
    fn non_finite_progress_is_sanitized() {
        assert_eq!(finite_progress(f32::NAN), 0.0);
        assert_eq!(finite_progress(f32::INFINITY), 0.0);
        assert_eq!(finite_progress(-1.0), 0.0);
        assert_eq!(finite_progress(2.0), 1.0);
    }
'''
last = queue.rfind("\n}")
if last < 0:
    raise SystemExit("queue tests module closing brace not found")
queue = queue[:last] + test_insert + queue[last:]
queue_path.write_text(queue, encoding="utf-8")

# Add a reusable recent-project query to Project View's store rather than duplicating sorting in UI.
recent_anchor = '''    pub fn entries(&self) -> &[PreviousShadeEntry] {\n        &self.entries\n    }\n'''
recent_new = recent_anchor + '''\n    pub fn recent(&self, limit: usize) -> Vec<&PreviousShadeEntry> {\n        let mut rows = self.entries.iter().collect::<Vec<_>>();\n        rows.sort_by(|left, right| {\n            right\n                .last_opened_unix_ms\n                .cmp(&left.last_opened_unix_ms)\n                .then_with(|| right.saved_at_unix_ms.cmp(&left.saved_at_unix_ms))\n        });\n        rows.truncate(limit);\n        rows\n    }\n'''
recent = replace_once(recent, recent_anchor, recent_new, "recent projects helper")
recent_path.write_text(recent, encoding="utf-8")

# Queue UI: richer aggregate actions and per-processing metrics.
old_rows_tuple = '''                    item.error.clone(),\n                    item.restored,\n                    item.requires_resume,\n                )\n'''
new_rows_tuple = '''                    item.error.clone(),\n                    item.restored,\n                    item.requires_resume,\n                    self.export.queue.metrics_text(item.id),\n                )\n'''
main = replace_once(main, old_rows_tuple, new_rows_tuple, "queue row metrics")
main = replace_once(main, "        let pending = self.export.queue.pending_count();\n        let recovered_waiting", "        let pending = self.export.queue.pending_count();\n        let queue_paused = self.export.queue.is_paused();\n        let (_, _, done_count, failed_count, _) = self.export.queue.status_counts();\n        let recovered_waiting", "queue header state")
main = replace_once(main, "        let mut clear_finished = false;", "        let mut pause_toggle = false;\n        let mut retry_all_failed = false;\n        let mut clear_completed = false;\n        let mut clear_failed = false;", "queue action state")

old_header_buttons = '''                        clear_finished = ui.button("Clear finished").clicked();\n                        cancel_waiting = ui.button("Cancel waiting").clicked();\n                        if recovered_waiting > 0 {\n                            resume_recovered = ui.button("Resume recovered").clicked();\n                        }\n'''
new_header_buttons = '''                        if failed_count > 0 {\n                            clear_failed = ui.button(format!("Clear failed ({failed_count})")).clicked();\n                            retry_all_failed = ui.button(format!("Retry all failed ({failed_count})")).clicked();\n                        }\n                        if done_count > 0 {\n                            clear_completed = ui.button(format!("Clear completed ({done_count})")).clicked();\n                        }\n                        cancel_waiting = ui.button("Cancel waiting").clicked();\n                        pause_toggle = ui\n                            .button(if queue_paused { "Resume queue" } else { "Pause queue" })\n                            .clicked();\n                        if recovered_waiting > 0 {\n                            resume_recovered = ui.button("Resume recovered").clicked();\n                        }\n'''
main = replace_once(main, old_header_buttons, new_header_buttons, "queue header actions")
main = main.replace("for (id, label, destination, status, progress, detail, error, restored, requires_resume) in &rows {", "for (id, label, destination, status, progress, detail, error, restored, requires_resume, metrics) in &rows {")

old_progress = '''                                        export_queue_progress_bar(\n                                            ui,\n                                            *progress,\n                                            if detail.trim().is_empty() {\n                                                "Processing"\n                                            } else {\n                                                detail\n                                            },\n                                        );\n'''
new_progress = '''                                        let progress_text = if let Some(metrics) = metrics {\n                                            if detail.trim().is_empty() {\n                                                format!("Processing · {metrics}")\n                                            } else {\n                                                format!("{detail} · {metrics}")\n                                            }\n                                        } else if detail.trim().is_empty() {\n                                            "Processing".to_owned()\n                                        } else {\n                                            detail.clone()\n                                        };\n                                        export_queue_progress_bar(ui, *progress, &progress_text);\n'''
main = replace_once(main, old_progress, new_progress, "queue row progress metrics")

old_after_actions = '''        if cancel_waiting {\n            self.export.queue.cancel_all_waiting();\n        }\n        if clear_finished {\n            self.export.queue.clear_finished();\n        }\n'''
new_after_actions = '''        if pause_toggle {\n            let paused = !self.export.queue.is_paused();\n            self.export.queue.set_paused(paused);\n            self.report_info(if paused {\n                "Export Queue paused; current atomic export may finish safely"\n            } else {\n                "Export Queue resumed"\n            });\n        }\n        if retry_all_failed {\n            let count = self.export.queue.retry_all_failed();\n            if count > 0 {\n                self.report_info(format!("Retried {count} failed export(s)"));\n            }\n        }\n        if cancel_waiting {\n            self.export.queue.cancel_all_waiting();\n        }\n        if clear_completed {\n            self.export.queue.clear_completed();\n        }\n        if clear_failed {\n            self.export.queue.clear_failed();\n        }\n'''
main = replace_once(main, old_after_actions, new_after_actions, "queue aggregate actions")

# Compact toolbar status replaces the count-only label.
old_queue_label = '''                let queue_pending = self.export.queue.pending_count();\n                let queue_recovered = self.export.queue.recovered_waiting_count();\n                let queue_label = if queue_recovered > 0 {\n                    format!("Queue ({queue_pending} + {queue_recovered} recovered)")\n                } else {\n                    format!("Queue ({queue_pending})")\n                };\n                if ui.button(queue_label).clicked() { self.export.show_queue = true; }\n'''
new_queue_label = '''                let queue_pending = self.export.queue.pending_count();\n                let queue_recovered = self.export.queue.recovered_waiting_count();\n                let queue_label = self.export.queue.compact_status().unwrap_or_else(|| {\n                    if queue_recovered > 0 {\n                        format!("Queue ({queue_pending} + {queue_recovered} recovered)")\n                    } else {\n                        format!("Queue ({queue_pending})")\n                    }\n                });\n                if ui.button(queue_label).clicked() { self.export.show_queue = true; }\n'''
main = replace_once(main, old_queue_label, new_queue_label, "compact queue toolbar")

# File > Recent Projects. Snapshot the rows before entering egui closures to avoid borrowing self twice.
main = replace_once(main, "        let mut queue_requested = false;\n", '''        let mut queue_requested = false;\n        let recent_projects = self\n            .previous_shades\n            .recent(8)\n            .into_iter()\n            .map(|entry| (entry.display_name(), entry.path.clone(), entry.is_missing()))\n            .collect::<Vec<_>>();\n        let mut recent_requested: Option<PathBuf> = None;\n''', "recent toolbar state")
open_anchor = '''                    if ui.add_enabled(enabled, egui::Button::new("Open .shade...")).clicked() {\n                        self.open_project_dialog();\n                    }\n                    if ui.add_enabled(enabled, egui::Button::new("Add TIFF faces...")).clicked() {\n'''
open_new = '''                    if ui.add_enabled(enabled, egui::Button::new("Open .shade...")).clicked() {\n                        self.open_project_dialog();\n                    }\n                    ui.menu_button("Recent projects", |ui| {\n                        if recent_projects.is_empty() {\n                            ui.label("No recent projects");\n                        } else {\n                            for (name, path, missing) in &recent_projects {\n                                let label = if *missing {\n                                    format!("{name}  [missing]")\n                                } else {\n                                    name.clone()\n                                };\n                                if ui\n                                    .add_enabled(enabled && !*missing, egui::Button::new(label))\n                                    .on_hover_text(path)\n                                    .clicked()\n                                {\n                                    recent_requested = Some(PathBuf::from(path));\n                                    ui.close();\n                                }\n                            }\n                        }\n                    });\n                    if ui.add_enabled(enabled, egui::Button::new("Add TIFF faces...")).clicked() {\n'''
main = replace_once(main, open_anchor, open_new, "File recent menu")

post_toolbar = '''        if queue_requested {\n            self.export.show_queue = true;\n        }\n'''
post_toolbar_new = '''        if queue_requested {\n            self.export.show_queue = true;\n        }\n        if let Some(path) = recent_requested {\n            self.request_project_transition(ProjectTransition::Open(path), Some(ui.ctx()));\n        }\n'''
main = replace_once(main, post_toolbar, post_toolbar_new, "recent lifecycle open")

main_path.write_text(main, encoding="utf-8")

Path(__file__).unlink()
bootstrap = root / ".github" / "workflows" / "apply-v019-queue-recent.yml"
if bootstrap.exists():
    bootstrap.unlink()
