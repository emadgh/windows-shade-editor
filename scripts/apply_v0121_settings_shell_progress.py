from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]


def read(rel: str) -> str:
    return (ROOT / rel).read_text(encoding="utf-8")


def write(rel: str, text: str) -> None:
    (ROOT / rel).write_text(text, encoding="utf-8")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    if old not in text:
        raise RuntimeError(f"patch anchor not found: {label}")
    return text.replace(old, new, 1)


def regex_once(text: str, pattern: str, replacement: str, label: str) -> str:
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise RuntimeError(f"regex patch anchor not found: {label} ({count})")
    return updated


# Idempotence for the PR workflow's second run after the patch commit is pushed.
if (
    'version = "0.12.1"' in read("Cargo.toml")
    and "pub lzw_compression: bool" in read("src/settings_v6.rs")
    and "export_face_with_progress_options" in read("src/export_v6.rs")
    and "Install Shell integration" in read("src/app_main.rs")
):
    print("v0.12.1 patch already applied")
    raise SystemExit(0)

# Version.
cargo = read("Cargo.toml")
cargo = replace_once(cargo, 'version = "0.12.0"', 'version = "0.12.1"', "Cargo version")
write("Cargo.toml", cargo)

# Persistent application settings: LZW is opt-out and therefore enabled for old
# settings files too because the struct uses #[serde(default)].
settings = read("src/settings_v6.rs")
settings = replace_once(
    settings,
    "    pub validate_after_export: bool,\n    pub default_dpi: f64,",
    "    pub validate_after_export: bool,\n    pub lzw_compression: bool,\n    pub default_dpi: f64,",
    "settings LZW field",
)
settings = replace_once(
    settings,
    "            validate_after_export: false,\n            default_dpi: DEFAULT_DPI,",
    "            validate_after_export: false,\n            lzw_compression: true,\n            default_dpi: DEFAULT_DPI,",
    "settings LZW default",
)
settings = replace_once(
    settings,
    "    fn post_export_validation_defaults_off() {\n        assert!(!AppSettings::default().validate_after_export);\n    }\n",
    "    fn post_export_validation_defaults_off() {\n        assert!(!AppSettings::default().validate_after_export);\n    }\n\n    #[test]\n    fn lzw_compression_defaults_on() {\n        assert!(AppSettings::default().lzw_compression);\n    }\n",
    "settings LZW test",
)
write("src/settings_v6.rs", settings)

