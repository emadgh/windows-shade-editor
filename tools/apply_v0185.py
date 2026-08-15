from pathlib import Path
import re


def read(path):
    return Path(path).read_text(encoding="utf-8")


def write(path, text):
    Path(path).write_text(text, encoding="utf-8")


def replace_once(text, old, new, label):
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected 1 anchor, found {count}")
    return text.replace(old, new, 1)


def regex_once(text, pattern, replacement, label):
    new_text, count = re.subn(pattern, lambda _match: replacement, text, count=1, flags=re.S)
    if count != 1:
        raise SystemExit(f"{label}: expected 1 regex match, found {count}")
    return new_text


# Version and release panic policy.
cargo = read("Cargo.toml")
cargo = replace_once(cargo, 'version = "0.18.4"', 'version = "0.18.5"', "cargo version")
cargo = replace_once(cargo, 'panic = "abort"', 'panic = "unwind"', "release panic policy")
write("Cargo.toml", cargo)

lock = read("Cargo.lock")
lock = replace_once(
    lock,
    'name = "windows-shade-editor"\nversion = "0.18.4"',
    'name = "windows-shade-editor"\nversion = "0.18.5"',
    "lock version",
)
write("Cargo.lock", lock)
write("VERSION", "0.18.5\n")

notes = read("RELEASE_NOTES.md")
section = """# Shade Editor 0.18.5\n\n- Harden Export Queue runtime stability: Release builds now unwind worker panics instead of aborting the entire process, export worker panics are converted into Failed queue rows, and panic details are written to the application log.\n- Move the large disk-backed export spool to a local ShadeEditor cache directory before memory mapping. Final TIFF staging/commit still happens beside the requested destination, including UNC/network destinations.\n- Protect preview/background jobs from taking down the whole process and surface unexpected worker termination as an application error.\n- Replace the two Light/Pigment selector buttons with one toggle button everywhere.\n- Correct the user-facing Light/Pigment labels while retaining the existing serialized enum values for settings compatibility.\n- Add the same tonal-direction toggle to Settings > Color guides.\n- Add regression coverage for worker panic isolation, local spool placement, and queue polling/enqueue activity while an export is processing.\n\n"""
if not notes.startswith("# Shade Editor 0.18.5"):
    notes = section + notes
write("RELEASE_NOTES.md", notes)

# App log panic hook.
app_log = read("src/app_log.rs")
insert = """

pub fn install_panic_hook() {
    let log = AppLog::default();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = if let Some(message) = info.payload().downcast_ref::<&str>() {
            (*message).to_owned()
        } else if let Some(message) = info.payload().downcast_ref::<String>() {
            message.clone()
        } else {
            "unknown panic payload".to_owned()
        };
        let location = info
            .location()
            .map(|location| format!("{}:{}:{}", location.file(), location.line(), location.column()))
            .unwrap_or_else(|| "unknown location".to_owned());
        log.write("PANIC", &format!("{payload} @ {location}"));
        default_hook(info);
    }));
}
"""
if "pub fn install_panic_hook()" not in app_log:
    app_log += insert
write("src/app_log.rs", app_log)

# Tonal display labels: keep enum serialization stable, correct only user-facing names.
settings = read("src/settings.rs")
settings = replace_once(
    settings,
    '''    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Pigment => "Pigment",
        }
    }
''',
    '''    pub fn label(self) -> &'static str {
        // v0.18.4 shipped the two presentation names reversed. Keep the enum
        // variants stable for settings compatibility and correct only the UI label.
        match self {
            Self::Light => "Pigment",
            Self::Pigment => "Light",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Light => Self::Pigment,
            Self::Pigment => Self::Light,
        }
    }
''',
    "tonal label correction",
)
settings = replace_once(
    settings,
    '''    #[test]
    fn compact_curve_controls_default_off() {
        assert!(!AppSettings::default().compact_curve_controls);
    }
''',
    '''    #[test]
    fn compact_curve_controls_default_off() {
        assert!(!AppSettings::default().compact_curve_controls);
    }

    #[test]
    fn tonal_display_labels_match_the_corrected_ui_names() {
        assert_eq!(TonalDisplayMode::Light.label(), "Pigment");
        assert_eq!(TonalDisplayMode::Pigment.label(), "Light");
        assert_eq!(TonalDisplayMode::Light.toggled(), TonalDisplayMode::Pigment);
    }
''',
    "tonal label test",
)
write("src/settings.rs", settings)

