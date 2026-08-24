from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one anchor, found {count}: {old[:100]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")


def replace_between(path: str, start: str, end: str, replacement: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    i = text.index(start)
    j = text.index(end, i)
    p.write_text(text[:i] + replacement + text[j:], encoding="utf-8", newline="\n")


# Unified plan carries the exact output collision policy selected by route ownership.
replace_once(
    "src/ui/conversion_plan.rs",
    "use windows_shade_editor::conversion_transaction::CapturedSourceProfile;\n",
    "use windows_shade_editor::conversion_transaction::{CapturedOutputPolicy, CapturedSourceProfile};\n",
)
replace_once(
    "src/ui/conversion_plan.rs",
    "use windows_shade_editor::model::IccProfileIdentity as ConversionIccProfileIdentity;\n",
    "use windows_shade_editor::model::{\n    ConversionRouteRecord, IccProfileIdentity as ConversionIccProfileIdentity,\n};\n",
)
replace_once(
    "src/ui/conversion_plan.rs",
    '''pub(crate) struct UnifiedConversionPlan {
    pub(crate) production_project_path: PathBuf,
    pub(crate) disposition: ProductionProjectDisposition,
    pub(crate) output_paths: Vec<PathBuf>,
    pub(crate) recipes: Vec<ConversionRecipe>,
}
''',
    '''pub(crate) struct UnifiedConversionPlan {
    pub(crate) production_project_path: PathBuf,
    pub(crate) disposition: ProductionProjectDisposition,
    pub(crate) output_policy: CapturedOutputPolicy,
    pub(crate) output_paths: Vec<PathBuf>,
    pub(crate) recipes: Vec<ConversionRecipe>,
}
''',
)

start = "pub(crate) fn build_unified_plan(\n"
end = "fn source_stem_occurrences(\n"
new_block = r'''pub(crate) fn build_unified_plan(
    app: &ShadeApp,
    scope: ConversionBatchScope,
    inspections: &[ConversionFaceInspection],
    transparency_policies: &BTreeMap<usize, SourceTransparencyPolicy>,
    target: &ConversionTargetState,
    output_folder: &Path,
    destination_mode: UnifiedDestinationMode,
    selected_existing: Option<&Path>,
    candidates: &[ProductionDestinationCandidate],
    routes: &[ConversionRouteRecord],
    allow_production_work_discard: bool,
) -> Result<UnifiedConversionPlan, Vec<String>> {
    let mut errors = Vec::new();
    if inspections.is_empty() {
        return Err(vec!["Select at least one Source Face.".to_owned()]);
    }
    for inspection in inspections {
        if !inspection.ready() {
            errors.push(format!(
                "Face {} ('{}') has blocking source preflight findings.",
                inspection.index + 1,
                inspection.label
            ));
        }
    }

    let mut recipes = Vec::with_capacity(inspections.len());
    for inspection in inspections {
        match build_conversion_recipe(
            target,
            inspection,
            transparency_policies.get(&inspection.index).copied(),
        ) {
            Ok(recipe) => recipes.push(recipe),
            Err(error) => errors.push(format!(
                "Face {} ('{}'): {error}",
                inspection.index + 1,
                inspection.label
            )),
        }
    }
    if recipes.len() != inspections.len() {
        return Err(errors);
    }

    let destination: Result<(
        PathBuf,
        ProductionProjectDisposition,
        CapturedOutputPolicy,
        Option<&ConversionRouteRecord>,
    ), String> = match destination_mode {
        UnifiedDestinationMode::CreateNew => {
            let project_path = deterministic_production_project_path(
                output_folder,
                &app.project.name,
                &target.target_name,
            );
            if project_path.exists() {
                Err(format!(
                    "Production project already exists: {}. Select its saved route or choose a new destination; Shade Editor will not infer overwrite ownership.",
                    project_path.display()
                ))
            } else {
                FrozenProductionDestination::create_new(project_path).map(|frozen| {
                    (
                        frozen.production_project_path,
                        frozen.disposition,
                        CapturedOutputPolicy::MustNotExist,
                        None,
                    )
                })
            }
        }
        UnifiedDestinationMode::AppendExisting => selected_existing
            .and_then(|path| candidates.iter().find(|candidate| paths_match(&candidate.path, path)))
            .ok_or_else(|| "Select a compatible linked Production project.".to_owned())
            .and_then(|candidate| {
                let route = routes.iter().find(|route| {
                    paths_match(
                        Path::new(&route.production_project_path),
                        &candidate.path,
                    )
                });
                if let Some(route) = route {
                    route.validate()?;
                    if !route.matches_recipe_policy(&recipes[0])? {
                        return Err(
                            "Current conversion settings differ from the selected saved route. Restore the saved route settings or create a new Production route; route mutation is never implicit."
                                .to_owned(),
                        );
                    }
                    if !paths_match(&route.output_folder(), output_folder) {
                        return Err(format!(
                            "Selected route owns destination folder {}. Restore the route destination instead of redirecting an existing route.",
                            route.output_folder().display()
                        ));
                    }
                    let compatibility = candidate.compatibility.as_ref().ok_or_else(|| {
                        "Selected Production route has no validated compatibility identity."
                            .to_owned()
                    })?;
                    let project_sha = candidate.project_sha256.as_ref().ok_or_else(|| {
                        "Selected Production route has no stable project SHA-256."
                            .to_owned()
                    })?;
                    let disposition = ProductionProjectDisposition::update_existing_route(
                        project_sha.clone(),
                        compatibility,
                        route.batch_recipe_policy_sha256.clone(),
                        allow_production_work_discard,
                    )?;
                    Ok((
                        candidate.path.clone(),
                        disposition,
                        CapturedOutputPolicy::TransactionalReplace,
                        Some(route),
                    ))
                } else {
                    FrozenProductionDestination::append_existing(candidate, &recipes[0]).map(
                        |frozen| {
                            (
                                frozen.production_project_path,
                                frozen.disposition,
                                CapturedOutputPolicy::MustNotExist,
                                None,
                            )
                        },
                    )
                }
            }),
    };
    let (production_project_path, disposition, output_policy, route) = match destination {
        Ok(destination) => destination,
        Err(error) => {
            errors.push(error);
            return Err(errors);
        }
    };

    let output_paths = match deterministic_output_paths(app, output_folder, inspections, route) {
        Ok(paths) => paths,
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    };
    if !errors.is_empty() {
        return Err(errors);
    }

    let _ = scope;
    Ok(UnifiedConversionPlan {
        production_project_path,
        disposition,
        output_policy,
        output_paths,
        recipes,
    })
}

fn deterministic_output_paths(
    app: &ShadeApp,
    folder: &Path,
    inspections: &[ConversionFaceInspection],
    route: Option<&ConversionRouteRecord>,
) -> Result<Vec<PathBuf>, String> {
    let mut reserved = BTreeSet::new();
    let mut output_paths = Vec::with_capacity(inspections.len());
    for inspection in inspections {
        let duplicate_stem = source_stem_occurrences(app, &inspection.source_path) > 1;
        let filename = deterministic_converted_filename(
            &inspection.source_path,
            duplicate_stem.then_some(inspection.index + 1),
        )
        .map_err(|error| error.to_string())?;
        let output = crate::tiff_output::canonical_destination(&folder.join(filename));
        validate_conversion_output_path(&inspection.source_path, &output)
            .map_err(|error| error.to_string())?;
        let key = path_key(&output);
        if !reserved.insert(key) {
            return Err(format!(
                "Deterministic conversion output collision: {}",
                output.display()
            ));
        }

        let owned = route.and_then(|route| route.face_for_source(&inspection.source_path));
        if let Some(owned) = owned {
            if !paths_match(Path::new(&owned.provenance.output_path), &output) {
                return Err(format!(
                    "Saved route maps Source Face '{}' to {}, not {}. Route output mapping cannot drift implicitly.",
                    inspection.label,
                    owned.provenance.output_path,
                    output.display()
                ));
            }
        }
        if output.exists() && owned.is_none() {
            return Err(format!(
                "Deterministic output already exists but is not owned by this Source Face + saved conversion route: {}. Different-route collisions fail closed.",
                output.display()
            ));
        }
        output_paths.push(output);
    }
    Ok(output_paths)
}

'''
replace_between("src/ui/conversion_plan.rs", start, end, new_block)

# Add a serialized library-model route view beside the existing candidate projection.
anchor = "pub(crate) fn default_output_folder(app: &ShadeApp) -> Option<PathBuf> {\n"
helper = r'''pub(crate) fn production_routes(app: &ShadeApp) -> Vec<ConversionRouteRecord> {
    let Ok(value) = serde_json::to_value(&app.project) else {
        return Vec::new();
    };
    let Ok(source_project) =
        serde_json::from_value::<windows_shade_editor::model::ShadeProject>(value)
    else {
        return Vec::new();
    };
    source_project
        .conversion_routes
        .into_iter()
        .filter(|route| route.validate().is_ok())
        .collect()
}

'''
replace_once("src/ui/conversion_plan.rs", anchor, helper + anchor)
# Common path equivalence is needed for route/candidate matching.
replace_once(
    "src/ui/conversion_plan.rs",
    '''fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase()
}
''',
    '''fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase()
}

fn paths_match(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}
''',
)

# Queue captures the policy frozen by the unified route plan rather than hardcoding MustNotExist.
replace_once(
    "src/ui/conversion_batch.rs",
    '''                CapturedOutputPolicy::MustNotExist,
                output_path.clone(),
''',
    '''                plan.output_policy,
                output_path.clone(),
''',
)

# UI state carries explicit destructive replacement confirmation and passes persisted routes to planning.
replace_once(
    "src/ui/color_conversion.rs",
    '''    pub(crate) selected_existing: Option<PathBuf>,
}
''',
    '''    pub(crate) selected_existing: Option<PathBuf>,
    pub(crate) allow_production_work_discard: bool,
}
''',
)
replace_once(
    "src/ui/color_conversion.rs",
    '''            destination_mode: UnifiedDestinationMode::CreateNew,
            selected_existing: None,
        }
''',
    '''            destination_mode: UnifiedDestinationMode::CreateNew,
            selected_existing: None,
            allow_production_work_discard: false,
        }
''',
)
replace_once(
    "src/ui/color_conversion.rs",
    '''    inspect_conversion_face, production_candidates, scope_indices,
};
''',
    '''    inspect_conversion_face, production_candidates, production_routes, scope_indices,
};
''',
)
replace_once(
    "src/ui/color_conversion.rs",
    '''        let candidates = production_candidates(self);
        let plan_preview = state.output_folder.as_deref().map(|folder| {
''',
    '''        let candidates = production_candidates(self);
        let routes = production_routes(self);
        let plan_preview = state.output_folder.as_deref().map(|folder| {
''',
)
replace_once(
    "src/ui/color_conversion.rs",
    '''                state.selected_existing.as_deref(),
                &candidates,
            )
''',
    '''                state.selected_existing.as_deref(),
                &candidates,
                &routes,
                state.allow_production_work_discard,
            )
''',
)
# There are two build_unified_plan call sites; patch second if still old.
p = Path("src/ui/color_conversion.rs")
text = p.read_text(encoding="utf-8")
old = '''                        state.selected_existing.as_deref(),
                        &candidates,
                    )
'''
new = '''                        state.selected_existing.as_deref(),
                        &candidates,
                        &routes,
                        state.allow_production_work_discard,
                    )
'''
if old in text:
    text = text.replace(old, new, 1)
p.write_text(text, encoding="utf-8", newline="\n")

# Expose saved-route ownership in destination UI and make destructive Production work discard explicit.
needle = '''                                if state.destination_mode == UnifiedDestinationMode::AppendExisting {
                                    for candidate in &candidates {
'''
replacement = '''                                if state.destination_mode == UnifiedDestinationMode::AppendExisting {
                                    if let Some(selected_path) = state.selected_existing.as_deref() {
                                        if let Some(route) = routes.iter().find(|route| {
                                            route.production_project_path()
                                                .to_string_lossy()
                                                .eq_ignore_ascii_case(&selected_path.to_string_lossy())
                                        }) {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Saved route · {} committed Face(s) · policy {}",
                                                    route.converted_face_count(),
                                                    short_hash(&route.batch_recipe_policy_sha256)
                                                ))
                                                .color(egui::Color32::LIGHT_GREEN),
                                            );
                                            ui.checkbox(
                                                &mut state.allow_production_work_discard,
                                                "Allow same-route replacement when Production-side adjustments/Snapshots require explicit discard confirmation",
                                            );
                                        }
                                    }
                                    for candidate in &candidates {
'''
replace_once("src/ui/color_conversion.rs", needle, replacement)
# Changing selected destination clears prior destructive confirmation.
replace_once(
    "src/ui/color_conversion.rs",
    '''                                        if response.clicked() && candidate.can_append() {
                                            state.selected_existing = Some(candidate.path.clone());
                                        }
''',
    '''                                        if response.clicked() && candidate.can_append() {
                                            if state.selected_existing.as_deref()
                                                != Some(candidate.path.as_path())
                                            {
                                                state.allow_production_work_discard = false;
                                            }
                                            state.selected_existing = Some(candidate.path.clone());
                                        }
''',
)

print("issue #373 route-aware planning patch applied")