# Export backend: expose an explicit compression option while retaining the old
# API as a default-LZW compatibility wrapper.
export = read("src/export_v6.rs")
export = replace_once(
    export,
    "use crate::tiff_io::{\n    ColorModel, StreamInfo, TiffMetadata, decode_full, for_each_decoded_region,\n    for_each_decoded_strip, stream_info,\n};\n\npub fn export_face(",
    "use crate::tiff_io::{\n    ColorModel, StreamInfo, TiffMetadata, decode_full, for_each_decoded_region,\n    for_each_decoded_strip, stream_info,\n};\n\n#[derive(Clone, Copy, Debug)]\npub struct ExportOptions {\n    pub force_lzw: bool,\n}\n\nimpl Default for ExportOptions {\n    fn default() -> Self {\n        Self { force_lzw: true }\n    }\n}\n\npub fn export_face(",
    "export options type",
)
export = regex_once(
    export,
    r"pub fn export_face_with_progress<F>\(.*?\n\}\n\nfn export_face_direct_with_progress<F>",
    '''pub fn export_face_with_progress<F>(
    source: &Path,
    destination: &Path,
    project: &ShadeProject,
    default_dpi: f64,
    progress: F,
) -> Result<(), String>
where
    F: FnMut(f32, &str),
{
    export_face_with_progress_options(
        source,
        destination,
        project,
        default_dpi,
        ExportOptions::default(),
        progress,
    )
}

pub fn export_face_with_progress_options<F>(
    source: &Path,
    destination: &Path,
    project: &ShadeProject,
    default_dpi: f64,
    options: ExportOptions,
    mut progress: F,
) -> Result<(), String>
where
    F: FnMut(f32, &str),
{
    let temporary = temporary_export_path(destination)?;
    let result = export_face_direct_with_progress(
        source,
        &temporary,
        project,
        default_dpi,
        options,
        |fraction, detail| progress((fraction * 0.98).clamp(0.0, 0.98), detail),
    );
    if let Err(err) = result {
        let _ = fs::remove_file(&temporary);
        return Err(err);
    }
    progress(0.99, "Committing TIFF atomically");
    if let Err(err) = atomic_replace(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(err);
    }
    progress(1.0, "Export complete");
    Ok(())
}

fn export_face_direct_with_progress<F>''',
    "export progress options wrapper",
)
export = replace_once(
    export,
    "    project: &ShadeProject,\n    default_dpi: f64,\n    mut progress: F,\n) -> Result<(), String>\nwhere\n    F: FnMut(f32, &str),\n{\n    progress(0.02, \"Inspecting TIFF\");",
    "    project: &ShadeProject,\n    default_dpi: f64,\n    options: ExportOptions,\n    mut progress: F,\n) -> Result<(), String>\nwhere\n    F: FnMut(f32, &str),\n{\n    progress(0.02, \"Inspecting TIFF\");",
    "direct export options argument",
)
export = replace_once(
    export,
    "            project,\n            default_dpi,\n            &stream,\n            &mut progress,",
    "            project,\n            default_dpi,\n            options,\n            &stream,\n            &mut progress,",
    "streaming call options",
)
export = export.replace(
    "                dpi_info,\n                None,\n                OutputPixels::",
    "                dpi_info,\n                options,\n                None,\n                OutputPixels::",
)
export = replace_once(
    export,
    "    project: &ShadeProject,\n    default_dpi: f64,\n    stream: &StreamInfo,",
    "    project: &ShadeProject,\n    default_dpi: f64,\n    options: ExportOptions,\n    stream: &StreamInfo,",
    "stream export options argument",
)
export = export.replace(
    "                    dpi_info,\n                    Some(stream.rows_per_strip),\n                    OutputPixels::",
    "                    dpi_info,\n                    options,\n                    Some(stream.rows_per_strip),\n                    OutputPixels::",
)
export = replace_once(
    export,
    "fn configure_tiff_encoder<W, K>(\n    mut encoder: TiffEncoder<W, K>,\n    metadata: &TiffMetadata,\n) -> TiffEncoder<W, K>",
    "fn configure_tiff_encoder<W, K>(\n    mut encoder: TiffEncoder<W, K>,\n    metadata: &TiffMetadata,\n    options: ExportOptions,\n) -> TiffEncoder<W, K>",
    "configure encoder options argument",
)
export = replace_once(
    export,
    "    let compression = match metadata.compression {\n        Some(1) => Compression::Uncompressed,\n        Some(5) => Compression::Lzw,\n        Some(8 | 32946) => Compression::Deflate(tiff::encoder::DeflateLevel::Balanced),\n        Some(32773) => Compression::Packbits,\n        _ => Compression::Lzw,\n    };",
    "    let compression = if options.force_lzw {\n        Compression::Lzw\n    } else {\n        match metadata.compression {\n            Some(1) => Compression::Uncompressed,\n            Some(5) => Compression::Lzw,\n            Some(8 | 32946) => Compression::Deflate(tiff::encoder::DeflateLevel::Balanced),\n            Some(32773) => Compression::Packbits,\n            _ => Compression::Lzw,\n        }\n    };",
    "LZW compression policy",
)
export = replace_once(
    export,
    "    metadata: &TiffMetadata,\n    dpi_info: DpiInfo,\n    rows_per_strip: Option<u32>,",
    "    metadata: &TiffMetadata,\n    dpi_info: DpiInfo,\n    options: ExportOptions,\n    rows_per_strip: Option<u32>,",
    "write TIFF options argument",
)
export = export.replace(
    "configure_tiff_encoder(encoder, metadata);",
    "configure_tiff_encoder(encoder, metadata, options);",
)
write("src/export_v6.rs", export)

