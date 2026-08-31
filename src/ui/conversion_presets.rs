use eframe::egui;
use std::collections::BTreeMap;
use std::fs;

use windows_shade_editor::color_conversion::{ConversionEngineMode, ConversionRecipe};
use windows_shade_editor::conversion_preset_runtime::{
    PresetApplicationAvailability, PresetRuntimeController,
    unified_strategy_preset_availability_for_recipe,
};
use windows_shade_editor::conversion_presets::{
    PresetCompatibility, PresetOrigin, SeparationPresetDefinition,
};
use windows_shade_editor::custom_optimizer_strategy_capability::{
    CustomOptimizerStrategyAuthorityKind, CustomOptimizerStrategyCapability,
};

use super::characterization_intake::{
    CharacterizationIntakeUiState, render_characterization_intake,
};

const ENABLE_MEASURED_CHARACTERIZATION_TOOLS: bool = false;

#[derive(Clone, Default)]
pub(crate) struct ConversionPresetUiState {
    initialized: bool,
    controller: Option<PresetRuntimeController>,
    load_error: Option<String>,
    rename_buffers: BTreeMap<String, String>,
    characterization_intake: CharacterizationIntakeUiState,
}

#[derive(Clone, Debug)]
pub(crate) enum ConversionPresetUiAction {
    RetryLoad,
    Duplicate { source_id: String },
    Rename { id: String, name: String },
    Delete { id: String },
    Import,
    Export { id: String },
}

impl ConversionPresetUiState {
    pub(crate) fn ensure_loaded(&mut self) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        match PresetRuntimeController::load_default() {
            Ok(controller) => {
                self.controller = Some(controller);
                self.load_error = None;
            }
            Err(error) => {
                self.controller = None;
                self.load_error = Some(error.to_string());
            }
        }
    }

    fn retry_load(&mut self) {
        self.initialized = false;
        self.controller = None;
        self.load_error = None;
        self.ensure_loaded();
    }
}

