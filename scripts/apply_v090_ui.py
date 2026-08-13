from pathlib import Path
import re

APP = Path('src/app_main.rs')
MODEL = Path('src/model_v6.rs')
CARGO = Path('Cargo.toml')
NOTES = Path('RELEASE_NOTES.md')

app = APP.read_text(encoding='utf-8')
model = MODEL.read_text(encoding='utf-8')
cargo = CARGO.read_text(encoding='utf-8')
notes = NOTES.read_text(encoding='utf-8')


def once(text, old, new, label):
    if old not in text:
        raise SystemExit(f'anchor not found: {label}')
    return text.replace(old, new, 1)

# --- package / clean schema v9 ---
cargo = once(cargo, 'version = "0.8.1"', 'version = "0.9.0"', 'Cargo version')
model = once(model, 'pub const SHADE_SCHEMA_VERSION: u32 = 8;', 'pub const SHADE_SCHEMA_VERSION: u32 = 9;', 'schema version')

load_start = model.index('    pub fn load(path: &Path) -> Result<Self, String> {')
load_end = model.index('    pub fn save(&self, path: &Path, resolved_face_paths: &[PathBuf])', load_start)
new_load = '''    pub fn load(path: &Path) -> Result<Self, String> {
        let text =
            fs::read_to_string(path).map_err(|err| format!("Cannot read .shade file: {err}"))?;
        let project: Self =
            serde_json::from_str(&text).map_err(|err| format!("Invalid .shade file: {err}"))?;
        if project.schema_version != SHADE_SCHEMA_VERSION {
            return Err(format!(
                "Unsupported .shade schema {}. Shade Editor 0.9 accepts schema {} only; pre-production migration code has been removed.",
                project.schema_version, SHADE_SCHEMA_VERSION
            ));
        }
        Ok(project)
    }

'''
model = model[:load_start] + new_load + model[load_end:]

mig_start = model.index('fn enable_legacy_curve_midpoints(')
mig_end = model.index('fn ensure_adjustment_channels(', mig_start)
model = model[:mig_start] + model[mig_end:]
model = model.replace('    /// Snapshot creation time as Unix milliseconds. Older schema versions do\n    /// not have this field and deserialize it as zero.\n', '    /// Snapshot creation time as Unix milliseconds.\n')

# exact-schema regression test
insert_test = '''
    #[test]
    fn loader_rejects_old_schema_without_migration() {
        let mut project = ShadeProject::default();
        project.schema_version = 8;
        let path = std::env::temp_dir().join(format!(
            "shade-editor-old-schema-{}-{}.shade",
            std::process::id(),
            now_unix_ms()
        ));
        fs::write(&path, serde_json::to_string(&project).unwrap()).unwrap();
        let error = ShadeProject::load(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        assert!(error.contains("accepts schema 9 only"));
    }
'''
model = once(model, '\n}\n', insert_test + '\n}\n', 'model tests ending') if False else model
# append test before final test-module brace, identified from the last occurrence
last = model.rfind('\n}')
model = model[:last] + insert_test + model[last:]

# --- app modules / constants ---
app = once(app, 'mod dpi;\n', 'mod dpi;\nmod history;\n', 'history module')
app = once(app, 'mod palette;\n', 'mod palette;\nmod recovery;\n', 'recovery module')
app = once(
    app,
    'const ERROR_TOAST_LIFETIME: Duration = Duration::from_secs(120);',
    'const ERROR_TOAST_LIFETIME: Duration = Duration::from_secs(120);\nconst AUTOSAVE_INTERVAL: Duration = Duration::from_secs(120);\nconst HISTORY_COMMIT_DELAY: Duration = Duration::from_millis(300);',
    'app constants',
)

