from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one anchor, found {count}: {old[:80]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")


# Persist route mirrors in .shade without a schema bump: serde(default) keeps v9 files backward-safe.
replace_once(
    "src/model.rs",
    "pub struct ProductionSourceProfileAssignment {\n    pub path: String,\n    pub identity: IccProfileIdentity,\n}\n",
    "pub struct ProductionSourceProfileAssignment {\n    pub path: String,\n    pub identity: IccProfileIdentity,\n}\n\n#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]\npub struct ConversionRouteRecord {\n    pub schema_version: u32,\n    pub production_project_path: String,\n    pub output_folder: String,\n    pub batch_recipe_policy_sha256: String,\n    pub faces: Vec<ConversionRouteFaceRecord>,\n}\n\n#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]\npub struct ConversionRouteFaceRecord {\n    pub provenance: ProductionProvenance,\n    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n    pub source_profile_path: Option<String>,\n}\n",
)
replace_once(
    "src/model.rs",
    "    pub production_provenance: Vec<ProductionProvenance>,\n    pub faces: Vec<FaceRef>,\n",
    "    pub production_provenance: Vec<ProductionProvenance>,\n    /// Source-side persisted mirrors of linked Production conversion routes.\n    /// Legacy schema-v9 projects deserialize this as empty; Production provenance remains\n    /// authoritative for committed output history.\n    #[serde(default)]\n    pub conversion_routes: Vec<ConversionRouteRecord>,\n    pub faces: Vec<FaceRef>,\n",
)
replace_once(
    "src/model.rs",
    "            production_provenance: Vec::new(),\n            faces: Vec::new(),\n",
    "            production_provenance: Vec::new(),\n            conversion_routes: Vec::new(),\n            faces: Vec::new(),\n",
)

# Route core owns behavior; serialized structs live in model.rs so both lib and binary model modules share them.
p = Path("src/conversion_route.rs")
text = p.read_text(encoding="utf-8")
start = text.index("#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]\npub struct ConversionRouteRecord")
end_marker = "impl ConversionRouteRecord {"
end = text.index(end_marker, start)
text = text[:start] + end_marker + text[end + len(end_marker):]
text = text.replace("use serde::{Deserialize, Serialize};\n\n", "")
text = text.replace(
    "use crate::model::ShadeProject;",
    "use crate::model::{ConversionRouteFaceRecord, ConversionRouteRecord, ShadeProject};",
)
p.write_text(text, encoding="utf-8", newline="\n")

# Keep Source link and exact route mirror in one completion operation.
replace_once(
    "src/production_project.rs",
    "use crate::model::{FaceRef, FaceStatus, ShadeProject};\n",
    "use crate::conversion_route::{build_conversion_route_record, upsert_conversion_route};\nuse crate::model::{FaceRef, FaceStatus, ShadeProject};\n",
)
insert_anchor = "fn paths_match(recorded: &str, actual: &Path) -> bool {\n"
sync_fn = r'''/// Synchronize the Source-side link and persisted conversion-route mirror from an exact,
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

'''
replace_once("src/production_project.rs", insert_anchor, sync_fn + insert_anchor)

# Completion now mirrors the exact committed route, not only its .shade path.
old = '''                    match production_project::link_source_project_to_production(
                        &mut self.project,
                        &completed.production_project_path,
                    ) {
'''
new = '''                    let source_project_path = self
                        .project_path
                        .as_deref()
                        .expect("current Source path was verified above");
                    match production_project::sync_source_project_to_production_route(
                        &mut self.project,
                        source_project_path,
                        &completed.production_project_path,
                        &completed.production_project,
                    ) {
'''
replace_once("src/ui/conversion_batch.rs", old, new)
replace_once(
    "src/ui/conversion_batch.rs",
    '"Production linkage changed the open Source project; explicit Save is required.",',
    '"Production linkage/route changed the open Source project; explicit Save is required.",',
)
replace_once(
    "src/ui/conversion_batch.rs",
    '"Could not mirror Production link in the open Source project: {error}"',
    '"Could not mirror Production conversion route in the open Source project: {error}"',
)

print("issue #373 route persistence foundation applied")
