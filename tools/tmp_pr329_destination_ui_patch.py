from pathlib import Path


def replace_one(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one target, found {count}")
    return text.replace(old, new)

ui_path = Path("work/src/ui/color_conversion.rs")
ui = ui_path.read_text(encoding="utf-8")

ui = replace_one(
    ui,
    "use windows_shade_editor::model::IccProfileIdentity as ConversionIccProfileIdentity;\nuse windows_shade_editor::source_transparency::SourceTransparencyPolicy;",
    "use windows_shade_editor::model::IccProfileIdentity as ConversionIccProfileIdentity;\nuse windows_shade_editor::production_destination::{\n    ProductionDestinationCandidate, inspect_linked_production_destinations,\n};\nuse windows_shade_editor::production_destination_selection::FrozenProductionDestination;\nuse windows_shade_editor::production_project_disposition::ProductionProjectDisposition;\nuse windows_shade_editor::source_transparency::SourceTransparencyPolicy;",
    "imports",
)

ui = replace_one(
    ui,
    """#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ConversionStage {
    #[default]
    SourcePreflight,
    TargetSetup,
}

pub(crate) struct ColorConversionUiState {""",
    """#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ConversionStage {
    #[default]
    SourcePreflight,
    TargetSetup,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ProductionDestinationMode {
    #[default]
    CreateNew,
    AppendExisting,
}

pub(crate) struct ColorConversionUiState {""",
    "destination mode enum",
)

ui = replace_one(
    ui,
    """    collision_policy: OutputCollisionPolicy,
    rendering_intent: ConversionRenderingIntent,
    black_point_compensation: bool,
    source_transparency_policy: Option<SourceTransparencyPolicy>,
}""",
    """    collision_policy: OutputCollisionPolicy,
    destination_mode: ProductionDestinationMode,
    selected_existing: Option<ProductionDestinationCandidate>,
    destination_error: Option<String>,
    rendering_intent: ConversionRenderingIntent,
    black_point_compensation: bool,
    source_transparency_policy: Option<SourceTransparencyPolicy>,
}""",
    "state fields",
)

ui = replace_one(
    ui,
    """            collision_policy: OutputCollisionPolicy::Versioned,
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: true,""",
    """            collision_policy: OutputCollisionPolicy::Versioned,
            destination_mode: ProductionDestinationMode::CreateNew,
            selected_existing: None,
            destination_error: None,
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: true,""",
    "state defaults",
)

ui = replace_one(
    ui,
    """struct TargetSetupReview {
    recipe: ConversionRecipe,
    recipe_sha256: String,
    effective_output_path: PathBuf,
    production_project_path: PathBuf,
}""",
    """struct TargetSetupReview {
    recipe: ConversionRecipe,
    recipe_sha256: String,
    effective_output_path: PathBuf,
    production_project_path: PathBuf,
    production_project_disposition: ProductionProjectDisposition,
}""",
    "review disposition",
)

ui = replace_one(
    ui,
    """        let mut start_conversion = false;
        let queue_rows = self""",
    """        let mut start_conversion = false;
        let production_candidates = self
            .project_path
            .as_deref()
            .map(|source_project_path| {
                inspect_linked_production_destinations(&self.project, source_project_path)
            })
            .unwrap_or_default();
        let queue_rows = self""",
    "candidate discovery",
)

ui = replace_one(
    ui,
    """                        render_target_setup(
                            ui,
                            state,
                            &source,
                            &mut select_target_profile,""",
    """                        render_target_setup(
                            ui,
                            state,
                            &source,
                            &production_candidates,
                            &mut select_target_profile,""",
    "pass candidates",
)

ui = replace_one(
    ui,
    """fn render_target_setup(
    ui: &mut egui::Ui,
    state: &mut ColorConversionUiState,
    source: &CurrentConversionSource,
    select_target_profile: &mut bool,""",
    """fn render_target_setup(
    ui: &mut egui::Ui,
    state: &mut ColorConversionUiState,
    source: &CurrentConversionSource,
    production_candidates: &[ProductionDestinationCandidate],
    select_target_profile: &mut bool,""",
    "render signature",
)

old_destination = """    ui.separator();
    ui.strong("Production destination");
    ui.horizontal_wrapped(|ui| {
        if ui.button("Select TIFF output...").clicked() {
            *select_output_path = true;
        }
        ui.label(
            state
                .output_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "No destination selected".to_owned()),
        );
    });
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(
            &mut state.collision_policy,
            OutputCollisionPolicy::Versioned,
            "Create versioned output (safe default)",
        );
        ui.radio_value(
            &mut state.collision_policy,
            OutputCollisionPolicy::TransactionalReplace,
            "Explicit transactional replacement",
        );
    });
"""
new_destination = """    ui.separator();
    ui.strong("Production destination");
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(
            &mut state.destination_mode,
            ProductionDestinationMode::CreateNew,
            "Create new Production project",
        );
        ui.radio_value(
            &mut state.destination_mode,
            ProductionDestinationMode::AppendExisting,
            "Add to compatible Production project",
        );
    });

    if state.destination_mode == ProductionDestinationMode::AppendExisting {
        state.collision_policy = OutputCollisionPolicy::Versioned;
        if production_candidates.is_empty() {
            ui.label(
                egui::RichText::new("No linked Production projects are recorded for this Source project.")
                    .color(egui::Color32::YELLOW),
            );
        }
        for candidate in production_candidates {
            let selected = state.selected_existing.as_ref().is_some_and(|current| {
                current
                    .path
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&candidate.path.to_string_lossy())
                    && current.project_sha256 == candidate.project_sha256
            });
            let title = format!(
                "{} · {} Face(s) · {}",
                candidate
                    .project_name
                    .as_deref()
                    .unwrap_or("Production project"),
                candidate.face_count.unwrap_or(0),
                candidate.path.display()
            );
            let response = ui.add_enabled(
                candidate.can_append(),
                egui::Button::selectable(selected, title),
            );
            let response = if let Some(diagnostic) = candidate.diagnostic.as_deref() {
                response.on_hover_text(diagnostic)
            } else {
                response
            };
            if response.clicked() {
                match seed_state_from_existing_destination(state, source, candidate) {
                    Ok(()) => state.destination_error = None,
                    Err(error) => {
                        state.selected_existing = None;
                        state.destination_error = Some(error);
                    }
                }
            }
            if !candidate.can_append() {
                if let Some(diagnostic) = candidate.diagnostic.as_deref() {
                    ui.small(
                        egui::RichText::new(diagnostic).color(egui::Color32::LIGHT_RED),
                    );
                }
            }
        }
        ui.small(
            "Append Existing always creates a new/versioned TIFF. Replacing a prior Production Face is a separate explicit re-conversion workflow.",
        );
    } else {
        state.selected_existing = None;
    }
    if let Some(error) = state.destination_error.as_deref() {
        ui.label(egui::RichText::new(error).color(egui::Color32::LIGHT_RED));
    }

    ui.horizontal_wrapped(|ui| {
        if ui.button("Select TIFF output...").clicked() {
            *select_output_path = true;
        }
        ui.label(
            state
                .output_path
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "No destination selected".to_owned()),
        );
    });
    if state.destination_mode == ProductionDestinationMode::CreateNew {
        ui.horizontal_wrapped(|ui| {
            ui.radio_value(
                &mut state.collision_policy,
                OutputCollisionPolicy::Versioned,
                "Create versioned output (safe default)",
            );
            ui.radio_value(
                &mut state.collision_policy,
                OutputCollisionPolicy::TransactionalReplace,
                "Explicit transactional replacement",
            );
        });
    }
"""
ui = replace_one(ui, old_destination, new_destination, "destination UI")

seed_helper = r'''
fn seed_state_from_existing_destination(
    state: &mut ColorConversionUiState,
    source: &CurrentConversionSource,
    candidate: &ProductionDestinationCandidate,
) -> Result<(), String> {
    let recipe = candidate
        .baseline_recipe
        .as_ref()
        .ok_or_else(|| "Selected Production project has no validated baseline recipe.".to_owned())?
        .clone();
    let (profile_path, profile_identity) = match recipe.engine_mode {
        ConversionEngineMode::Icc => (
            recipe.target.output_profile_path.as_deref(),
            recipe.target.output_profile_identity.as_ref(),
        ),
        ConversionEngineMode::DeviceLink => (
            recipe.target.device_link_path.as_deref(),
            recipe.target.device_link_identity.as_ref(),
        ),
        ConversionEngineMode::CustomOptimizer => {
            return Err(
                "Custom Optimizer Production destinations require the dedicated characterized-target UI."
                    .to_owned(),
            );
        }
    };
    let profile_path = profile_path.ok_or_else(|| {
        "Selected Production recipe does not contain its external target profile path.".to_owned()
    })?;
    let profile_identity = profile_identity.ok_or_else(|| {
        "Selected Production recipe does not contain its target profile identity.".to_owned()
    })?;
    let verified = verify_production_target_profile(
        Path::new(profile_path),
        profile_identity,
        recipe.engine_mode,
        conversion_color_model(source.source_model),
    )?;

    state.engine_mode = recipe.engine_mode;
    state.accept_target_profile(verified, &source.source_path);
    state.target_name = recipe.target.name.clone();
    state.channel_names = recipe
        .target
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect();
    state.channel_names_confirmed = true;
    state.output_bit_depth = recipe.target.bit_depth;
    state.rendering_intent = recipe.rendering_intent;
    state.black_point_compensation = recipe.black_point_compensation;
    state.collision_policy = OutputCollisionPolicy::Versioned;
    state.selected_existing = Some(candidate.clone());
    Ok(())
}

'''
ui = replace_one(
    ui,
    "fn build_target_setup_review(\n",
    seed_helper + "fn build_target_setup_review(\n",
    "seed helper",
)

ui = replace_one(
    ui,
    """    let production_project_path = effective_output_path.with_extension("shade");
    if state.collision_policy == OutputCollisionPolicy::Versioned
        && production_project_path.exists()
    {
        errors.push(format!(
            "Production project already exists: {}. Select another TIFF name or explicitly choose transactional replacement.",
            production_project_path.display()
        ));
    }

    let profile_path = inspected.path.to_string_lossy().into_owned();""",
    """    let create_new_project_path = effective_output_path.with_extension("shade");
    if state.destination_mode == ProductionDestinationMode::CreateNew
        && state.collision_policy == OutputCollisionPolicy::Versioned
        && create_new_project_path.exists()
    {
        errors.push(format!(
            "Production project already exists: {}. Select another TIFF name or explicitly choose transactional replacement.",
            create_new_project_path.display()
        ));
    }

    let profile_path = inspected.path.to_string_lossy().into_owned();""",
    "defer project destination",
)

ui = replace_one(
    ui,
    """    if let Err(recipe_errors) = recipe.validate() {
        errors.extend(recipe_errors);
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let recipe_sha256 = recipe_sha256(&recipe).map_err(|err| vec![err])?;
    Ok(TargetSetupReview {
        recipe,
        recipe_sha256,
        effective_output_path,
        production_project_path,
    })""",
    """    if let Err(recipe_errors) = recipe.validate() {
        errors.extend(recipe_errors);
    }
    let frozen_destination = match state.destination_mode {
        ProductionDestinationMode::CreateNew => {
            FrozenProductionDestination::create_new(create_new_project_path)
        }
        ProductionDestinationMode::AppendExisting => state
            .selected_existing
            .as_ref()
            .ok_or_else(|| "Select a compatible linked Production project.".to_owned())
            .and_then(|candidate| FrozenProductionDestination::append_existing(candidate, &recipe)),
    };
    let frozen_destination = match frozen_destination {
        Ok(destination) => Some(destination),
        Err(error) => {
            errors.push(error);
            None
        }
    };
    if !errors.is_empty() {
        return Err(errors);
    }
    let frozen_destination = frozen_destination.expect("destination validated when errors are empty");
    let recipe_sha256 = recipe_sha256(&recipe).map_err(|err| vec![err])?;
    Ok(TargetSetupReview {
        recipe,
        recipe_sha256,
        effective_output_path,
        production_project_path: frozen_destination.production_project_path,
        production_project_disposition: frozen_destination.disposition,
    })""",
    "freeze destination in review",
)

ui = replace_one(
    ui,
    """        let production_project_path = review.production_project_path;
        let target_name = review.recipe.target.name.clone();""",
    """        let production_project_path = review.production_project_path;
        let production_project_disposition = review.production_project_disposition;
        let target_name = review.recipe.target.name.clone();""",
    "capture disposition",
)

ui = replace_one(
    ui,
    """            JobResult::ConversionCapture {
                result,
                default_dpi,
            }""",
    """            JobResult::ConversionCapture {
                result,
                production_project_disposition,
                default_dpi,
            }""",
    "emit disposition",
)

ui_path.write_text(ui, encoding="utf-8", newline="\n")

main_path = Path("work/src/main.rs")
main = main_path.read_text(encoding="utf-8")
main = replace_one(
    main,
    """    ConversionCapture {
        result: Result<windows_shade_editor::conversion_transaction::ConversionJobCapture, String>,
        default_dpi: f64,
    },""",
    """    ConversionCapture {
        result: Result<windows_shade_editor::conversion_transaction::ConversionJobCapture, String>,
        production_project_disposition:
            windows_shade_editor::production_project_disposition::ProductionProjectDisposition,
        default_dpi: f64,
    },""",
    "JobResult disposition",
)
main = replace_one(
    main,
    """            JobResult::ConversionCapture {
                result,
                default_dpi,
            } => match result {
                Ok(capture) => match self.conversion_queue.enqueue(capture, default_dpi) {""",
    """            JobResult::ConversionCapture {
                result,
                production_project_disposition,
                default_dpi,
            } => match result {
                Ok(capture) => match self
                    .conversion_queue
                    .enqueue_with_production_project_disposition(
                        capture,
                        production_project_disposition,
                        default_dpi,
                    )
                {""",
    "enqueue disposition",
)
main_path.write_text(main, encoding="utf-8", newline="\n")