# Runtime / payload state
app = once(
    app,
    '    texture: Option<egui::TextureHandle>,\n    generation: u64,',
    '    texture: Option<egui::TextureHandle>,\n    original_texture: Option<egui::TextureHandle>,\n    generation: u64,',
    'original texture field',
)
app = once(
    app,
    'struct OpenPayload {\n    path: PathBuf,\n    project: ShadeProject,\n    faces: Vec<LoadedFace>,\n    errors: Vec<String>,\n}\n',
    '''struct OpenPayload {
    path: PathBuf,
    project: ShadeProject,
    faces: Vec<LoadedFace>,
    errors: Vec<String>,
}

struct RecoveryPayload {
    origin_path: Option<PathBuf>,
    project: ShadeProject,
    faces: Vec<LoadedFace>,
    errors: Vec<String>,
}
''',
    'RecoveryPayload',
)
app = once(
    app,
    '    Open(Result<OpenPayload, String>),\n    Save {',
    '    Open(Result<OpenPayload, String>),\n    Recover(Result<RecoveryPayload, String>),\n    Save {',
    'JobResult Recover',
)
app = once(
    app,
    '    rgba: Vec<u8>,\n}',
    '    rgba: Vec<u8>,\n    original_rgba: Vec<u8>,\n}',
    'RenderResult original rgba',
)

# ShadeApp fields
app = once(
    app,
    '    allow_close_once: bool,\n    job: Option<JobHandle>,',
    '''    allow_close_once: bool,
    history: history::AdjustmentHistory,
    history_pending_label: Option<String>,
    history_pending_at: Option<Instant>,
    recovery_candidate: Option<recovery::RecoveryFile>,
    autosave_tx: mpsc::Sender<Result<PathBuf, String>>,
    autosave_rx: mpsc::Receiver<Result<PathBuf, String>>,
    autosave_busy: bool,
    last_autosave: Instant,
    job: Option<JobHandle>,''',
    'ShadeApp history/recovery fields',
)

# new(): channels/history/recovery after render channel and log
app = once(
    app,
    '        let (render_tx, render_rx) = mpsc::channel();\n        let mut project = ShadeProject::default();',
    '        let (render_tx, render_rx) = mpsc::channel();\n        let (autosave_tx, autosave_rx) = mpsc::channel();\n        let mut project = ShadeProject::default();',
    'autosave channel init',
)
app = once(
    app,
    '        log.info(&format!(\n            "Shade Editor {} started",\n            env!("CARGO_PKG_VERSION")\n        ));\n        Self {',
    '''        log.info(&format!(
            "Shade Editor {} started",
            env!("CARGO_PKG_VERSION")
        ));
        let recovery_candidate = match recovery::load() {
            Ok(candidate) => candidate,
            Err(err) => {
                log.error(&err);
                None
            }
        };
        let mut history = history::AdjustmentHistory::default();
        history.reset(&project.adjustments, "Start");
        Self {''',
    'new recovery/history setup',
)
app = once(
    app,
    '            allow_close_once: false,\n            job: None,',
    '''            allow_close_once: false,
            history,
            history_pending_label: None,
            history_pending_at: None,
            recovery_candidate,
            autosave_tx,
            autosave_rx,
            autosave_busy: false,
            last_autosave: Instant::now(),
            job: None,''',
    'new field init',
)
app = once(
    app,
    '            texture: None,\n            generation: 1,',
    '            texture: None,\n            original_texture: None,\n            generation: 1,',
    'runtime original texture init',
)

# new project resets history transaction
app = once(
    app,
    '        self.close_after_save = false;\n        self.report_info("New shade project");',
    '''        self.close_after_save = false;
        self.history.reset(&self.project.adjustments, "New project");
        self.history_pending_label = None;
        self.history_pending_at = None;
        self.report_info("New shade project");''',
    'new project history reset',
)

