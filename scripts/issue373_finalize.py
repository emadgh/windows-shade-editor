from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one anchor, found {count}: {old[:120]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")


# First apply the already-reviewed route-aware planning patch.
exec(Path("scripts/issue373_route_plan.py").read_text(encoding="utf-8"), {"__name__": "__main__"})

# Fix the route-recovery borrow found by Windows CI: keep an owned prior provenance.
replace_once(
    "src/conversion_recovery_route.rs",
    '''        .map(|(index, provenance)| (index, provenance))
        .collect::<Vec<_>>();
''',
    '''        .map(|(index, provenance)| (index, provenance.clone()))
        .collect::<Vec<_>>();
''',
)
replace_once(
    "src/conversion_recovery_route.rs",
    '''    if let Some((index, previous)) = matching_current.first().copied() {
        let previous_policy = batch_recipe_policy_sha256(&previous.recipe)?;
''',
    '''    if let Some((index, previous)) = matching_current.first() {
        let index = *index;
        let previous_policy = batch_recipe_policy_sha256(&previous.recipe)?;
''',
)

# Existing route ownership is per Face, so compare the full per-Face recipe in addition to
# the shared batch policy before permitting an existing TIFF to be replaced.
replace_once(
    "src/ui/conversion_plan.rs",
    '''    let output_paths = match deterministic_output_paths(app, output_folder, inspections, route) {
''',
    '''    let output_paths = match deterministic_output_paths(app, output_folder, inspections, &recipes, route) {
''',
)
replace_once(
    "src/ui/conversion_plan.rs",
    '''fn deterministic_output_paths(
    app: &ShadeApp,
    folder: &Path,
    inspections: &[ConversionFaceInspection],
    route: Option<&ConversionRouteRecord>,
) -> Result<Vec<PathBuf>, String> {
''',
    '''fn deterministic_output_paths(
    app: &ShadeApp,
    folder: &Path,
    inspections: &[ConversionFaceInspection],
    recipes: &[ConversionRecipe],
    route: Option<&ConversionRouteRecord>,
) -> Result<Vec<PathBuf>, String> {
''',
)
replace_once(
    "src/ui/conversion_plan.rs",
    '''    for inspection in inspections {
        let duplicate_stem = source_stem_occurrences(app, &inspection.source_path) > 1;
''',
    '''    for (inspection, recipe) in inspections.iter().zip(recipes) {
        let duplicate_stem = source_stem_occurrences(app, &inspection.source_path) > 1;
''',
)
replace_once(
    "src/ui/conversion_plan.rs",
    '''        if let Some(owned) = owned {
            if !paths_match(Path::new(&owned.provenance.output_path), &output) {
''',
    '''        if let Some(owned) = owned {
            if owned.provenance.recipe != *recipe {
                return Err(format!(
                    "Source Face '{}' no longer matches its saved route recipe (Source ICC/transparency or conversion settings changed). Restore the saved route or create a new route.",
                    inspection.label
                ));
            }
            if !paths_match(Path::new(&owned.provenance.output_path), &output) {
''',
)

# Restore the exact persisted target after re-verifying the external ICC/DeviceLink identity.
anchor = '''pub(crate) fn production_routes(app: &ShadeApp) -> Vec<ConversionRouteRecord> {
'''
helper = r'''pub(crate) fn restore_target_from_route(
    route: &ConversionRouteRecord,
    source_model: RuntimeColorModel,
) -> Result<ConversionTargetState, String> {
    route.validate()?;
    let recipe = route
        .baseline_recipe()
        .ok_or_else(|| "Saved conversion route has no baseline recipe.".to_owned())?;
    let (path, expected_identity) = match recipe.engine_mode {
        ConversionEngineMode::Icc => (
            recipe
                .target
                .output_profile_path
                .as_deref()
                .ok_or_else(|| "Saved ICC route has no target profile path.".to_owned())?,
            recipe
                .target
                .output_profile_identity
                .as_ref()
                .ok_or_else(|| "Saved ICC route has no target profile identity.".to_owned())?,
        ),
        ConversionEngineMode::DeviceLink => (
            recipe
                .target
                .device_link_path
                .as_deref()
                .ok_or_else(|| "Saved DeviceLink route has no profile path.".to_owned())?,
            recipe
                .target
                .device_link_identity
                .as_ref()
                .ok_or_else(|| "Saved DeviceLink route has no profile identity.".to_owned())?,
        ),
        ConversionEngineMode::CustomOptimizer => {
            return Err(
                "Saved Custom Optimizer route restore is not enabled in the unified ICC/DeviceLink UI."
                    .to_owned(),
            );
        }
    };
    let verified = verify_production_target_profile(
        Path::new(path),
        expected_identity,
        recipe.engine_mode,
        conversion_color_model(source_model),
    )?;
    if verified.output_channel_count != recipe.target.channels.len() {
        return Err(
            "Saved route target topology no longer matches the verified external profile."
                .to_owned(),
        );
    }
    let channel_names = recipe
        .target
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect::<Vec<_>>();
    validate_target_channel_names(&channel_names, verified.output_channel_count)?;
    Ok(ConversionTargetState {
        engine_mode: recipe.engine_mode,
        target_profile: Some(verified),
        target_name: recipe.target.name.clone(),
        channel_names,
        channel_names_confirmed: true,
        output_bit_depth: recipe.target.bit_depth,
        rendering_intent: recipe.rendering_intent,
        black_point_compensation: recipe.black_point_compensation,
    })
}

'''
replace_once("src/ui/conversion_plan.rs", anchor, helper + anchor)

# Wire restore into the unified window and keep explicit same-route discard confirmation in UI state.
replace_once(
    "src/ui/color_conversion.rs",
    '''    inspect_conversion_face, production_candidates, production_routes, scope_indices,
};
''',
    '''    inspect_conversion_face, production_candidates, production_routes, restore_target_from_route,
    scope_indices,
};
''',
)
replace_once(
    "src/ui/color_conversion.rs",
    '''        let mut requested_candidate_visibility: Option<bool> = None;
        let mut queue_requested = false;
''',
    '''        let mut requested_candidate_visibility: Option<bool> = None;
        let mut restore_route_requested: Option<PathBuf> = None;
        let mut queue_requested = false;
''',
)

# Auto-identify one exact route when current target policy + destination folder have one match.
replace_once(
    "src/ui/color_conversion.rs",
    '''        let routes = production_routes(self);
        let plan_preview = state.output_folder.as_deref().map(|folder| {
''',
    '''        let routes = production_routes(self);
        if state.destination_mode == UnifiedDestinationMode::AppendExisting
            && state.selected_existing.is_none()
        {
            if let (Some(folder), Ok(recipe)) = (
                state.output_folder.as_deref(),
                build_conversion_recipe(&state.target, &current_inspection, current_policy.copied()),
            ) {
                let matching = routes
                    .iter()
                    .filter(|route| {
                        route.matches_recipe_policy(&recipe).unwrap_or(false)
                            && route
                                .output_folder()
                                .to_string_lossy()
                                .eq_ignore_ascii_case(&folder.to_string_lossy())
                    })
                    .take(2)
                    .collect::<Vec<_>>();
                if let [route] = matching.as_slice() {
                    state.selected_existing = Some(route.production_project_path());
                }
            }
        }
        let plan_preview = state.output_folder.as_deref().map(|folder| {
''',
)

# Add restore/status controls to a selected saved route.
replace_once(
    "src/ui/color_conversion.rs",
    '''                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Saved route · {} committed Face(s) · policy {}",
                                                    route.converted_face_count(),
                                                    short_hash(&route.batch_recipe_policy_sha256)
                                                ))
                                                .color(egui::Color32::LIGHT_GREEN),
                                            );
                                            ui.checkbox(
''',
    '''                                            let missing_outputs = route
                                                .faces
                                                .iter()
                                                .filter(|face| !PathBuf::from(&face.provenance.output_path).exists())
                                                .count();
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Saved route · {} committed Face(s) · {} missing output(s) · policy {}",
                                                    route.converted_face_count(),
                                                    missing_outputs,
                                                    short_hash(&route.batch_recipe_policy_sha256)
                                                ))
                                                .color(if missing_outputs == 0 {
                                                    egui::Color32::LIGHT_GREEN
                                                } else {
                                                    egui::Color32::YELLOW
                                                }),
                                            );
                                            if ui.button("Restore saved route settings").clicked() {
                                                restore_route_requested = Some(route.production_project_path());
                                            }
                                            ui.checkbox(
''',
)

# Restore is an explicit action: verify target bytes first, then Source ICC/fallback and transparency.
replace_once(
    "src/ui/color_conversion.rs",
    '''        if refresh_profile_catalog {
''',
    '''        if let Some(route_path) = restore_route_requested {
            if let Some(route) = routes.iter().find(|route| {
                route
                    .production_project_path()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&route_path.to_string_lossy())
            }) {
                match restore_target_from_route(route, current_inspection.source_model) {
                    Ok(target) => match self.restore_source_bindings_from_route(route, &mut state) {
                        Ok(source_changed) => {
                            state.target = target;
                            state.output_folder = Some(route.output_folder());
                            state.destination_mode = UnifiedDestinationMode::AppendExisting;
                            state.selected_existing = Some(route.production_project_path());
                            state.allow_production_work_discard = false;
                            state.clear_profile_catalog();
                            force_candidate_refresh = true;
                            if source_changed {
                                self.report_info(
                                    "Restored saved conversion route. Source ICC bindings changed; Save the Source project before final conversion."
                                );
                            } else {
                                self.report_info("Restored and reverified saved conversion route settings.");
                            }
                        }
                        Err(error) => self.report_error(error),
                    },
                    Err(error) => self.report_error(format!(
                        "Saved conversion route requires repair before restore: {error}"
                    )),
                }
            }
        }

        if refresh_profile_catalog {
''',
)

# Add the per-Face route source-state restore helper inside ShadeApp.
anchor = '''    fn assign_production_source_profile(&mut self, index: usize, path: PathBuf) {
'''
helper = r'''    fn restore_source_bindings_from_route(
        &mut self,
        route: &windows_shade_editor::model::ConversionRouteRecord,
        state: &mut ColorConversionUiState,
    ) -> Result<bool, String> {
        let mut desired = Vec::new();
        for (index, runtime) in self.faces.iter().enumerate() {
            let Some(route_face) = route.face_for_source(&runtime.path) else {
                continue;
            };
            let source_model = runtime.preview.color_model();
            let desired_assignment = if let Some(recorded_path) = route_face.source_profile_path.as_deref() {
                let path = PathBuf::from(recorded_path);
                let verified = IccProfileRegistry.verify_identity(
                    &path,
                    &route_face.provenance.recipe.source_profile_identity,
                )?;
                if !verified.compatible_with_source_model(conversion_color_model(source_model)) {
                    return Err(format!(
                        "Saved Source ICC '{}' no longer matches {} source data for Face {}.",
                        verified.description,
                        source_model.title(),
                        index + 1
                    ));
                }
                Some(model::ProductionSourceProfileAssignment {
                    path: path.to_string_lossy().into_owned(),
                    identity: model::IccProfileIdentity {
                        description: verified.identity.description,
                        sha256: verified.identity.sha256,
                    },
                })
            } else {
                let descriptor = runtime.preview.source_descriptor().ok_or_else(|| {
                    format!("Cannot inspect Source ICC state for Face {}.", index + 1)
                })?;
                let actual = color_management::production_source_profile_identity_or_rgb_fallback_for_runtime(
                    source_model,
                    descriptor.embedded_icc,
                )?
                .ok_or_else(|| format!("Saved route requires a Source ICC for Face {}.", index + 1))?;
                let expected = &route_face.provenance.recipe.source_profile_identity;
                if !actual.sha256.eq_ignore_ascii_case(expected.sha256.trim()) {
                    return Err(format!(
                        "Embedded/fallback Source ICC for Face {} no longer matches the saved route. Relink the original Source ICC before reconversion.",
                        index + 1
                    ));
                }
                None
            };
            desired.push((
                index,
                desired_assignment,
                route_face.provenance.recipe.source_transparency_policy,
            ));
        }

        let mut changed = false;
        for (index, assignment, transparency) in desired {
            if let Some(face) = self.project.faces.get_mut(index) {
                if face.production_source_profile != assignment {
                    face.production_source_profile = assignment;
                    changed = true;
                }
            }
            match transparency {
                Some(policy) => {
                    state.transparency_policies.insert(index, policy);
                }
                None => {
                    state.transparency_policies.remove(&index);
                }
            }
        }
        if changed {
            self.mark_project_dirty();
        }
        Ok(changed)
    }

'''
replace_once("src/ui/color_conversion.rs", anchor, helper + anchor)

# The UI contract should explicitly retain route restoration and collision safety.
replace_once(
    "src/ui/mod.rs",
    '''        assert!(plan.contains("deterministic_converted_filename"));
''',
    '''        assert!(plan.contains("deterministic_converted_filename"));
        assert!(plan.contains("restore_target_from_route"));
        assert!(plan.contains("UpdateExistingRoute"));
        assert!(conversion.contains("Restore saved route settings"));
        assert!(conversion.contains("allow_production_work_discard"));
''',
)

print("issue #373 final route planning/restore patch applied")
