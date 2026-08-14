from pathlib import Path


def once(text, old, new, label):
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected 1, got {count}")
    return text.replace(old, new, 1)

# Cargo: Windows ColorSystem enumeration API.
p = Path("Cargo.toml")
t = p.read_text(encoding="utf-8")
t = once(t, '    "Win32_Storage_FileSystem",\n]', '    "Win32_Storage_FileSystem",\n    "Win32_UI_ColorSystem",\n]', "ColorSystem feature")
p.write_text(t, encoding="utf-8")

# Model: external ICC identity only; no profile payload embedding.
p = Path("src/model.rs")
t = p.read_text(encoding="utf-8")
identity = '''#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct IccProfileIdentity {
    pub description: String,
    pub sha256: String,
}

'''
t = once(t, '/// Project-owned display-only color setup. It is serialized in `.shade`, but is\n', identity + '/// Project-owned display-only color setup. It is serialized in `.shade`, but is\n', "ICC identity type")
t = once(t, '    pub assigned_profile_path: Option<String>,\n    pub rendering_intent:', '    pub assigned_profile_path: Option<String>,\n    /// Identity metadata only; ICC payloads are never embedded in `.shade`.\n    pub assigned_profile_identity: Option<IccProfileIdentity>,\n    pub rendering_intent:', "assigned identity field")
t = once(t, '    pub proof_profile_path: Option<String>,\n    pub proofing_intent:', '    pub proof_profile_path: Option<String>,\n    /// Identity metadata only; ICC payloads are never embedded in `.shade`.\n    pub proof_profile_identity: Option<IccProfileIdentity>,\n    pub proofing_intent:', "proof identity field")
t = once(t, '            assigned_profile_path: None,\n            rendering_intent:', '            assigned_profile_path: None,\n            assigned_profile_identity: None,\n            rendering_intent:', "assigned identity default")
t = once(t, '            proof_profile_path: None,\n            proofing_intent:', '            proof_profile_path: None,\n            proof_profile_identity: None,\n            proofing_intent:', "proof identity default")
p.write_text(t, encoding="utf-8")

# Settings: monitor/gamut are machine-local, not project data.
p = Path("src/settings.rs")
t = p.read_text(encoding="utf-8")
t = once(t, 'use crate::export_batch::{ConflictPolicy, DEFAULT_EXPORT_TEMPLATE, DEFAULT_FOLDER_TEMPLATE};\n', 'use crate::export_batch::{ConflictPolicy, DEFAULT_EXPORT_TEMPLATE, DEFAULT_FOLDER_TEMPLATE};\nuse crate::model::IccProfileIdentity;\n', "settings identity import")
t = once(t, '    pub lzw_compression: bool,\n    pub default_dpi:', '    pub lzw_compression: bool,\n    pub monitor_profile_path: Option<String>,\n    pub monitor_profile_identity: Option<IccProfileIdentity>,\n    pub gamut_warning: bool,\n    pub default_dpi:', "monitor settings fields")
t = once(t, '            lzw_compression: true,\n            default_dpi:', '            lzw_compression: true,\n            monitor_profile_path: None,\n            monitor_profile_identity: None,\n            gamut_warning: false,\n            default_dpi:', "monitor settings defaults")
t = once(t, '        if self.export_all_template.trim().is_empty() {\n            self.export_all_template = DEFAULT_EXPORT_TEMPLATE.to_owned();\n        }\n', '        if self.export_all_template.trim().is_empty() {\n            self.export_all_template = DEFAULT_EXPORT_TEMPLATE.to_owned();\n        }\n        if self.monitor_profile_path.as_ref().is_some_and(|path| path.trim().is_empty()) {\n            self.monitor_profile_path = None;\n            self.monitor_profile_identity = None;\n        }\n', "monitor sanitize")
t = once(t, '    fn lzw_compression_defaults_on() {\n        assert!(AppSettings::default().lzw_compression);\n    }\n', '    fn lzw_compression_defaults_on() {\n        assert!(AppSettings::default().lzw_compression);\n    }\n\n    #[test]\n    fn monitor_color_management_defaults_are_non_intrusive() {\n        let settings = AppSettings::default();\n        assert!(settings.monitor_profile_path.is_none());\n        assert!(settings.monitor_profile_identity.is_none());\n        assert!(!settings.gamut_warning);\n    }\n', "monitor defaults test")
p.write_text(t, encoding="utf-8")

# Fix lcms test API in the already-added Phase 2 backend.
p = Path("src/color_management.rs")
t = p.read_text(encoding="utf-8")
t = once(t, '        let mut source_profile = Profile::new_srgb();\n        let mut bytes = Vec::new();\n        source_profile.write_icc(&mut bytes).unwrap();\n', '        let source_profile = Profile::new_srgb();\n        let bytes = source_profile.icc().unwrap();\n', "lcms test ICC serialization")
p.write_text(t, encoding="utf-8")