# render result: original preview in worker
app = once(
    app,
    '            let adjusted = render::adjusted_planes(&preview, &project);\n            let rgba = render::rgba_from_planes(&preview, &adjusted, solo_channel);\n            let _ = tx.send(RenderResult {\n                face_index,\n                generation,\n                adjusted,\n                rgba,\n            });',
    '''            let adjusted = render::adjusted_planes(&preview, &project);
            let rgba = render::rgba_from_planes(&preview, &adjusted, solo_channel);
            let original_rgba = render::rgba_from_planes(&preview, &preview.channels, solo_channel);
            let _ = tx.send(RenderResult {
                face_index,
                generation,
                adjusted,
                rgba,
                original_rgba,
            });''',
    'render original rgba',
)

# poll render creates/updates original texture
poll_anchor = '''            if let Some(texture) = &mut face.texture {
                texture.set(image, options);
            } else {
                face.texture = Some(ctx.load_texture(
                    format!("face-preview-{}", result.face_index),
                    image,
                    options,
                ));
            }
            face.rendered_generation = result.generation;'''
poll_new = '''            if let Some(texture) = &mut face.texture {
                texture.set(image, options);
            } else {
                face.texture = Some(ctx.load_texture(
                    format!("face-preview-{}", result.face_index),
                    image,
                    options,
                ));
            }
            let original_image = egui::ColorImage::from_rgba_unmultiplied(
                [face.preview.width, face.preview.height],
                &result.original_rgba,
            );
            if let Some(texture) = &mut face.original_texture {
                texture.set(original_image, options);
            } else {
                face.original_texture = Some(ctx.load_texture(
                    format!("face-original-preview-{}", result.face_index),
                    original_image,
                    options,
                ));
            }
            face.rendered_generation = result.generation;'''
app = once(app, poll_anchor, poll_new, 'poll original texture')

# Snapshot apply is a new history baseline, not an undoable action.
app = once(
    app,
    '            self.mark_all_previews_dirty();\n            self.report_info("Snapshot loaded");',
    '''            self.mark_all_previews_dirty();
            let history_label = self
                .project
                .active_snapshot_name()
                .map(|name| format!("Snapshot - {name}"))
                .unwrap_or_else(|| "Snapshot".to_owned());
            self.history.reset(&self.project.adjustments, history_label);
            self.history_pending_label = None;
            self.history_pending_at = None;
            self.report_info("Snapshot loaded");''',
    'snapshot history baseline',
)

# Insert history/autosave/recovery methods before ui_faces.
methods_anchor = '    fn ui_faces(&mut self, ui: &mut egui::Ui) {'
if methods_anchor not in app:
    raise SystemExit('ui_faces anchor missing')