# Keep the large mmap-backed spool local, never on an SMB/UNC destination.
export = read("src/export.rs")
export = replace_once(
    export,
    "use std::path::{Path, PathBuf};\n",
    "use std::path::{Path, PathBuf};\nuse std::sync::atomic::{AtomicU64, Ordering};\n",
    "export atomic imports",
)
if "static EXPORT_SPOOL_SEQUENCE" not in export:
    export = replace_once(
        export,
        "use crate::tiff_io::{\n",
        "static EXPORT_SPOOL_SEQUENCE: AtomicU64 = AtomicU64::new(1);\n\nuse crate::tiff_io::{\n",
        "spool sequence static",
    )
export = regex_once(
    export,
    r'''fn temporary_spool_path\(destination: &Path\) -> Result<PathBuf, String> \{.*?\n\}\n\nfn temporary_export_path''',
    '''fn local_export_spool_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ShadeEditor")
        .join("export-spool")
}

fn temporary_spool_path(_destination: &Path) -> Result<PathBuf, String> {
    let root = local_export_spool_root();
    fs::create_dir_all(&root)
        .map_err(|err| format!("Cannot create local export spool folder {}: {err}", root.display()))?;
    let sequence = EXPORT_SPOOL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(root.join(format!(
        "export-{}-{sequence}.spool.tmp",
        std::process::id()
    )))
}

fn temporary_export_path''',
    "local spool path",
)
export = regex_once(
    export,
    r'''    #\[test\]\n    fn temporary_export_and_spool_names_stay_short_and_deterministic\(\) \{.*?\n    \}\n''',
    '''    #[test]
    fn temporary_export_name_stays_short_and_spool_is_local() {
        let destination = Path::new(r"\\\\192.168.100.154\\DurstPrinter\\TEST\\Fabia_Gray_S8-E6_2026-08-15.tif");
        let temporary = temporary_export_path(destination).unwrap();
        assert_eq!(
            temporary.file_name().unwrap().to_string_lossy(),
            "Fabia_Gray_S8-E6_2026-08-15.tif.tmp"
        );
        let spool = temporary_spool_path(&temporary).unwrap();
        assert_eq!(spool.parent().unwrap(), local_export_spool_root());
        assert_ne!(spool.parent(), temporary.parent());
        assert!(spool.file_name().unwrap().to_string_lossy().ends_with(".spool.tmp"));
    }
''',
    "local spool regression test",
)
write("src/export.rs", export)