# Validation follows the selected compression policy instead of assuming source
# compression preservation.
validation = read("src/validation.rs")
validation = regex_once(
    validation,
    r"pub fn validate_no_adjustment_roundtrip<F>\(.*?\n\{\n    if !source\.is_file\(\) \{",
    '''pub fn validate_no_adjustment_roundtrip<F>(
    source: &Path,
    output_folder: &Path,
    default_dpi: f64,
    progress: F,
) -> Result<ValidationArtifacts, String>
where
    F: FnMut(f32, &str),
{
    validate_no_adjustment_roundtrip_with_options(
        source,
        output_folder,
        default_dpi,
        true,
        progress,
    )
}

pub fn validate_no_adjustment_roundtrip_with_options<F>(
    source: &Path,
    output_folder: &Path,
    default_dpi: f64,
    force_lzw: bool,
    mut progress: F,
) -> Result<ValidationArtifacts, String>
where
    F: FnMut(f32, &str),
{
    if !source.is_file() {''',
    "validation options wrapper",
)
validation = replace_once(
    validation,
    "    export::export_face_with_progress(\n        source,\n        &export_path,\n        &identity_project,\n        default_dpi,\n        |fraction, detail| progress(0.12 + fraction * 0.58, detail),\n    )?;",
    "    export::export_face_with_progress_options(\n        source,\n        &export_path,\n        &identity_project,\n        default_dpi,\n        export::ExportOptions { force_lzw },\n        |fraction, detail| progress(0.12 + fraction * 0.58, detail),\n    )?;",
    "validation export options",
)
validation = validation.replace(
    "expected_export_compression(source_decoded.metadata.compression)",
    "expected_export_compression(source_decoded.metadata.compression, force_lzw)",
    1,
)
validation = replace_once(
    validation,
    "pub fn validate_export_transport(source: &Path, exported: &Path) -> Result<String, String> {\n    let source_info =",
    "pub fn validate_export_transport(source: &Path, exported: &Path) -> Result<String, String> {\n    validate_export_transport_with_options(source, exported, true)\n}\n\npub fn validate_export_transport_with_options(\n    source: &Path,\n    exported: &Path,\n    force_lzw: bool,\n) -> Result<String, String> {\n    let source_info =",
    "transport validation options wrapper",
)
validation = validation.replace(
    "expected_export_compression(source_meta.compression)",
    "expected_export_compression(source_meta.compression, force_lzw)",
    1,
)
validation = replace_once(
    validation,
    "fn expected_export_compression(source: Option<u16>) -> Option<u16> {\n    match source {\n        Some(1 | 5 | 8 | 32946 | 32773) => source,\n        _ => Some(5),\n    }\n}",
    "fn expected_export_compression(source: Option<u16>, force_lzw: bool) -> Option<u16> {\n    if force_lzw {\n        return Some(5);\n    }\n    match source {\n        Some(1 | 5 | 8 | 32946 | 32773) => source,\n        _ => Some(5),\n    }\n}",
    "expected compression policy",
)
write("src/validation.rs", validation)

# UI/application wiring.
app = read("src/app_main.rs")
app = replace_once(
    app,
    "const ERROR_TOAST_LIFETIME: Duration = Duration::from_secs(120);",
    "const ERROR_TOAST_LIFETIME: Duration = Duration::from_secs(8);",
    "error toast lifetime",
)
app = replace_once(
    app,
    "        self.log.error(&message);\n        self.status_message = message.clone();\n        self.toast = Some(ErrorToast {",
    "        self.log.error(&message);\n        self.status_message = \"Error - see Logs\".to_owned();\n        self.toast = Some(ErrorToast {",
    "error status message",
)
app = replace_once(
    app,
    "        if self\n            .toast\n            .as_ref()\n            .is_some_and(|toast| toast.created.elapsed() > ERROR_TOAST_LIFETIME)\n        {\n            self.toast = None;\n        }",
    "        if self\n            .toast\n            .as_ref()\n            .is_some_and(|toast| toast.created.elapsed() > ERROR_TOAST_LIFETIME)\n        {\n            self.toast = None;\n            if self.status_message == \"Error - see Logs\" {\n                self.status_message = \"Ready\".to_owned();\n            }\n        }",
    "error status expiration",
)

