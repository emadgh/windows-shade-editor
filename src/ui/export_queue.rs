use crate::*;
use eframe::egui;

impl ShadeApp {
    pub(crate) fn ui_export_queue_window(&mut self, ctx: &egui::Context) {
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
        let (_, _, done_count, failed_count, _) = self.export.queue.status_counts();
        let recovered_waiting = self.export.queue.recovered_waiting_count();
        let mut cancel_id = None;
        let mut resume_id = None;
        let mut retry_id = None;
        let mut reveal_folder = None;
        let mut resume_recovered = false;
        let mut cancel_waiting = false;
        let mut pause_toggle = false;
        let mut retry_all_failed = false;
        let mut clear_completed = false;
        let mut clear_failed = false;

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
                        if failed_count > 0 {
                            clear_failed = ui.button(format!("Clear failed ({failed_count})")).clicked();
                            retry_all_failed = ui.button(format!("Retry all failed ({failed_count})")).clicked();
                        }
                        if done_count > 0 {
                            clear_completed = ui.button(format!("Clear completed ({done_count})")).clicked();
                        }
                        cancel_waiting = ui.button("Cancel waiting").clicked();
                        pause_toggle = ui
                            .button(if queue_paused { "Resume queue" } else { "Pause queue" })
                            .clicked();
                        if recovered_waiting > 0 {
                            resume_recovered = ui.button("Resume recovered").clicked();
                        }
                    });
                });
                if recovered_waiting > 0 {
                    ui.small("Recovered exports are never started automatically. Resume individual rows or use Resume recovered when you want them to run.");
                } else {
                    ui.small("Waiting items can be cancelled immediately. Processing items use Stop after current so the current atomic TIFF finishes safely.");
                }
                ui.separator();

                if rows.is_empty() {
                    ui.label("No export jobs yet.");
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (id, label, destination, status, progress, detail, error, restored, requires_resume, metrics) in &rows {
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
                                                    reveal_folder = Some(
                                                        destination
                                                            .parent()
                                                            .unwrap_or_else(|| Path::new("."))
                                                            .to_path_buf(),
                                                    );
                                                }
                                                if *requires_resume {
                                                    if ui.small_button("Resume").clicked() {
                                                        resume_id = Some(*id);
                                                    }
                                                    if ui.small_button("Cancel").clicked() {
                                                        cancel_id = Some(*id);
                                                    }
                                                } else {
                                                    match status {
                                                        export_queue::ExportQueueStatus::Waiting => {
                                                            if ui.small_button("Cancel").clicked() {
                                                                cancel_id = Some(*id);
                                                            }
                                                        }
                                                        export_queue::ExportQueueStatus::Processing => {
                                                            if ui.small_button("Stop after current").clicked() {
                                                                cancel_id = Some(*id);
                                                            }
                                                        }
                                                        export_queue::ExportQueueStatus::Failed
                                                        | export_queue::ExportQueueStatus::Cancelled => {
                                                            if ui.small_button("Retry").clicked() {
                                                                retry_id = Some(*id);
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
        self.export.show_queue = open;

        if resume_recovered {
            let count = self.export.queue.resume_recovered();
            if count > 0 {
                self.report_info(format!("Resumed {count} recovered export(s)"));
            }
        }
        if pause_toggle {
            let paused = !self.export.queue.is_paused();
            self.export.queue.set_paused(paused);
            self.report_info(if paused {
                "Export Queue paused; current atomic export may finish safely"
            } else {
                "Export Queue resumed"
            });
        }
        if retry_all_failed {
            let count = self.export.queue.retry_all_failed();
            if count > 0 {
                self.report_info(format!("Retried {count} failed export(s)"));
            }
        }
        if cancel_waiting {
            self.export.queue.cancel_all_waiting();
        }
        if clear_completed {
            self.export.queue.clear_completed();
        }
        if clear_failed {
            self.export.queue.clear_failed();
        }
        if let Some(id) = resume_id {
            self.export.queue.resume(id);
        }
        if let Some(id) = cancel_id {
            self.export.queue.cancel(id);
        }
        if let Some(id) = retry_id {
            self.export.queue.retry(id);
        }
        if let Some(folder) = reveal_folder {
            if let Err(err) = open_folder(&folder) {
                self.report_error(err);
            }
        }
    }
}