# Export queue: panic in a worker becomes a Failed item instead of terminating the app.
queue = read("src/export_queue.rs")
queue = replace_once(
    queue,
    "use crate::validation;\n",
    "use crate::validation;\nuse crate::worker_guard;\n",
    "queue worker guard import",
)
queue = regex_once(
    queue,
    r'''        thread::spawn\(move \|\| \{\n            let validate_after_export = spec\.validate_after_export;.*?            let mark = result\.as_ref\(\)\.ok\(\)\.and\(spec\.mark\);\n            let _ = tx\.send\(ExportQueueEvent::Finished \{\n                id,\n                project_session_id: session_id,\n                result,\n                mark,\n            \}\);\n        \}\);''',
    '''        thread::spawn(move || {
            let mark = spec.mark.clone();
            let result = worker_guard::catch_result("Export worker", || {
                let validate_after_export = spec.validate_after_export;
                let progress_tx = tx.clone();
                let project = spec.recipe.materialize_project();
                export::export_face_with_progress_options(
                    &spec.source,
                    &spec.destination,
                    &project,
                    spec.default_dpi,
                    export::ExportOptions {
                        force_lzw: spec.force_lzw,
                    },
                    move |fraction, detail| {
                        let _ = progress_tx.send(ExportQueueEvent::Progress {
                            id,
                            fraction: if validate_after_export {
                                fraction * 0.90
                            } else {
                                fraction
                            },
                            detail: detail.to_owned(),
                        });
                    },
                )
                .and_then(|_| {
                    if spec.validate_after_export {
                        let _ = tx.send(ExportQueueEvent::Progress {
                            id,
                            fraction: 0.94,
                            detail: "Validating exported TIFF".to_owned(),
                        });
                        let verified = validation::validate_export_transport_with_options(
                            &spec.source,
                            &spec.destination,
                            spec.force_lzw,
                        )?;
                        Ok(format!("Done · {verified}"))
                    } else {
                        Ok("Done".to_owned())
                    }
                })
            });
            let mark = result.as_ref().ok().and(mark);
            let _ = tx.send(ExportQueueEvent::Finished {
                id,
                project_session_id: session_id,
                result,
                mark,
            });
        });''',
    "queue panic isolation",
)
queue = replace_once(
    queue,
    '''    #[test]
    fn restored_waiting_work_requires_explicit_resume() {
''',
    '''    #[test]
    fn queue_can_be_read_and_extended_while_an_item_is_processing() {
        let mut queue = ExportQueue::new();
        let first = queue.enqueue(spec("first.tif"));
        queue.items[0].status = ExportQueueStatus::Processing;
        queue.active_id = Some(first);
        let second = queue.enqueue(spec("second.tif"));

        for _ in 0..200 {
            let rows = queue
                .items()
                .iter()
                .map(|item| (item.id, item.status, item.progress, item.detail.clone()))
                .collect::<Vec<_>>();
            assert_eq!(rows.len(), 2);
            assert_eq!(queue.active_id, Some(first));
            assert_eq!(queue.items()[1].id, second);
            assert_eq!(queue.items()[1].status, ExportQueueStatus::Waiting);
        }
    }

    #[test]
    fn restored_waiting_work_requires_explicit_resume() {
''',
    "queue activity regression test",
)
write("src/export_queue.rs", queue)