pub(crate) fn render_conversion_preset_manager(
    ui: &mut egui::Ui,
    state: &mut ConversionPresetUiState,
    recipe: Option<&ConversionRecipe>,
    _legacy_custom_optimizer_production_authorized: bool,
    actions: &mut Vec<ConversionPresetUiAction>,
) {
    if ENABLE_MEASURED_CHARACTERIZATION_TOOLS {
        egui::CollapsingHeader::new("Measured characterization package builder")
            .id_salt("color-conversion-characterization-intake")
            .default_open(false)
            .show(ui, |ui| {
                render_characterization_intake(ui, &mut state.characterization_intake);
            });
        ui.separator();
    }

    state.ensure_loaded();

    if let Some(error) = state.load_error.as_deref() {
        ui.label(
            egui::RichText::new(format!("Preset library unavailable: {error}"))
                .color(egui::Color32::LIGHT_RED),
        );
        if ui.button("Retry preset library load").clicked() {
            actions.push(ConversionPresetUiAction::RetryLoad);
        }
        return;
    }

    let Some(controller) = state.controller.as_ref() else {
        ui.label(
            egui::RichText::new("Preset library is not loaded.")
                .color(egui::Color32::LIGHT_RED),
        );
        return;
    };

    let built_ins = built_ins_for_recipe(recipe);
    let runtime = match controller.runtime_library(&built_ins) {
        Ok(library) => library,
        Err(error) => {
            ui.label(
                egui::RichText::new(format!("Preset library validation failed: {error}"))
                    .color(egui::Color32::LIGHT_RED),
            );
            return;
        }
    };

    ui.small(format!("User preset store: {}", controller.path().display()));
    match recipe {
        Some(recipe) => {
            let availability = preset_availability(recipe);
            if let Some(reason) = availability.reason() {
                ui.label(egui::RichText::new(reason).color(egui::Color32::YELLOW));
            }
        }
        None => {
            ui.label(
                egui::RichText::new(
                    "Choose and validate a Production target to evaluate preset compatibility.",
                )
                .color(egui::Color32::YELLOW),
            );
        }
    }

    ui.horizontal_wrapped(|ui| {
        if ui.button("Import preset JSON...").clicked() {
            actions.push(ConversionPresetUiAction::Import);
        }
        let save_reason = recipe
            .map(|recipe| {
                preset_availability(recipe).reason().unwrap_or(
                    "The current profile-backed recipe has exact strategy editing authority. A separate named-capture UI is still required before saving arbitrary manual settings as a new preset.",
                )
            })
            .unwrap_or("Choose a valid Production target first.");
        ui.add_enabled(false, egui::Button::new("Save current settings as preset"))
            .on_hover_text(save_reason);
    });

    if runtime.presets.is_empty() {
        ui.small("No presets are available for the current runtime/target context.");
        return;
    }

    ui.add_space(4.0);
    for preset in &runtime.presets {
        let compatibility = recipe.map(|recipe| preset.compatibility_with_recipe(recipe));
        let availability = recipe.map(preset_availability);
        let origin = match preset.origin {
            PresetOrigin::BuiltIn => "Built-in",
            PresetOrigin::User => "User",
        };

        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(&preset.name);
                ui.small(origin);
                if let Some(compatibility) = compatibility.as_ref() {
                    let text = compatibility_label(compatibility);
                    ui.label(egui::RichText::new(text).color(
                        if *compatibility == PresetCompatibility::Compatible {
                            egui::Color32::LIGHT_GREEN
                        } else {
                            egui::Color32::YELLOW
                        },
                    ));
                } else {
                    ui.small("Compatibility: target not ready");
                }
            });
            ui.small(format!("ID: {}", preset.id));
            if let Some(notes) = preset
                .notes
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                ui.small(notes);
            }

            let apply_reason = match (compatibility.as_ref(), availability) {
                (None, _) => "Choose a valid Production target first.".to_owned(),
                (Some(value), _) if *value != PresetCompatibility::Compatible => {
                    format!("Preset cannot apply: {}", compatibility_label(value))
                }
                (_, Some(value)) if value != PresetApplicationAvailability::Available => value
                    .reason()
                    .unwrap_or("Preset application is unavailable.")
                    .to_owned(),
                _ => "This preset is compatible with the exact recipe-bound editing authority. The unified UI still applies strategy through the direct optimizer controls until the named preset action surface is connected.".to_owned(),
            };
            ui.add_enabled(false, egui::Button::new("Apply"))
                .on_hover_text(apply_reason);

            ui.horizontal_wrapped(|ui| {
                if ui.small_button("Duplicate").clicked() {
                    actions.push(ConversionPresetUiAction::Duplicate {
                        source_id: preset.id.clone(),
                    });
                }
                if ui.small_button("Export JSON...").clicked() {
                    actions.push(ConversionPresetUiAction::Export {
                        id: preset.id.clone(),
                    });
                }
            });

            if preset.origin == PresetOrigin::User {
                let rename = state
                    .rename_buffers
                    .entry(preset.id.clone())
                    .or_insert_with(|| preset.name.clone());
                ui.horizontal_wrapped(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(rename);
                    if ui
                        .add_enabled(
                            !rename.trim().is_empty() && rename.trim() != preset.name,
                            egui::Button::new("Rename"),
                        )
                        .clicked()
                    {
                        actions.push(ConversionPresetUiAction::Rename {
                            id: preset.id.clone(),
                            name: rename.trim().to_owned(),
                        });
                    }
                    if ui.small_button("Delete").clicked() {
                        actions.push(ConversionPresetUiAction::Delete {
                            id: preset.id.clone(),
                        });
                    }
                });
            } else {
                ui.small("Built-in presets are immutable; duplicate one to create a user preset.");
            }
        });
        ui.add_space(3.0);
    }
}

pub(crate) fn dispatch_conversion_preset_actions(
    state: &mut ConversionPresetUiState,
    actions: Vec<ConversionPresetUiAction>,
    recipe: Option<&ConversionRecipe>,
) -> Vec<Result<String, String>> {
    let built_ins = built_ins_for_recipe(recipe);
    let mut results = Vec::with_capacity(actions.len());

    for action in actions {
        let result = match action {
            ConversionPresetUiAction::RetryLoad => {
                state.retry_load();
                match state.load_error.as_deref() {
                    Some(error) => Err(format!("Preset library reload failed: {error}")),
                    None => Ok("Preset library reloaded.".to_owned()),
                }
            }
            ConversionPresetUiAction::Duplicate { source_id } => state
                .controller
                .as_mut()
                .ok_or_else(|| "Preset library is not loaded.".to_owned())
                .and_then(|controller| {
                    let runtime = controller
                        .runtime_library(&built_ins)
                        .map_err(|error| error.to_string())?;
                    let source = runtime
                        .get(&source_id)
                        .ok_or_else(|| format!("Unknown preset '{source_id}'."))?;
                    let (id, name) = next_duplicate_identity(&runtime, source);
                    let duplicate = controller
                        .duplicate_as_user(&source_id, id, name, &built_ins)
                        .map_err(|error| error.to_string())?;
                    state
                        .rename_buffers
                        .insert(duplicate.id.clone(), duplicate.name.clone());
                    Ok(format!("Created user preset '{}'.", duplicate.name))
                }),
            ConversionPresetUiAction::Rename { id, name } => state
                .controller
                .as_mut()
                .ok_or_else(|| "Preset library is not loaded.".to_owned())
                .and_then(|controller| {
                    controller
                        .rename_user(&id, name.clone(), &built_ins)
                        .map_err(|error| error.to_string())?;
                    state.rename_buffers.insert(id, name.clone());
                    Ok(format!("Renamed preset to '{name}'."))
                }),
            ConversionPresetUiAction::Delete { id } => state
                .controller
                .as_mut()
                .ok_or_else(|| "Preset library is not loaded.".to_owned())
                .and_then(|controller| {
                    controller
                        .delete_user(&id, &built_ins)
                        .map_err(|error| error.to_string())?;
                    state.rename_buffers.remove(&id);
                    Ok("Deleted user preset.".to_owned())
                }),
            ConversionPresetUiAction::Import => import_preset(state, &built_ins),
            ConversionPresetUiAction::Export { id } => export_preset(state, &built_ins, &id),
        };
        results.push(result);
    }
    results
}

