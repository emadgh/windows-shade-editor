use crate::ui::actions::ExportQueueUiAction;
use crate::*;
use eframe::egui;

impl ShadeApp {
    pub(crate) fn ui_export_queue_window(&mut self, ctx: &egui::Context) {
        // Called after CentralPanel each frame, so this is the stable post-layout
        // hook for viewport-owned controls even when the Export Queue itself is closed.
        super::viewport_controls::show(self, ctx);

        if !self.export.show_queue {
            return;
        }
        let mut open = self.export.show_queue;
        let rows = self
            .export
            .queue
            .items()
            .iter()
            .map(|item| {
                (
                    item.id,
                    item.label.clone(),
                    item.destination.clone(),
                    item.status,
                    item.progress,
                    item.detail.clone(),
                    item.error.clone(),
                    item.restored,
                    item.requires_resume,
                    self.export.queue.metrics_text(item.id),
                )
            })
            .collect::<Vec<_>>();
        let pending = self.export.queue.pending_count();
        let queue_paused = self.export.queue.is_paused();
        let (_, _, done_count, failed_count, cancelled_count) = self.export.queue.status_counts();
        let finished_count = done_count + failed_count + cancelled_count;
        let cancellable_count = rows
            .iter()
            .filter(|(_, _, _, status, _, _, _, _, _, _)| {
                matches!(
                    status,
                    export_queue::ExportQueueStatus::Waiting
                        | export_queue::ExportQueueStatus::Processing
                )
            })
            .count();
        let recovered_waiting = self.export.queue.recovered_waiting_count();
        let mut actions = Vec::new();

        egui::Window::new("Export Queue")
            .open(&mut open)
            .resizable(true)
            .default_size([820.0, 540.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Export Queue");
                    ui.label(format!("{pending} pending"));
                    if recovered_waiting > 0 {
                        ui.colored_label(
                            egui::Color32::from_rgb(225, 175, 70),
                            format!("{recovered_waiting} recovered · paused"),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if failed_count > 0
                            && ui
                                .button(format!("Retry all failed ({failed_count})"))
                                .clicked()
                        {
                            actions.push(ExportQueueUiAction::RetryAllFailed);
                        }
                        if finished_count > 0
                            && ui
                                .button(format!("Clear Jobs ({finished_count})"))
                                .on_hover_text(
                                    "Remove completed, failed and cancelled jobs from the queue history.",
                                )
                                .clicked()
                        {
                            actions.push(ExportQueueUiAction::ClearJobs);
                        }
                        if cancellable_count > 0
                            && ui
                                .button(format!("Cancel All ({cancellable_count})"))
                                .on_hover_text(
                                    "Cancel every waiting job. If one export is processing, request a safe stop after its current atomic TIFF commit.",
                                )
                                .clicked()
                        {
                            for (id, _, _, status, _, _, _, _, _, _) in &rows {
                                if matches!(
                                    status,
                                    export_queue::ExportQueueStatus::Waiting
                                        | export_queue::ExportQueueStatus::Processing
                                ) {
                                    actions.push(ExportQueueUiAction::Cancel(*id));
                                }
                            }
                        }
                        if ui
                            .button(if queue_paused {
                                "Resume queue"
                            } else {
                                "Pause queue"
                            })
                            .clicked()
                        {
                            actions.push(ExportQueueUiAction::TogglePaused);
                        }
                        if recovered_waiting > 0 && ui.button("Resume recovered").clicked() {
                            actions.push(ExportQueueUiAction::ResumeRecovered);
                        }
                    });
                });
                if recovered_waiting > 0 {
                    ui.small("Recovered exports are never started automatically. Resume individual rows or use Resume recovered when you want them to run.");
                } else {
                    ui.small("Cancel is immediate for waiting jobs. Cancelling the active export preserves its atomic TIFF boundary, then stops the remaining queue.");
                }
                ui.separator();

                if rows.is_empty() {
                    ui.label("No export jobs yet.");
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (
                            id,
                            label,
                            destination,
                            status,
                            progress,
                            detail,
                            error,
                            restored,
                            requires_resume,
                            metrics,
                        ) in &rows
                        {
                            let (fill, status_color) = match status {
                                export_queue::ExportQueueStatus::Waiting => (
                                    egui::Color32::from_rgba_unmultiplied(135, 95, 20, 34),
                                    egui::Color32::from_rgb(230, 180, 70),
                                ),
                                export_queue::ExportQueueStatus::Processing => (
                                    egui::Color32::from_rgba_unmultiplied(25, 90, 165, 38),
                                    egui::Color32::from_rgb(90, 165, 255),
                                ),
                                export_queue::ExportQueueStatus::Done => (
                                    egui::Color32::from_rgba_unmultiplied(30, 115, 60, 32),
                                    egui::Color32::from_rgb(90, 205, 125),
                                ),
                                export_queue::ExportQueueStatus::Failed => (
                                    egui::Color32::from_rgba_unmultiplied(155, 35, 35, 40),
                                    egui::Color32::from_rgb(245, 105, 105),
                                ),
                                export_queue::ExportQueueStatus::Cancelled => (
                                    egui::Color32::from_rgba_unmultiplied(80, 80, 80, 26),
                                    egui::Color32::from_rgb(165, 165, 165),
                                ),
                            };
                            let status_text = if *requires_resume {
                                "Waiting · paused"
                            } else {
                                status.label()
                            };
                            egui::Frame::new()
                                .inner_margin(8)
                                .fill(fill)
                                .stroke(egui::Stroke::new(1.0, status_color))
                                .corner_radius(5)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.strong(label);
                                        if *restored && !*requires_resume {
                                            ui.small("restored");
                                        }
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    egui::RichText::new(status_text)
                                                        .color(status_color)
                                                        .strong(),
                                                );
                                                if ui.small_button("Reveal folder").clicked() {
                                                    actions.push(ExportQueueUiAction::RevealFolder(
                                                        destination
                                                            .parent()
                                                            .unwrap_or_else(|| Path::new("."))
                                                            .to_path_buf(),
                                                    ));
                                                }
                                                if *requires_resume {
                                                    if ui.small_button("Resume").clicked() {
                                                        actions.push(ExportQueueUiAction::Resume(*id));
                                                    }
                                                    if ui.small_button("Cancel").clicked() {
                                                        actions.push(ExportQueueUiAction::Cancel(*id));
                                                    }
                                                } else {
                                                    match status {
                                                        export_queue::ExportQueueStatus::Waiting => {
                                                            if ui.small_button("Cancel").clicked() {
                                                                actions.push(
                                                                    ExportQueueUiAction::Cancel(*id),
                                                                );
                                                            }
                                                        }
                                                        export_queue::ExportQueueStatus::Processing => {
                                                            if ui
                                                                .small_button("Cancel")
                                                                .on_hover_text(
                                                                    "Request cancellation without breaking the current atomic TIFF commit. The queue stops after the active export reaches its safe boundary.",
                                                                )
                                                                .clicked()
                                                            {
                                                                actions.push(
                                                                    ExportQueueUiAction::Cancel(*id),
                                                                );
                                                            }
                                                        }
                                                        export_queue::ExportQueueStatus::Failed
                                                        | export_queue::ExportQueueStatus::Cancelled => {
                                                            if ui.small_button("Retry").clicked() {
                                                                actions.push(
                                                                    ExportQueueUiAction::Retry(*id),
                                                                );
                                                            }
                                                        }
                                                        export_queue::ExportQueueStatus::Done => {}
                                                    }
                                                }
                                            },
                                        );
                                    });

                                    if *status == export_queue::ExportQueueStatus::Processing {
                                        let progress_text = if let Some(metrics) = metrics {
                                            if detail.trim().is_empty() {
                                                format!("Processing · {metrics}")
                                            } else {
                                                format!("{detail} · {metrics}")
                                            }
                                        } else if detail.trim().is_empty() {
                                            "Processing".to_owned()
                                        } else {
                                            detail.clone()
                                        };
                                        export_queue_progress_bar(ui, *progress, &progress_text);
                                    } else {
                                        let detail = detail.trim();
                                        if !detail.is_empty()
                                            && detail != status.label()
                                            && detail != "Done"
                                        {
                                            let detail = detail.strip_prefix("Done · ").unwrap_or(detail);
                                            if !detail.is_empty() {
                                                ui.small(detail);
                                            }
                                        }
                                    }
                                    ui.small(destination.display().to_string());
                                    if let Some(error) = error {
                                        ui.colored_label(egui::Color32::LIGHT_RED, error);
                                    }
                                });
                            ui.add_space(5.0);
                        }
                    });
                }
            });

        if open != self.export.show_queue {
            actions.push(ExportQueueUiAction::SetOpen(open));
        }
        for action in actions {
            self.dispatch_export_queue_ui_action(action);
        }
    }
}