# Export current.
app = replace_once(
    app,
    "        let default_dpi = self.settings.default_dpi;\n        let validate_after_export = self.settings.validate_after_export;\n        self.launch_job(\"Exporting TIFF\"",
    "        let default_dpi = self.settings.default_dpi;\n        let force_lzw = self.settings.lzw_compression;\n        let validate_after_export = self.settings.validate_after_export;\n        self.launch_job(\"Exporting TIFF\"",
    "current export LZW setting",
)
app = replace_once(
    app,
    "            let result = export::export_face_with_progress(\n                &source,\n                &destination,\n                &project,\n                default_dpi,",
    "            let result = export::export_face_with_progress_options(\n                &source,\n                &destination,\n                &project,\n                default_dpi,\n                export::ExportOptions { force_lzw },",
    "current export options call",
)
app = replace_once(
    app,
    "                    let verified = validation::validate_export_transport(&source, &destination)?;",
    "                    let verified = validation::validate_export_transport_with_options(\n                        &source,\n                        &destination,\n                        force_lzw,\n                    )?;",
    "current export validation policy",
)

# Validate current face.
app = replace_once(
    app,
    "        let source = face.path.clone();\n        let default_dpi = self.settings.default_dpi;\n        self.launch_job(\"Validating TIFF\"",
    "        let source = face.path.clone();\n        let default_dpi = self.settings.default_dpi;\n        let force_lzw = self.settings.lzw_compression;\n        self.launch_job(\"Validating TIFF\"",
    "validation LZW setting",
)
app = replace_once(
    app,
    "            let result = validation::validate_no_adjustment_roundtrip(\n                &source,\n                &folder,\n                default_dpi,",
    "            let result = validation::validate_no_adjustment_roundtrip_with_options(\n                &source,\n                &folder,\n                default_dpi,\n                force_lzw,",
    "validation options call",
)

# Export all.
app = replace_once(
    app,
    "        let project = self.project.clone();\n        let default_dpi = self.settings.default_dpi;\n        let validate_after_export = self.settings.validate_after_export;\n        self.launch_job(\"Exporting faces\"",
    "        let project = self.project.clone();\n        let default_dpi = self.settings.default_dpi;\n        let force_lzw = self.settings.lzw_compression;\n        let validate_after_export = self.settings.validate_after_export;\n        self.launch_job(\"Exporting faces\"",
    "all export LZW setting",
)
app = replace_once(
    app,
    "                    export::export_face_with_progress(\n                        source,\n                        &destination,\n                        &project,\n                        default_dpi,",
    "                    export::export_face_with_progress_options(\n                        source,\n                        &destination,\n                        &project,\n                        default_dpi,\n                        export::ExportOptions { force_lzw },",
    "all export options call",
)
app = replace_once(
    app,
    "                        validation::validate_export_transport(source, &destination)?;",
    "                        validation::validate_export_transport_with_options(\n                            source,\n                            &destination,\n                            force_lzw,\n                        )?;",
    "all export validation policy",
)

# Snapshot export.
app = replace_once(
    app,
    "        let mut project = self.project.clone();\n        let default_dpi = self.settings.default_dpi;\n        project.adjustments = snapshot.adjustments.clone();",
    "        let mut project = self.project.clone();\n        let default_dpi = self.settings.default_dpi;\n        let force_lzw = self.settings.lzw_compression;\n        project.adjustments = snapshot.adjustments.clone();",
    "snapshot export LZW setting",
)
app = replace_once(
    app,
    "            let result = export::export_face_with_progress(\n                &source,\n                &destination,\n                &project,\n                default_dpi,",
    "            let result = export::export_face_with_progress_options(\n                &source,\n                &destination,\n                &project,\n                default_dpi,\n                export::ExportOptions { force_lzw },",
    "snapshot export options call",
)