fn preset_availability(recipe: &ConversionRecipe) -> PresetApplicationAvailability {
    let capability = strategy_capability_for_recipe(recipe);
    unified_strategy_preset_availability_for_recipe(recipe, capability.as_ref())
}

fn strategy_capability_for_recipe(
    recipe: &ConversionRecipe,
) -> Option<CustomOptimizerStrategyCapability> {
    if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
        return None;
    }
    let kind = if recipe
        .target
        .characterization_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        CustomOptimizerStrategyAuthorityKind::MeasuredProduction
    } else {
        CustomOptimizerStrategyAuthorityKind::ProfileBackedOutputIcc
    };
    CustomOptimizerStrategyCapability::for_recipe(recipe, kind).ok()
}

fn import_preset(
    state: &mut ConversionPresetUiState,
    built_ins: &[SeparationPresetDefinition],
) -> Result<String, String> {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Shade Editor conversion preset", &["json"])
        .set_title("Import conversion preset")
        .pick_file()
    else {
        return Ok("Preset import cancelled.".to_owned());
    };
    let json = fs::read_to_string(&path)
        .map_err(|error| format!("Cannot read preset {}: {error}", path.display()))?;
    let controller = state
        .controller
        .as_mut()
        .ok_or_else(|| "Preset library is not loaded.".to_owned())?;
    let preset = controller
        .import_user_json(&json, built_ins)
        .map_err(|error| error.to_string())?;
    state
        .rename_buffers
        .insert(preset.id.clone(), preset.name.clone());
    Ok(format!("Imported user preset '{}'.", preset.name))
}

fn export_preset(
    state: &mut ConversionPresetUiState,
    built_ins: &[SeparationPresetDefinition],
    id: &str,
) -> Result<String, String> {
    let controller = state
        .controller
        .as_ref()
        .ok_or_else(|| "Preset library is not loaded.".to_owned())?;
    let json = controller
        .export_preset_json(id, built_ins)
        .map_err(|error| error.to_string())?;
    let runtime = controller
        .runtime_library(built_ins)
        .map_err(|error| error.to_string())?;
    let preset = runtime
        .get(id)
        .ok_or_else(|| format!("Unknown preset '{id}'."))?;
    let Some(path) = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .set_file_name(format!("{}.json", safe_filename_component(&preset.name)))
        .set_title("Export conversion preset")
        .save_file()
    else {
        return Ok("Preset export cancelled.".to_owned());
    };
    windows_shade_editor::safe_fs::atomic_write(&path, json.as_bytes(), None)?;
    Ok(format!("Exported preset '{}': {}", preset.name, path.display()))
}

fn built_ins_for_recipe(recipe: Option<&ConversionRecipe>) -> Vec<SeparationPresetDefinition> {
    let Some(recipe) = recipe else {
        return Vec::new();
    };
    let mut presets = vec![SeparationPresetDefinition::built_in_balanced(
        &recipe.target,
        recipe.engine_mode,
    )];
    if recipe.engine_mode == ConversionEngineMode::CustomOptimizer {
        if let Some(black) = recipe.target.channels.iter().find(|channel| {
            let name = channel.name.trim();
            name.eq_ignore_ascii_case("black") || name.eq_ignore_ascii_case("k")
        }) {
            if let Ok(preset) = SeparationPresetDefinition::built_in_black_focused(
                &recipe.target,
                recipe.engine_mode,
                &black.name,
            ) {
                presets.push(preset);
            }
        }
    }
    presets
}