methods = r'''    fn queue_adjustment_history(&mut self, before: &BTreeMap<String, ChannelAdjustment>) {
        if *before == self.project.adjustments {
            return;
        }
        self.history_pending_label = Some(history::describe_change(before, &self.project.adjustments));
        self.history_pending_at = Some(Instant::now());
    }

    fn commit_pending_history(&mut self, ctx: &egui::Context, force: bool) {
        let Some(label) = self.history_pending_label.clone() else {
            return;
        };
        let ready = force
            || (self
                .history_pending_at
                .is_some_and(|at| at.elapsed() >= HISTORY_COMMIT_DELAY)
                && !ctx.input(|input| input.pointer.any_down()));
        if !ready {
            return;
        }
        self.history.record(&self.project.adjustments, label);
        self.history_pending_label = None;
        self.history_pending_at = None;
    }

    fn apply_history_adjustments(&mut self, adjustments: BTreeMap<String, ChannelAdjustment>, message: &str) {
        self.project.adjustments = adjustments;
        self.history_pending_label = None;
        self.history_pending_at = None;
        self.mark_all_previews_dirty();
        self.report_info(message);
    }

    fn undo_adjustment(&mut self, ctx: &egui::Context) {
        self.commit_pending_history(ctx, true);
        if let Some(adjustments) = self.history.undo() {
            self.apply_history_adjustments(adjustments, "Undo adjustment");
        }
    }

    fn redo_adjustment(&mut self, ctx: &egui::Context) {
        self.commit_pending_history(ctx, true);
        if let Some(adjustments) = self.history.redo() {
            self.apply_history_adjustments(adjustments, "Redo adjustment");
        }
    }

    fn handle_history_shortcuts(&mut self, ctx: &egui::Context) {
        let (undo, redo) = ctx.input(|input| {
            let z = input.key_pressed(egui::Key::Z);
            (
                z && input.modifiers.ctrl && input.modifiers.alt && !input.modifiers.shift,
                z && input.modifiers.ctrl && input.modifiers.shift && !input.modifiers.alt,
            )
        });
        if undo {
            self.undo_adjustment(ctx);
        } else if redo {
            self.redo_adjustment(ctx);
        }
    }

    fn ui_history(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("History");
            if ui
                .add_enabled(self.history.can_undo(), egui::Button::new("Undo").small())
                .on_hover_text("Ctrl+Alt+Z")
                .clicked()
            {
                self.undo_adjustment(ui.ctx());
            }
            if ui
                .add_enabled(self.history.can_redo(), egui::Button::new("Redo").small())
                .on_hover_text("Ctrl+Shift+Z")
                .clicked()
            {
                self.redo_adjustment(ui.ctx());
            }
        });
        ui.small("Adjustment history only. Faces, Snapshots and Palette changes are intentionally excluded.");
        let rows = self
            .history
            .entries()
            .iter()
            .enumerate()
            .map(|(index, entry)| (index, entry.label.clone()))
            .collect::<Vec<_>>();
        let cursor = self.history.cursor();
        let mut requested = None;
        egui::ScrollArea::vertical()
            .id_salt("adjustment-history")
            .max_height(210.0)
            .show(ui, |ui| {
                for (index, label) in rows {
                    if clickable_row(ui, index == cursor, &label, None, None, 28.0).clicked() {
                        requested = Some(index);
                    }
                }
            });
        if let Some(index) = requested {
            self.commit_pending_history(ui.ctx(), true);
            if let Some(adjustments) = self.history.jump(index) {
                self.apply_history_adjustments(adjustments, "History state selected");
            }
        }
    }

    fn poll_autosave(&mut self) {
        while let Ok(result) = self.autosave_rx.try_recv() {
            self.autosave_busy = false;
            match result {
                Ok(path) => self.log.info(&format!("Recovery autosaved: {}", path.display())),
                Err(err) => self.log.error(&format!("Recovery autosave failed: {err}")),
            }
        }
    }

    fn maybe_autosave(&mut self) {
        if !self.project_dirty
            || self.autosave_busy
            || self.job.is_some()
            || self.faces.is_empty()
            || self.last_autosave.elapsed() < AUTOSAVE_INTERVAL
        {
            return;
        }
        let recovery_file = recovery::RecoveryFile::new(
            self.project.clone(),
            self.faces.iter().map(|face| face.path.clone()).collect(),
            self.project_path.clone(),
        );
        let tx = self.autosave_tx.clone();
        self.autosave_busy = true;
        self.last_autosave = Instant::now();
        std::thread::spawn(move || {
            let _ = tx.send(recovery::write(&recovery_file));
        });
    }

    fn recover_project(&mut self) {
        if self.job.is_some() {
            return;
        }
        let Some(candidate) = self.recovery_candidate.take() else {
            return;
        };
        let max_dimension = self.settings.max_preview_dimension;
        let default_dpi = self.settings.default_dpi;
        self.launch_job("Recovering project", move |progress| {
            let result = (|| -> Result<RecoveryPayload, String> {
                let mut project = candidate.project;
                let paths = candidate.resolved_face_paths();
                let total = paths.len().max(1);
                let mut faces = Vec::new();
                let mut errors = Vec::new();
                for (index, source) in paths.into_iter().enumerate() {
                    Self::set_progress(
                        &progress,
                        Some(index as f32 / total as f32),
                        "Recovering project",
                        &source
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    );
                    match tiff_io::load_preview(&source, max_dimension) {
                        Ok(preview) => {
                            project.ensure_channels(&preview.metadata.channel_names);
                            faces.push(LoadedFace {
                                dpi: dpi::read_dpi(&source, default_dpi),
                                path: source,
                                preview,
                            });
                        }
                        Err(err) => errors.push(format!("{}: {err}", source.display())),
                    }
                }
                Ok(RecoveryPayload {
                    origin_path: candidate.origin_path(),
                    project,
                    faces,
                    errors,
                })
            })();
            Self::set_progress(&progress, Some(1.0), "Recovering project", "Complete");
            JobResult::Recover(result)
        });
    }

    fn ui_recovery_window(&mut self, ctx: &egui::Context) {
        let Some(candidate) = self.recovery_candidate.as_ref() else {
            return;
        };
        let saved = Local
            .timestamp_millis_opt(candidate.saved_at_unix_ms)
            .single()
            .map(|value| value.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "unknown time".to_owned());
        let origin = candidate
            .origin_project_path
            .as_deref()
            .unwrap_or("Unsaved project")
            .to_owned();
        let mut recover_now = false;
        let mut discard = false;
        egui::Window::new("Recovery available")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.strong("An autosaved Shade Editor recovery state was found.");
                ui.label(format!("Saved: {saved}"));
                ui.label(format!("Project: {origin}"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    recover_now = ui.button("Recover").clicked();
                    discard = ui.button("Discard recovery").clicked();
                });
            });
        if recover_now {
            self.recover_project();
        } else if discard {
            if let Err(err) = recovery::clear() {
                self.report_error(err);
            }
            self.recovery_candidate = None;
        }
    }

'''
app = app.replace(methods_anchor, methods + methods_anchor, 1)