# Snapshot groups.
app = replace_once(
    app,
    "        let base_project = self.project.clone();\n        let default_dpi = self.settings.default_dpi;\n        let snapshots = snapshot_ids",
    "        let base_project = self.project.clone();\n        let default_dpi = self.settings.default_dpi;\n        let force_lzw = self.settings.lzw_compression;\n        let snapshots = snapshot_ids",
    "snapshot group LZW setting",
)
app = replace_once(
    app,
    "                    export::export_face_with_progress(\n                        &source,\n                        &destination,\n                        &project,\n                        default_dpi,",
    "                    export::export_face_with_progress_options(\n                        &source,\n                        &destination,\n                        &project,\n                        default_dpi,\n                        export::ExportOptions { force_lzw },",
    "snapshot group export options call",
)

# Bundled Shell installation helpers next to settings persistence.
app = replace_once(
    app,
    "    fn save_settings_quietly(&mut self) {\n        if let Err(err) = self.settings.save() {\n            self.report_error(err);\n        }\n    }\n\n    fn sync_update_state(&mut self) {",
    '''    fn save_settings_quietly(&mut self) {
        if let Err(err) = self.settings.save() {
            self.report_error(err);
        }
    }

    fn bundled_shell_script(file_name: &str) -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let root = exe.parent()?;
        for folder in ["shell", "Shell"] {
            let candidate = root.join(folder).join(file_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }

    fn launch_shell_script(&mut self, file_name: &str, action: &str) {
        let Some(script) = Self::bundled_shell_script(file_name) else {
            self.report_error(
                "Shell integration package was not found next to ShadeEditor.exe. Install the Shell package separately.",
            );
            return;
        };
        match std::process::Command::new("powershell.exe")
            .arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&script)
            .spawn()
        {
            Ok(_) => self.report_info(format!(
                "Shell integration {action} started - approve the Windows administrator prompt."
            )),
            Err(err) => self.report_error(format!(
                "Cannot start Shell integration {action}: {err}"
            )),
        }
    }

    fn sync_update_state(&mut self) {''',
    "shell installer helpers",
)