# Main wiring.
p = Path("src/main.rs")
t = p.read_text(encoding="utf-8")
t = once(t, '        let color_config = PreviewColorConfig::from_project(&self.project);', '        let color_config = PreviewColorConfig::for_viewport(&self.project, &self.settings);', "viewport monitor config")
# Middle-mouse source preview remains embedded-only and portable/sRGB.
t = once(t, '''                        assigned_profile_path: None,
                        soft_proof_enabled: false,
                        proof_profile_path: None,
                        proofing_intent: PreviewRenderingIntent::RelativeColorimetric,
''', '''                        assigned_profile_path: None,
                        assigned_profile_identity: None,
                        soft_proof_enabled: false,
                        proof_profile_path: None,
                        proof_profile_identity: None,
                        proofing_intent: PreviewRenderingIntent::RelativeColorimetric,
                        monitor_profile_path: None,
                        monitor_profile_identity: None,
                        gamut_warning: false,
''', "embedded source config")

# Catalog relink by identity for project/source-proof and workstation monitor.
t = once(t, '''            Ok(profiles) => {
                self.icc_profiles = profiles;
                self.icc_profile_scan_error = None;
            }
''', '''            Ok(profiles) => {
                self.icc_profiles = profiles;
                self.icc_profile_scan_error = None;
                let project_relinked = color_management::relink_project_profiles(
                    &mut self.project,
                    &self.icc_profiles,
                );
                let monitor_relinked = color_management::relink_monitor_profile(
                    &mut self.settings,
                    &self.icc_profiles,
                );
                if project_relinked {
                    self.project_dirty = true;
                    self.invalidate_display_previews();
                }
                if monitor_relinked {
                    if let Err(err) = self.settings.save() {
                        self.log.error(&err);
                    }
                    self.invalidate_display_previews();
                }
            }
''', "catalog identity relink")

# UI local monitor state.
t = once(t, '''        let proof_path = self.project.preview_color.proof_profile_path.clone();
        let mut show_incompatible = self.icc_show_incompatible;
        let mut requested_profile: Option<Option<PathBuf>> = None;
        let mut requested_proof: Option<Option<PathBuf>> = None;
        let mut browse_requested = false;
        let mut browse_proof_requested = false;
''', '''        let proof_path = self.project.preview_color.proof_profile_path.clone();
        let monitor_path = self.settings.monitor_profile_path.clone();
        let mut gamut_warning = self.settings.gamut_warning;
        let mut show_incompatible = self.icc_show_incompatible;
        let mut requested_profile: Option<Option<PathBuf>> = None;
        let mut requested_proof: Option<Option<PathBuf>> = None;
        let mut requested_monitor: Option<Option<PathBuf>> = None;
        let mut browse_requested = false;
        let mut browse_proof_requested = false;
        let mut browse_monitor_requested = false;
''', "monitor UI locals")

# Insert monitor section before the middle-mouse note/end of window.
t = once(t, '''                if soft_proof_enabled && proof_path.is_none() && requested_proof.is_none() {
                    ui.colored_label(egui::Color32::YELLOW, "Soft Proof is enabled but no printer/RIP profile is selected.");
                }
                ui.small("Middle-mouse source preview deliberately bypasses assigned source profiles and Soft Proof and uses only the TIFF's embedded ICC.");
''', '''                if soft_proof_enabled && proof_path.is_none() && requested_proof.is_none() {
                    ui.colored_label(egui::Color32::YELLOW, "Soft Proof is enabled but no printer/RIP profile is selected.");
                }

                ui.separator();
                ui.heading("Monitor / Display ICC");
                ui.small("Workstation-local display conversion. This path is saved in application settings, not inside the .shade project.");
                let display_profiles = profiles
                    .iter()
                    .filter(|profile| profile.is_display_profile())
                    .collect::<Vec<_>>();
                let monitor_selected_text = monitor_path
                    .as_deref()
                    .and_then(|path| {
                        display_profiles
                            .iter()
                            .find(|profile| profile.path.to_string_lossy() == path)
                            .map(|profile| profile.description.clone())
                    })
                    .or_else(|| monitor_path.clone())
                    .unwrap_or_else(|| "sRGB display fallback".to_owned());
                egui::ComboBox::from_label("Installed display profile")
                    .selected_text(monitor_selected_text)
                    .width(520.0)
                    .show_ui(ui, |ui| {
                        for profile in &display_profiles {
                            let selected_now = monitor_path.as_deref()
                                == Some(profile.path.to_string_lossy().as_ref());
                            if ui
                                .selectable_label(
                                    selected_now,
                                    format!("{} · {}", profile.description, profile.filename()),
                                )
                                .clicked()
                            {
                                requested_monitor = Some(Some(profile.path.clone()));
                            }
                        }
                    });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Browse Monitor ICC...").clicked() {
                        browse_monitor_requested = true;
                    }
                    if ui
                        .add_enabled(monitor_path.is_some(), egui::Button::new("Use sRGB fallback"))
                        .clicked()
                    {
                        requested_monitor = Some(None);
                    }
                    ui.add_enabled_ui(soft_proof_enabled, |ui| {
                        ui.checkbox(&mut gamut_warning, "Gamut warning");
                    });
                });
                ui.small("Gamut warning is active only with Printer/RIP Soft Proof. Middle-mouse source preview deliberately bypasses assigned source, proof and monitor profiles and uses only the TIFF embedded ICC.");
''', "monitor UI section")