# AddFaces and Open success reset history baseline. Insert near their report_info calls.
app = once(
    app,
    '                    self.project_dirty = true;\n                    self.report_info(format!("Added {added} face(s)"));',
    '''                    self.project_dirty = true;
                    self.history.reset(&self.project.adjustments, "Faces changed");
                    self.history_pending_label = None;
                    self.history_pending_at = None;
                    self.report_info(format!("Added {added} face(s)"));''',
    'AddFaces history exclusion',
)
app = once(
    app,
    '                    self.project_dirty = false;\n                    self.report_info(format!("Opened {}", payload.path.display()));',
    '''                    self.project_dirty = false;
                    self.history.reset(&self.project.adjustments, "Open project");
                    self.history_pending_label = None;
                    self.history_pending_at = None;
                    self.report_info(format!("Opened {}", payload.path.display()));''',
    'Open history reset',
)

# Insert Recover arm before Save.
recover_arm_anchor = '            JobResult::Save { path, result } => match result {'
recover_arm = r'''            JobResult::Recover(result) => match result {
                Ok(payload) => {
                    self.project = payload.project;
                    self.project_path = payload.origin_path;
                    self.faces = payload
                        .faces
                        .into_iter()
                        .map(Self::make_runtime_face)
                        .collect();
                    self.current_face = 0;
                    self.selected_channel = 0;
                    self.solo_channel = None;
                    self.adjustment_scope = AdjustmentScope::Selected;
                    self.fit_requested = true;
                    self.viewport_recenter = true;
                    self.project_dirty = true;
                    self.history.reset(&self.project.adjustments, "Recovered project");
                    self.history_pending_label = None;
                    self.history_pending_at = None;
                    self.last_autosave = Instant::now();
                    self.report_info("Recovered autosaved project state");
                    if !payload.errors.is_empty() {
                        self.report_error(format!(
                            "Recovery opened with TIFF errors: {}",
                            payload.errors.join(" | ")
                        ));
                    }
                }
                Err(err) => self.report_error(format!("Recovery failed: {err}")),
            },
'''
if recover_arm_anchor not in app:
    raise SystemExit('Save arm anchor missing')
app = app.replace(recover_arm_anchor, recover_arm + recover_arm_anchor, 1)