# Toolbar: dismissible, short-lived error and wider progress UI.
app = regex_once(
    app,
    r"    fn ui_toolbar\(&mut self, ui: &mut egui::Ui\) \{.*?\n    fn ui_operation_progress\(&self, ui: &mut egui::Ui\) \{.*?\n    \}\n\n    fn ui_update_compact",
    '''    fn ui_toolbar(&mut self, ui: &mut egui::Ui) {
        let mut dismiss_error = false;
        ui.horizontal(|ui| {
            ui.horizontal_wrapped(|ui| {
                let enabled = self.job.is_none();
                if ui.add_enabled(enabled, egui::Button::new("New")).clicked() {
                    self.new_project();
                }
                if ui
                    .add_enabled(enabled, egui::Button::new("Open .shade"))
                    .clicked()
                {
                    self.open_project_dialog();
                }
                if ui
                    .add_enabled(enabled, egui::Button::new("Add TIFF faces"))
                    .clicked()
                {
                    self.add_faces_dialog();
                }
                ui.separator();
                if ui
                    .add_enabled(enabled && !self.faces.is_empty(), egui::Button::new("Save"))
                    .clicked()
                {
                    self.save_project(false);
                }
                if ui
                    .add_enabled(
                        enabled && !self.faces.is_empty(),
                        egui::Button::new("Save As"),
                    )
                    .clicked()
                {
                    self.save_project(true);
                }
                ui.separator();
                if ui
                    .add_enabled(
                        enabled && !self.faces.is_empty(),
                        egui::Button::new("Export face"),
                    )
                    .clicked()
                {
                    self.export_current_dialog();
                }
                if ui
                    .add_enabled(
                        enabled && !self.faces.is_empty(),
                        egui::Button::new("Export all"),
                    )
                    .clicked()
                {
                    self.export_all_dialog();
                }
                if ui
                    .add_enabled(
                        enabled && !self.faces.is_empty(),
                        egui::Button::new("Validate face"),
                    )
                    .on_hover_text("Run a no-adjustment export through the production TIFF backend, re-decode it, and compare pixels plus critical Photoshop/TIFF metadata.")
                    .clicked()
                {
                    self.validate_current_face_dialog();
                }
                ui.separator();
                if ui.button("Settings").clicked() {
                    self.show_settings = true;
                }
                if ui.button("About").clicked() {
                    self.show_about = true;
                }
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Logs").clicked() {
                    self.log_cache = self.log.read();
                    self.show_logs = true;
                }
                self.ui_update_compact(ui);
                self.ui_operation_progress(ui);
                if let Some(toast) = &self.toast {
                    ui.horizontal(|ui| {
                        dismiss_error = ui.small_button("x").on_hover_text("Dismiss error").clicked();
                        let full = toast.message.clone();
                        let mut compact = full.chars().take(56).collect::<String>();
                        if full.chars().count() > 56 {
                            compact.push('…');
                        }
                        ui.label(
                            egui::RichText::new(compact)
                                .color(egui::Color32::LIGHT_RED)
                                .small(),
                        )
                        .on_hover_text(full);
                    });
                }
            });
        });
        if dismiss_error {
            self.toast = None;
            if self.status_message == "Error - see Logs" {
                self.status_message = "Ready".to_owned();
            }
        }
    }

    fn ui_operation_progress(&self, ui: &mut egui::Ui) {
        if let Some(job) = &self.job {
            if let Ok(progress) = job.progress.lock() {
                let value = progress.fraction.unwrap_or(0.5);
                let label = progress.label.clone();
                let detail = progress.detail.clone();
                ui.vertical(|ui| {
                    ui.add(
                        egui::ProgressBar::new(value)
                            .desired_width(300.0)
                            .text(label)
                            .animate(progress.fraction.is_none()),
                    );
                    if !detail.is_empty() {
                        let mut compact = detail.chars().take(48).collect::<String>();
                        if detail.chars().count() > 48 {
                            compact.push('…');
                        }
                        ui.small(compact).on_hover_text(detail);
                    }
                });
                return;
            }
        }
        if self.render_busy.is_some() {
            ui.add(
                egui::ProgressBar::new(0.45)
                    .desired_width(240.0)
                    .text("Rendering preview")
                    .animate(true),
            );
        }
    }

    fn ui_update_compact''',
    "toolbar and progress UI",
)
app = app.replace(".desired_width(125.0)\n                        .text(\"Checking update\")", ".desired_width(190.0)\n                        .text(\"Checking update\")")
app = app.replace(".desired_width(150.0)\n                        .text(format!(\"Updating {}\", info.version))", ".desired_width(220.0)\n                        .text(format!(\"Updating {}\", info.version))")

# Settings sections: export/storage + bundled Shell controls.
app = replace_once(
    app,
    "                ui.small(\"The max dimension is used when TIFF previews are loaded. Use Rebuild previews to apply a changed value to Faces already open in this project.\");\n                changed |= ui\n                    .checkbox(\n                        &mut self.settings.validate_after_export,",
    "                ui.small(\"The max dimension is used when TIFF previews are loaded. Use Rebuild previews to apply a changed value to Faces already open in this project.\");\n                ui.separator();\n                ui.heading(\"Export & storage\");\n                changed |= ui\n                    .checkbox(\n                        &mut self.settings.lzw_compression,\n                        \"Use LZW compression for exported TIFF files\",\n                    )\n                    .changed();\n                ui.small(\"LZW is enabled by default. Disable it only when you specifically need to preserve a supported source compression mode.\");\n                changed |= ui\n                    .checkbox(\n                        &mut self.settings.validate_after_export,",
    "export storage settings section",
)
app = replace_once(
    app,
    "                }\n                ui.separator();\n                ui.heading(\"Editor layout\");",
    '''                }
                ui.separator();
                ui.heading("Windows Explorer integration");
                let shell_installer = Self::bundled_shell_script("Install-ShadeEditorShell.ps1");
                let shell_uninstaller = Self::bundled_shell_script("Uninstall-ShadeEditorShell.ps1");
                if let Some(installer) = shell_installer {
                    ui.small(format!(
                        "Bundled Shell package: {}",
                        installer.parent().unwrap_or_else(|| Path::new(".")).display()
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Install Shell integration").clicked() {
                            self.launch_shell_script("Install-ShadeEditorShell.ps1", "installation");
                        }
                        if shell_uninstaller.is_some()
                            && ui.button("Uninstall Shell integration").clicked()
                        {
                            self.launch_shell_script(
                                "Uninstall-ShadeEditorShell.ps1",
                                "removal",
                            );
                        }
                    });
                    ui.small("The installer may request administrator permission because Explorer COM/property handlers are registered machine-wide.");
                } else {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Bundled shell folder not found next to ShadeEditor.exe.",
                    );
                    ui.small("Install the Shell package separately, or place the shell folder from the build package next to ShadeEditor.exe.");
                }
                ui.separator();
                ui.heading("Editor layout");''',
    "Shell integration settings section",
)

