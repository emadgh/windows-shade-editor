from pathlib import Path

path = Path("work/src/ui/color_conversion.rs")
text = path.read_text(encoding="utf-8")

replacements = [
    (
        """enum ConversionQueueUiAction {\n    ResumeRecovered,\n    TogglePaused,\n    Cancel(u64),\n    Retry(u64),\n    ClearFinished,\n}""",
        """enum ConversionQueueUiAction {\n    ResumeRecovered,\n    TogglePaused,\n    Cancel(u64),\n    Retry(u64),\n    RecoverProject(u64),\n    ClearFinished,\n}""",
    ),
    (
        """                ConversionQueueUiAction::Retry(id) => {\n                    self.conversion_queue.retry(id);\n                }\n                ConversionQueueUiAction::ClearFinished => {""",
        """                ConversionQueueUiAction::Retry(id) => {\n                    self.conversion_queue.retry(id);\n                }\n                ConversionQueueUiAction::RecoverProject(id) => {\n                    match self.conversion_queue.recover_project(id) {\n                        Ok(Some(completion)) => {\n                            let is_current_source = self.project_path.as_ref().is_some_and(|path| {\n                                path.to_string_lossy().eq_ignore_ascii_case(\n                                    &completion.capture.source_project_path.to_string_lossy(),\n                                )\n                            });\n                            match completion.result {\n                                windows_shade_editor::conversion_queue::ConversionQueueCompletionResult::Completed(completed) => {\n                                    if is_current_source {\n                                        match production_project::link_source_project_to_production(\n                                            &mut self.project,\n                                            &completed.production_project_path,\n                                        ) {\n                                            Ok(()) => {\n                                                self.mark_project_dirty();\n                                                self.log.info(\n                                                    \"Recovered Production linkage changed the open Source project; explicit Save is required.\",\n                                                );\n                                            }\n                                            Err(error) => self.log.error(&format!(\n                                                \"Could not mirror recovered Production link in the open Source project: {error}\"\n                                            )),\n                                        }\n                                    }\n                                    self.report_info(format!(\n                                        \"Recovered Production project: {}\",\n                                        completed.production_project_path.display()\n                                    ));\n                                }\n                                _ => self.report_error(\n                                    \"Project-only recovery returned an unexpected conversion result.\",\n                                ),\n                            }\n                        }\n                        Ok(None) => self.report_info(\n                            \"This conversion item has no project-only recovery work.\",\n                        ),\n                        Err(error) => self.report_error(format!(\n                            \"Production project recovery blocked: {error}\"\n                        )),\n                    }\n                }\n                ConversionQueueUiAction::ClearFinished => {""",
    ),
    (
        """                ConversionQueueStatus::Failed\n                | ConversionQueueStatus::Cancelled\n                | ConversionQueueStatus::NeedsRecovery => {\n                    if ui.button(\"Retry safely\").clicked() {\n                        actions.push(ConversionQueueUiAction::Retry(row.id));\n                    }\n                }\n                ConversionQueueStatus::Done => {}""",
        """                ConversionQueueStatus::Failed | ConversionQueueStatus::Cancelled => {\n                    if ui.button(\"Retry safely\").clicked() {\n                        actions.push(ConversionQueueUiAction::Retry(row.id));\n                    }\n                }\n                ConversionQueueStatus::NeedsRecovery => {\n                    if ui\n                        .button(\"Recover Project\")\n                        .on_hover_text(\n                            \"Complete only the Production .shade save. The committed TIFF is verified and never rendered again.\",\n                        )\n                        .clicked()\n                    {\n                        actions.push(ConversionQueueUiAction::RecoverProject(row.id));\n                    }\n                }\n                ConversionQueueStatus::Done => {}""",
    ),
]

for old, new in replacements:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"Expected exactly one UI patch target, found {count}: {old[:80]!r}")
    text = text.replace(old, new)

path.write_text(text, encoding="utf-8", newline="\n")