# Successful save clears crash recovery.
app = once(
    app,
    '                    self.project_dirty = false;\n                    self.report_info(format!("Saved {}", path.display()));',
    '''                    self.project_dirty = false;
                    if let Err(err) = recovery::clear() {
                        self.log.error(&err);
                    }
                    self.report_info(format!("Saved {}", path.display()));''',
    'clear recovery on save',
)

# Discard-and-exit explicitly discards recovery too.
app = once(
    app,
    '        } else if discard_and_exit {\n            self.show_close_confirmation = false;\n            self.allow_close_once = true;',
    '''        } else if discard_and_exit {
            self.show_close_confirmation = false;
            if let Err(err) = recovery::clear() {
                self.log.error(&err);
            }
            self.allow_close_once = true;''',
    'discard exit clears recovery',
)

# Adjustment changes queue a coalesced history state.
adj_start = app.index('    fn ui_adjustments(&mut self, ui: &mut egui::Ui) {')
adj_end = app.index('    fn ui_selected_adjustment(', adj_start)
adj = app[adj_start:adj_end]
adj = adj.replace(
    '    fn ui_adjustments(&mut self, ui: &mut egui::Ui) {\n',
    '    fn ui_adjustments(&mut self, ui: &mut egui::Ui) {\n        let adjustments_before = self.project.adjustments.clone();\n',
    1,
)
needle = '''        if changed {
            self.mark_all_previews_dirty();
        }
    }
'''
if needle not in adj:
    raise SystemExit('ui_adjustments ending not found')
adj = adj.replace(
    needle,
    '''        if changed {
            self.mark_all_previews_dirty();
        }
        if self.project.adjustments != adjustments_before {
            self.queue_adjustment_history(&adjustments_before);
        }
    }
''',
    1,
)
app = app[:adj_start] + adj + app[adj_end:]

# History panel below channel/histogram content in both sidebar layouts.
app = once(
    app,
    '.show(&mut columns[0], |ui| self.ui_channels_histogram(ui));',
    '''.show(&mut columns[0], |ui| {
                        self.ui_channels_histogram(ui);
                        ui.separator();
                        self.ui_history(ui);
                    });''',
    'two-column history panel',
)
app = once(
    app,
    '                self.ui_channels_histogram(ui);\n                ui.separator();\n                self.ui_adjustments(ui);',
    '                self.ui_channels_histogram(ui);\n                ui.separator();\n                self.ui_history(ui);\n                ui.separator();\n                self.ui_adjustments(ui);',
    'single-column history panel',
)

# Dirty Snapshot gets stronger visual treatment.
app = once(
    app,
    '                    selected,\n                    &display_name,\n                    &time,',
    '                    selected,\n                    selected && active_dirty,\n                    &display_name,\n                    &time,',
    'snapshot dirty argument',
)
app = once(
    app,
    '    selected: bool,\n    left: &str,\n    time: &str,',
    '    selected: bool,\n    dirty: bool,\n    left: &str,\n    time: &str,',
    'snapshot row dirty signature',
)
app = once(
    app,
    '''    let fill = if selected {
        visuals.selection.bg_fill.gamma_multiply(0.72)
    } else if row_response.hovered() {''',
    '''    let fill = if dirty {
        visuals.selection.bg_fill.gamma_multiply(1.12)
    } else if selected {
        visuals.selection.bg_fill.gamma_multiply(0.72)
    } else if row_response.hovered() {''',
    'snapshot dirty fill',
)
app = once(
    app,
    '''    if fill != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, fill);
    }

    let action_width = 26.0;''',
    '''    if fill != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, fill);
    }
    if dirty {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            4.0,
            egui::Stroke::new(1.5, visuals.selection.stroke.color),
            egui::StrokeKind::Inside,
        );
        ui.painter().rect_filled(
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 3.0, rect.bottom())),
            2.0,
            visuals.selection.stroke.color,
        );
    }

    let action_width = 26.0;''',
    'snapshot dirty border',
)
app = once(
    app,
    '        egui::FontId::proportional(14.0),\n        if selected {',
    '        egui::FontId::proportional(if dirty { 15.0 } else { 14.0 }),\n        if selected {',
    'snapshot dirty font',
)