# About window: readable shortcut groups instead of one overflowing line.
app = replace_once(
    app,
    "        egui::Window::new(\"About Shade Editor\")\n            .open(&mut open)\n            .resizable(false)\n            .show(ctx, |ui| {",
    "        egui::Window::new(\"About Shade Editor\")\n            .open(&mut open)\n            .resizable(true)\n            .default_width(520.0)\n            .show(ctx, |ui| {",
    "About window sizing",
)
app = replace_once(
    app,
    "                ui.separator();\n                ui.label(\"Shortcuts: Ctrl+S Save · Ctrl+Shift+S Save As · F Fit · 1-9 channel · S Solo · Ctrl+Enter Update Snapshot · Curve arrows nudge; Shift+Arrow uses larger steps.\");",
    '''                ui.separator();
                ui.strong("Shortcuts");
                egui::Grid::new("about-shortcuts")
                    .num_columns(2)
                    .spacing([18.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("File");
                        ui.label("Ctrl+S  Save   |   Ctrl+Shift+S  Save As");
                        ui.end_row();
                        ui.strong("View");
                        ui.label("F  Fit image");
                        ui.end_row();
                        ui.strong("Channels");
                        ui.label("1-9  Select channel   |   S  Solo channel");
                        ui.end_row();
                        ui.strong("Snapshot");
                        ui.label("Ctrl+Enter  Update active Snapshot");
                        ui.end_row();
                        ui.strong("Curve");
                        ui.label("Arrow keys  Nudge point   |   Shift+Arrow  Larger step");
                        ui.end_row();
                        ui.strong("History");
                        ui.label("Ctrl+Alt+Z  Undo   |   Ctrl+Shift+Z  Redo");
                        ui.end_row();
                    });''',
    "About shortcut grid",
)
write("src/app_main.rs", app)

# Keep the bundled installer folder/version aligned with the application build.
for rel in ["shell/Install-ShadeEditorShell.ps1", "shell/Uninstall-ShadeEditorShell.ps1"]:
    text = read(rel)
    if "$version = '0.12.0'" in text:
        text = text.replace("$version = '0.12.0'", "$version = '0.12.1'", 1)
        write(rel, text)

# Release notes entry; this is documentation only. No GitHub Release is created.
notes = read("RELEASE_NOTES.md")
entry = '''# Shade Editor v0.12.1\n\n- Moved export/storage controls into a dedicated Settings section.\n- Added persistent LZW export compression control; enabled by default for old and new settings.\n- Added Settings buttons for bundled Windows Shell install/uninstall scripts, with a clear separate-package message when the shell folder is missing.\n- Reworked About shortcuts into readable groups.\n- Error toolbar messages now auto-expire quickly and can be dismissed without leaving the error as the permanent status text.\n- Widened operation/update progress bars and moved long operation details below the main progress label.\n- Build packaging uses a `shell` folder beside ShadeEditor.exe. GitHub Release publication is disabled.\n\n'''
if not notes.startswith("# Shade Editor v0.12.1"):
    notes = entry + notes
    write("RELEASE_NOTES.md", notes)

print("Applied Shade Editor v0.12.1 settings/Shell/progress patch")
