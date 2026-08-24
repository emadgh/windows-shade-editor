from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one anchor, found {count}: {old[:140]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")


# production_project.rs is compiled into both the library and the application binary.
# Keep route-specific logic in the library boundary instead of pulling the entire route stack
# into the binary's legacy local module graph.
replace_once(
    "src/production_project.rs",
    "use crate::conversion_route::{build_conversion_route_record, upsert_conversion_route};\n",
    "",
)
replace_once(
    "src/production_project.rs",
    '''/// Synchronize the Source-side link and persisted conversion-route mirror from an exact,
/// already-committed Production project. This is called after each durable batch checkpoint so a
/// restart never has to infer route settings from filenames or UI state.
pub fn sync_source_project_to_production_route(
    source: &mut ShadeProject,
    source_project_path: &Path,
    production_project_path: &Path,
    production_project: &ShadeProject,
) -> Result<(), String> {
    let route = build_conversion_route_record(
        source,
        source_project_path,
        production_project,
        production_project_path,
    )?;
    link_source_project_to_production(source, production_project_path)?;
    upsert_conversion_route(source, route)
}

''',
    "",
)

# The unified UI owns a legacy local ShadeProject while durable conversion results use the library
# model. Cross that boundary explicitly through serde, update route/link state in the library model,
# then project the result back into the open application model.
replace_once(
    "src/ui/conversion_batch.rs",
    '''use windows_shade_editor::conversion_transaction::{
    CapturedOutputPolicy, ConversionJobCapture,
};
''',
    '''use windows_shade_editor::conversion_transaction::ConversionJobCapture;
''',
)
replace_once(
    "src/ui/conversion_batch.rs",
    '''                    match production_project::sync_source_project_to_production_route(
                        &mut self.project,
                        source_project_path,
                        &completed.production_project_path,
                        &completed.production_project,
                    ) {
''',
    '''                    match sync_open_source_project_to_production_route(
                        &mut self.project,
                        source_project_path,
                        &completed.production_project_path,
                        &completed.production_project,
                    ) {
''',
)
helper = r'''fn sync_open_source_project_to_production_route(
    source: &mut model::ShadeProject,
    source_project_path: &Path,
    production_project_path: &Path,
    production_project: &windows_shade_editor::model::ShadeProject,
) -> Result<(), String> {
    let value = serde_json::to_value(&*source)
        .map_err(|error| format!("Cannot bridge open Source project for route persistence: {error}"))?;
    let mut shared_source = serde_json::from_value::<windows_shade_editor::model::ShadeProject>(value)
        .map_err(|error| format!("Cannot decode open Source project for route persistence: {error}"))?;
    let route = windows_shade_editor::conversion_route::build_conversion_route_record(
        &shared_source,
        source_project_path,
        production_project,
        production_project_path,
    )?;
    windows_shade_editor::production_project::link_source_project_to_production(
        &mut shared_source,
        production_project_path,
    )?;
    windows_shade_editor::conversion_route::upsert_conversion_route(&mut shared_source, route)?;
    let value = serde_json::to_value(shared_source)
        .map_err(|error| format!("Cannot serialize persisted Source conversion route: {error}"))?;
    *source = serde_json::from_value::<model::ShadeProject>(value)
        .map_err(|error| format!("Cannot restore open Source project after route persistence: {error}"))?;
    Ok(())
}

'''
replace_once(
    "src/ui/conversion_batch.rs",
    "fn render_batch_queue(\n",
    helper + "fn render_batch_queue(\n",
)

# Avoid holding an immutable borrow of UI state across catalog mutation.
replace_once(
    "src/ui/color_conversion.rs",
    '''        let current_policy = state.transparency_policies.get(&self.current_face);
        let current_inspection = inspect_conversion_face(self, self.current_face, current_policy);
        ensure_installed_profile_catalog(&mut state, &current_inspection);
''',
    '''        let current_policy = state.transparency_policies.get(&self.current_face).copied();
        let current_inspection =
            inspect_conversion_face(self, self.current_face, current_policy.as_ref());
        ensure_installed_profile_catalog(&mut state, &current_inspection);
''',
)
replace_once(
    "src/ui/color_conversion.rs",
    "build_conversion_recipe(&state.target, &current_inspection, current_policy.copied()),",
    "build_conversion_recipe(&state.target, &current_inspection, current_policy),",
)

# source_descriptor() owns embedded ICC bytes; the color-management API borrows them.
replace_once(
    "src/ui/color_conversion.rs",
    '''                    source_model,
                    descriptor.embedded_icc,
                )?
''',
    '''                    source_model,
                    descriptor.embedded_icc.as_deref(),
                )?
''',
)

print("issue #373 compile fixes applied")