# Before/After: retain original texture and show it while RMB is held over viewport.
app = once(
    app,
    '        let texture = face.texture.clone();\n        let (width_cm, height_cm)',
    '        let texture = face.texture.clone();\n        let original_texture = face.original_texture.clone();\n        let (width_cm, height_cm)',
    'viewport original texture clone',
)
old_put = '''                ui.put(
                    image_rect,
                    egui::Image::from_texture(&texture).fit_to_exact_size(image_size),
                );
                if recenter {'''
new_put = '''                let show_before = ui.input(|input| {
                    input.pointer.secondary_down()
                        && input.pointer.hover_pos().is_some_and(|pos| viewport.contains(pos))
                });
                let display_texture = if show_before {
                    original_texture.as_ref().unwrap_or(&texture)
                } else {
                    &texture
                };
                ui.put(
                    image_rect,
                    egui::Image::from_texture(display_texture).fit_to_exact_size(image_size),
                );
                if show_before {
                    ui.painter().text(
                        image_rect.left_top() + egui::vec2(10.0, 10.0),
                        egui::Align2::LEFT_TOP,
                        "BEFORE",
                        egui::FontId::proportional(13.0),
                        egui::Color32::WHITE,
                    );
                }
                if recenter {'''
app = once(app, old_put, new_put, 'viewport before/after')

# App frame hooks: shortcuts, autosave, recovery UI, coalesced history commit.
app = once(
    app,
    '        self.sync_update_state();\n        self.handle_close_request(ui.ctx());',
    '''        self.sync_update_state();
        self.poll_autosave();
        self.handle_history_shortcuts(ui.ctx());
        self.maybe_autosave();
        self.handle_close_request(ui.ctx());''',
    'app frame history/autosave hooks',
)
app = once(
    app,
    '        self.ui_logs_window(ui.ctx());\n        self.ui_snapshot_discard_confirmation(ui.ctx());',
    '        self.ui_logs_window(ui.ctx());\n        self.ui_recovery_window(ui.ctx());\n        self.ui_snapshot_discard_confirmation(ui.ctx());',
    'recovery window hook',
)
app = once(
    app,
    '        self.ui_close_confirmation(ui.ctx());\n\n        self.start_render_if_needed(ui.ctx());',
    '        self.ui_close_confirmation(ui.ctx());\n        self.commit_pending_history(ui.ctx(), false);\n\n        self.start_render_if_needed(ui.ctx());',
    'history commit hook',
)

# Release notes
release = '''# Shade Editor 0.9.0

Production-workflow foundation: adjustment History, crash recovery, Before/After comparison, and a clean .shade v9 schema.

- Photoshop-style adjustment Undo/Redo shortcuts: Ctrl+Alt+Z and Ctrl+Shift+Z, plus a clickable History panel. Only adjustment edits participate; Face operations, Snapshot operations, and Palette changes are intentionally excluded.
- Adjustment drag/keyboard edits are coalesced into useful history states instead of recording every render frame.
- Dirty active Snapshots retain the existing marker but now get a stronger selected background, border, and visual emphasis.
- Hold the right mouse button over the image viewport to temporarily show the unadjusted Before view. Space remains available for viewport panning.
- Recovery autosaves dirty projects every two minutes to LOCALAPPDATA without marking the project saved. On restart the app offers Recover or Discard recovery.
- Successful manual Save clears the recovery copy; Save and exit still waits for the background save to complete.
- .shade schema v9 is intentionally a clean break. All v1-v8 migration code was removed and the loader accepts schema v9 only.
- TIFF preview/export streaming improvements are part of the same v0.9 release backend work.

'''
notes = release + notes

APP.write_text(app, encoding='utf-8')
MODEL.write_text(model, encoding='utf-8')
CARGO.write_text(cargo, encoding='utf-8')
NOTES.write_text(notes, encoding='utf-8')