fn next_duplicate_identity(
    runtime: &windows_shade_editor::conversion_preset_library::SeparationPresetLibrary,
    source: &SeparationPresetDefinition,
) -> (String, String) {
    let base = safe_filename_component(&source.name).to_ascii_lowercase();
    let base = if base.is_empty() {
        "preset"
    } else {
        base.as_str()
    };
    for number in 1usize.. {
        let id = format!("user:{base}-{number}");
        if runtime.get(&id).is_none() {
            let name = if number == 1 {
                format!("{} copy", source.name)
            } else {
                format!("{} copy {number}", source.name)
            };
            return (id, name);
        }
    }
    unreachable!("unbounded preset duplicate id search")
}

fn compatibility_label(value: &PresetCompatibility) -> &'static str {
    match value {
        PresetCompatibility::Compatible => "Compatible",
        PresetCompatibility::EngineModeMismatch => "Unavailable: engine mismatch",
        PresetCompatibility::TransformIdentityMismatch => "Stale: transform identity changed",
        PresetCompatibility::CharacterizationMismatch => "Stale: characterization changed",
        PresetCompatibility::ChannelTopologyMismatch => "Stale: channel topology changed",
        PresetCompatibility::BitDepthMismatch => "Stale: bit depth changed",
        PresetCompatibility::ChannelLimitMismatch => "Stale: channel limits changed",
        PresetCompatibility::TargetInkLimitMismatch => "Stale: total-ink limit changed",
    }
}

fn safe_filename_component(value: &str) -> String {
    let cleaned = value
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let cleaned = cleaned.trim_matches('-');
    if cleaned.is_empty() {
        "conversion-preset".to_owned()
    } else {
        cleaned.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_shade_editor::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use windows_shade_editor::custom_optimizer_config::CustomOptimizerSolverConfig;
    use windows_shade_editor::model::IccProfileIdentity;

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn profile_recipe() -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: hash('a'),
            },
            target: ConversionTargetDefinition {
                name: "Output-backed".to_owned(),
                channels: vec![TargetChannelDefinition {
                    name: "Black".to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: None,
                }],
                bit_depth: 16,
                output_profile_identity: Some(IccProfileIdentity {
                    description: "Output".to_owned(),
                    sha256: hash('b'),
                }),
                output_profile_path: Some(r"C:\Color\Output.icc".to_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: None,
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
        }
    }

    #[test]
    fn compatibility_reasons_are_explicit_not_silent() {
        assert_eq!(
            compatibility_label(&PresetCompatibility::TransformIdentityMismatch),
            "Stale: transform identity changed"
        );
        assert_eq!(
            compatibility_label(&PresetCompatibility::ChannelTopologyMismatch),
            "Stale: channel topology changed"
        );
    }

    #[test]
    fn duplicate_ids_are_stable_user_ids() {
        let runtime = windows_shade_editor::conversion_preset_library::SeparationPresetLibrary::new();
        let target = windows_shade_editor::color_conversion::ConversionTargetDefinition {
            name: "Target".to_owned(),
            channels: vec![windows_shade_editor::color_conversion::TargetChannelDefinition {
                name: "Black".to_owned(),
                display_rgb: None,
                solidity: 1.0,
                max_coverage: None,
            }],
            bit_depth: 16,
            output_profile_identity: None,
            output_profile_path: None,
            device_link_identity: None,
            device_link_path: None,
            characterization_id: Some("measurement".to_owned()),
            total_ink_limit: None,
        };
        let source = SeparationPresetDefinition::built_in_black_focused(
            &target,
            ConversionEngineMode::CustomOptimizer,
            "Black",
        )
        .unwrap();
        let (id, name) = next_duplicate_identity(&runtime, &source);
        assert_eq!(id, "user:black-focused-1");
        assert_eq!(name, "Black-focused copy");
    }

    #[test]
    fn profile_backed_recipe_gets_typed_preset_editing_capability() {
        let recipe = profile_recipe();
        assert_eq!(
            preset_availability(&recipe),
            PresetApplicationAvailability::Available
        );
    }

    #[test]
    fn management_surface_uses_typed_recipe_capability_not_ui_boolean() {
        let source = include_str!("conversion_presets.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("unified_strategy_preset_availability_for_recipe"));
        assert!(runtime.contains("CustomOptimizerStrategyCapability::for_recipe"));
        assert!(!runtime.contains("unified_strategy_preset_availability("));
    }

    #[test]
    fn measured_characterization_tools_remain_compiled_but_deferred_from_ui() {
        assert!(!ENABLE_MEASURED_CHARACTERIZATION_TOOLS);
        let source = include_str!("conversion_presets.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("if ENABLE_MEASURED_CHARACTERIZATION_TOOLS"));
        assert!(runtime.contains("Measured characterization package builder"));
        assert!(runtime.contains("render_characterization_intake"));
        assert!(!runtime.contains("egui::Window::new"));
    }
}