# Browse monitor.
t = once(t, '''        if browse_proof_requested {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC color profiles", &["icc", "icm"])
                .pick_file()
            {
                requested_proof = Some(Some(path));
            }
        }

        let mut changed = false;
''', '''        if browse_proof_requested {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC color profiles", &["icc", "icm"])
                .pick_file()
            {
                requested_proof = Some(Some(path));
            }
        }
        if browse_monitor_requested {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ICC color profiles", &["icc", "icm"])
                .pick_file()
            {
                requested_monitor = Some(Some(path));
            }
        }

        let mut changed = false;
        let mut display_settings_changed = false;
''', "monitor browse")

# Source clear/set identity.
t = once(t, '''                        self.project.preview_color.assigned_profile_path = None;
                        self.icc_profile_selected = None;
                        changed = true;
''', '''                        self.project.preview_color.assigned_profile_path = None;
                        self.project.preview_color.assigned_profile_identity = None;
                        self.icc_profile_selected = None;
                        changed = true;
''', "clear source identity")
t = once(t, '''                            self.project.preview_color.assigned_profile_path =
                                Some(path_text.clone());
                            self.icc_profile_selected = Some(path_text);
                            changed = true;
''', '''                            self.project.preview_color.assigned_profile_path =
                                Some(path_text.clone());
                            self.project.preview_color.assigned_profile_identity =
                                Some(profile.identity().clone());
                            self.icc_profile_selected = Some(path_text);
                            changed = true;
''', "set source identity")

# Proof clear/set identity.
t = once(t, '''                        self.project.preview_color.proof_profile_path = None;
                        self.project.preview_color.soft_proof_enabled = false;
                        changed = true;
''', '''                        self.project.preview_color.proof_profile_path = None;
                        self.project.preview_color.proof_profile_identity = None;
                        self.project.preview_color.soft_proof_enabled = false;
                        changed = true;
''', "clear proof identity")
t = once(t, '''                            self.project.preview_color.proof_profile_path = Some(path_text);
                            changed = true;
''', '''                            self.project.preview_color.proof_profile_path = Some(path_text);
                            self.project.preview_color.proof_profile_identity =
                                Some(profile.identity().clone());
                            changed = true;
''', "set proof identity")

# Monitor processing before final changed block.
t = once(t, '''        if changed {
            self.project_dirty = true;
            self.invalidate_display_previews();
        }
    }

    fn ui_settings_window''', '''        if let Some(requested) = requested_monitor {
            match requested {
                None => {
                    if self.settings.monitor_profile_path.is_some() {
                        self.settings.monitor_profile_path = None;
                        self.settings.monitor_profile_identity = None;
                        display_settings_changed = true;
                    }
                }
                Some(path) => match color_management::inspect_profile(&path) {
                    Ok(profile) if profile.is_display_profile() => {
                        let path_text = path.to_string_lossy().into_owned();
                        self.settings.monitor_profile_path = Some(path_text);
                        self.settings.monitor_profile_identity = Some(profile.identity().clone());
                        display_settings_changed = true;
                    }
                    Ok(profile) => self.report_error(format!(
                        "Cannot use '{}' as Monitor ICC: profile must be RGB Display-class, found {} / {}.",
                        profile.description,
                        profile.device_class_label(),
                        profile.color_space_label(),
                    )),
                    Err(err) => self.report_error(err),
                },
            }
        }
        if self.settings.gamut_warning != gamut_warning {
            self.settings.gamut_warning = gamut_warning;
            display_settings_changed = true;
        }

        if changed {
            self.project_dirty = true;
            self.invalidate_display_previews();
        }
        if display_settings_changed {
            if let Err(err) = self.settings.save() {
                self.log.error(&err);
            }
            self.invalidate_display_previews();
        }
    }

    fn ui_settings_window''', "monitor settings commit")

p.write_text(t, encoding="utf-8")