# Main application: install panic logging, isolate generic jobs/render workers, single tonal toggle,
# and expose that toggle under Settings > Color guides.
main = read("src/main.rs")
main = replace_once(main, "mod workflow;\n", "mod workflow;\nmod worker_guard;\n", "worker guard module")
main = replace_once(
    main,
    "fn main() -> eframe::Result {\n    let startup_project",
    "fn main() -> eframe::Result {\n    app_log::install_panic_hook();\n    let startup_project",
    "panic hook startup",
)
main = replace_once(
    main,
    '''    Export(SnapshotExportBatchResult),
}
''',
    '''    Export(SnapshotExportBatchResult),
    WorkerPanic(String),
}
''',
    "job panic variant",
)
main = replace_once(
    main,
    '''struct RenderResult {
''',
    '''struct RenderFailure {
    face_index: usize,
    generation: u64,
    message: String,
}

type RenderMessage = Result<RenderResult, RenderFailure>;

struct RenderResult {
''',
    "render message type",
)
main = replace_once(
    main,
    '''    render_tx: mpsc::Sender<RenderResult>,
    render_rx: mpsc::Receiver<RenderResult>,
''',
    '''    render_tx: mpsc::Sender<RenderMessage>,
    render_rx: mpsc::Receiver<RenderMessage>,
''',
    "render channel type",
)
main = replace_once(
    main,
    '''        std::thread::spawn(move || {
            let result = task(worker_progress);
            let _ = tx.send(result);
        });
''',
    '''        std::thread::spawn(move || {
            let result = worker_guard::catch_value("Background operation", || task(worker_progress))
                .unwrap_or_else(JobResult::WorkerPanic);
            let _ = tx.send(result);
        });
''',
    "generic job panic isolation",
)
main = replace_once(
    main,
    '''            JobResult::Export(payload) => {
''',
    '''            JobResult::WorkerPanic(err) => {
                self.report_error(err);
            }
            JobResult::Export(payload) => {
''',
    "poll generic worker panic",
)
main = replace_once(
    main,
    '''        self.render_busy = Some((face_index, generation));
        std::thread::spawn(move || {
            let (adjusted, clipping) = render::adjusted_planes_with_stats(&preview, &project);
''',
    '''        self.render_busy = Some((face_index, generation));
        std::thread::spawn(move || {
            let outcome = worker_guard::catch_value("Preview render worker", || {
                let (adjusted, clipping) = render::adjusted_planes_with_stats(&preview, &project);
''',
    "render panic wrapper start",
)
main = replace_once(
    main,
    '''            let _ = tx.send(RenderResult {
''',
    '''                RenderResult {
''',
    "render result return",
)
main = replace_once(
    main,
    '''                embedded_original_status,
            });
        });
    }

    fn select_channel''',
    '''                    embedded_original_status,
                }
            });
            let message = match outcome {
                Ok(result) => Ok(result),
                Err(message) => Err(RenderFailure {
                    face_index,
                    generation,
                    message,
                }),
            };
            let _ = tx.send(message);
        });
    }

    fn select_channel''',
    "render panic wrapper tail",
)
main = replace_once(
    main,
    '''    fn poll_render(&mut self, ctx: &egui::Context) {
        while let Ok(result) = self.render_rx.try_recv() {
            let face_index = result.face_index;
''',
    '''    fn poll_render(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.render_rx.try_recv() {
            let result = match message {
                Ok(result) => result,
                Err(failure) => {
                    if self.render_busy == Some((failure.face_index, failure.generation)) {
                        self.render_busy = None;
                    }
                    self.report_error(failure.message);
                    continue;
                }
            };
            let face_index = result.face_index;
''',
    "poll render failure",
)
main = replace_once(
    main,
    '''fn tonal_display_mode_selector(ui: &mut egui::Ui, mode: &mut TonalDisplayMode) -> bool {
    let mut changed = false;
    changed |= ui
        .selectable_value(mode, TonalDisplayMode::Light, "Light")
        .on_hover_text(
            "Light: 0 is black and 255 is white, matching the current Shade Editor display.",
        )
        .changed();
    changed |= ui
        .selectable_value(mode, TonalDisplayMode::Pigment, "Pigment")
        .on_hover_text("Pigment: mirrors Curve axes and histograms like Photoshop Pigment/Ink, while keeping the labels 0-255. TIFF adjustment math is unchanged.")
        .changed();
    changed
}
''',
    '''fn tonal_display_mode_selector(ui: &mut egui::Ui, mode: &mut TonalDisplayMode) -> bool {
    let current = mode.label();
    let next = mode.toggled().label();
    if ui
        .button(format!("Mode: {current}"))
        .on_hover_text(format!(
            "Click to switch Curve and Histogram display to {next}. This only changes presentation and interaction; TIFF adjustment math is unchanged."
        ))
        .clicked()
    {
        *mode = mode.toggled();
        true
    } else {
        false
    }
}
''',
    "single tonal toggle",
)
main = replace_once(
    main,
    '''                ui.heading("Color guides");
                changed |= ui
''',
    '''                ui.heading("Color guides");
                ui.horizontal(|ui| {
                    ui.label("Curve / Histogram direction");
                    changed |= tonal_display_mode_selector(ui, &mut self.settings.tonal_display_mode);
                });
                changed |= ui
''',
    "settings tonal toggle",
)
write("src/main.rs", main)

architecture = read("docs/ARCHITECTURE.md")
append = """

## Export worker isolation (v0.18.5)

Production Release builds use unwind semantics so a panic in a background worker cannot abort the entire GUI process. Export Queue workers convert panics into normal Failed completions and the global panic hook records details in `%LOCALAPPDATA%/ShadeEditor/shade-editor.log`. The large raw streaming spool is always created under the local ShadeEditor application-data directory before memory mapping; UNC/network destinations only receive the final temporary TIFF and atomic commit. This keeps the memory-mapped backing file local while preserving bounded-RAM export and network output support.
"""
if "## Export worker isolation (v0.18.5)" not in architecture:
    architecture += append
write("docs/ARCHITECTURE.md", architecture)

print("v0.18.5 migration applied")
