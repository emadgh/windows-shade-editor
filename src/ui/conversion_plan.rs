use crate::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use windows_shade_editor::color_conversion::{
    CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
    ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
    TargetChannelDefinition,
};
use windows_shade_editor::conversion_batch::ConversionBatchScope;
use windows_shade_editor::conversion_output::{
    deterministic_converted_filename, validate_conversion_output_path,
};
use windows_shade_editor::conversion_preflight::{
    ConversionPreflightReport, SourceProfileState,
    build_conversion_preflight_for_source_with_policy,
};
use windows_shade_editor::conversion_transaction::CapturedSourceProfile;
use windows_shade_editor::conversion_workflow::{
    ConversionSourceState, conversion_save_gate,
};
use windows_shade_editor::design_source::{
    DesignSourceColorModel, SourceImageFormat, TransparencyState,
};
use windows_shade_editor::icc_profile_registry::IccProfileRegistry;
use windows_shade_editor::model::IccProfileIdentity as ConversionIccProfileIdentity;
use windows_shade_editor::production_destination::{
    ProductionDestinationCandidate, inspect_linked_production_destinations,
};
use windows_shade_editor::production_destination_selection::FrozenProductionDestination;
use windows_shade_editor::production_profile_catalog::verify_production_profile_candidate;
use windows_shade_editor::production_project_disposition::ProductionProjectDisposition;
use windows_shade_editor::production_target::{
    ProductionTargetProfileInspection, validate_target_channel_names,
    verify_production_target_profile,
};
use windows_shade_editor::source_transparency::SourceTransparencyPolicy;
use windows_shade_editor::tiff_io::ColorModel as ConversionColorModel;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum UnifiedDestinationMode {
    #[default]
    CreateNew,
    AppendExisting,
}

#[derive(Clone, Debug)]
pub(crate) struct ConversionTargetState {
    pub(crate) engine_mode: ConversionEngineMode,
    pub(crate) target_profile: Option<ProductionTargetProfileInspection>,
    pub(crate) target_name: String,
    pub(crate) channel_names: Vec<String>,
    pub(crate) channel_names_confirmed: bool,
    pub(crate) output_bit_depth: u8,
    pub(crate) rendering_intent: ConversionRenderingIntent,
    pub(crate) black_point_compensation: bool,
}

impl Default for ConversionTargetState {
    fn default() -> Self {
        Self {
            engine_mode: ConversionEngineMode::Icc,
            target_profile: None,
            target_name: String::new(),
            channel_names: Vec::new(),
            channel_names_confirmed: false,
            output_bit_depth: 16,
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: true,
        }
    }
}

impl ConversionTargetState {
    pub(crate) fn clear_profile(&mut self) {
        self.target_profile = None;
        self.target_name.clear();
        self.channel_names.clear();
        self.channel_names_confirmed = false;
        self.black_point_compensation = self.engine_mode == ConversionEngineMode::Icc;
    }

    pub(crate) fn accept_profile(&mut self, profile: ProductionTargetProfileInspection) {
        self.target_name = profile.identity.description.clone();
        self.channel_names = profile.channel_names.clone();
        self.channel_names_confirmed = profile.channel_names_authoritative;
        self.target_profile = Some(profile);
    }
}

#[derive(Clone)]
pub(crate) struct ConversionFaceInspection {
    pub(crate) index: usize,
    pub(crate) label: String,
    pub(crate) source_path: PathBuf,
    pub(crate) source_model: RuntimeColorModel,
    pub(crate) source_format: SourceImageFormat,
    pub(crate) bit_depth: u8,
    pub(crate) channel_count: usize,
    pub(crate) transparency: TransparencyState,
    pub(crate) profile_identity: Option<ConversionIccProfileIdentity>,
    pub(crate) captured_profile: CapturedSourceProfile,
    pub(crate) profile_label: String,
    pub(crate) execution_supported: bool,
    pub(crate) report: ConversionPreflightReport,
    pub(crate) error: Option<String>,
}

impl ConversionFaceInspection {
    pub(crate) fn ready(&self) -> bool {
        self.error.is_none() && self.execution_supported && self.report.can_convert()
    }
}

#[derive(Clone)]
pub(crate) struct UnifiedConversionPlan {
    pub(crate) production_project_path: PathBuf,
    pub(crate) disposition: ProductionProjectDisposition,
    pub(crate) output_paths: Vec<PathBuf>,
    pub(crate) recipes: Vec<ConversionRecipe>,
}

pub(crate) fn scope_indices(
    scope: ConversionBatchScope,
    current_face: usize,
    source_face_count: usize,
    selected: &BTreeSet<usize>,
) -> Vec<usize> {
    match scope {
        ConversionBatchScope::CurrentFace => (current_face < source_face_count)
            .then_some(current_face)
            .into_iter()
            .collect(),
        ConversionBatchScope::SelectedFaces => selected
            .iter()
            .copied()
            .filter(|index| *index < source_face_count)
            .collect(),
        ConversionBatchScope::AllFaces => (0..source_face_count).collect(),
    }
}

pub(crate) fn inspect_conversion_face(
    app: &ShadeApp,
    index: usize,
    transparency_policy: Option<&SourceTransparencyPolicy>,
) -> ConversionFaceInspection {
    let label = app
        .project
        .faces
        .get(index)
        .map(|face| face.label.clone())
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| format!("Face {}", index + 1));
    let Some(runtime) = app.faces.get(index) else {
        return unavailable_face(index, label, PathBuf::new(), "Runtime Face is unavailable.");
    };
    if !runtime.available {
        return unavailable_face(
            index,
            label,
            runtime.path.clone(),
            "Source Face file is missing or unreadable. Relink it before conversion.",
        );
    }
    let Some(owned_descriptor) = runtime.preview.source_descriptor() else {
        return unavailable_face(
            index,
            label,
            runtime.path.clone(),
            "Source descriptor is unavailable for production preflight.",
        );
    };
    let descriptor = owned_descriptor.as_borrowed();
    let source_model = runtime.preview.color_model();
    let save_gate = conversion_save_gate(ConversionSourceState {
        has_faces: !app.faces.is_empty(),
        has_saved_project_path: app.project_path.is_some(),
        has_unsaved_changes: app.project_dirty,
    });
    let (profile_state, captured_profile, profile_label) = source_profile_state(
        &descriptor,
        source_model,
        app.project.faces.get(index),
    );
    let profile_identity = profile_state.identity().cloned();
    let report = build_conversion_preflight_for_source_with_policy(
        &descriptor,
        profile_state,
        save_gate,
        transparency_policy,
    );

    ConversionFaceInspection {
        index,
        label,
        source_path: runtime.path.clone(),
        source_model,
        source_format: descriptor.format,
        bit_depth: descriptor.bit_depth,
        channel_count: descriptor.channel_count,
        transparency: descriptor.transparency,
        profile_identity,
        captured_profile,
        profile_label,
        execution_supported: execution_supported(descriptor.format, descriptor.color_model),
        report,
        error: None,
    }
}

fn unavailable_face(
    index: usize,
    label: String,
    source_path: PathBuf,
    message: &str,
) -> ConversionFaceInspection {
    ConversionFaceInspection {
        index,
        label,
        source_path,
        source_model: RuntimeColorModel::Other,
        source_format: SourceImageFormat::Tiff,
        bit_depth: 0,
        channel_count: 0,
        transparency: TransparencyState::None,
        profile_identity: None,
        captured_profile: CapturedSourceProfile::Embedded,
        profile_label: "Unavailable".to_owned(),
        execution_supported: false,
        report: ConversionPreflightReport::default(),
        error: Some(message.to_owned()),
    }
}

fn source_profile_state(
    descriptor: &windows_shade_editor::design_source::DesignSourceDescriptor<'_>,
    source_model: RuntimeColorModel,
    face: Option<&model::FaceRef>,
) -> (SourceProfileState, CapturedSourceProfile, String) {
    if let Some(assignment) = face.and_then(|face| face.production_source_profile.as_ref()) {
        let path = PathBuf::from(&assignment.path);
        let expected = ConversionIccProfileIdentity {
            description: assignment.identity.description.clone(),
            sha256: assignment.identity.sha256.clone(),
        };
        return match IccProfileRegistry.verify_identity(&path, &expected) {
            Ok(profile)
                if profile.compatible_with_source_model(conversion_color_model(source_model)) =>
            {
                (
                    SourceProfileState::Assigned(profile.identity.clone()),
                    CapturedSourceProfile::External { path },
                    format!("Assigned: {}", profile.description),
                )
            }
            Ok(profile) => (
                SourceProfileState::Invalid(format!(
                    "Assigned production Source ICC '{}' declares {} but Face is {}.",
                    profile.description,
                    profile.color_space_label(),
                    source_model.title(),
                )),
                CapturedSourceProfile::External { path },
                format!("Invalid assigned ICC: {}", assignment.identity.description),
            ),
            Err(error) => (
                SourceProfileState::Invalid(error),
                CapturedSourceProfile::External { path },
                format!("Invalid assigned ICC: {}", assignment.identity.description),
            ),
        };
    }

    match color_management::production_source_profile_identity_or_rgb_fallback_for_runtime(
        source_model,
        descriptor.embedded_icc,
    ) {
        Ok(Some(identity)) => {
            let identity = ConversionIccProfileIdentity {
                description: identity.description,
                sha256: identity.sha256,
            };
            let label = if windows_shade_editor::source_profile_fallback::is_srgb_fallback_identity(
                &identity,
            ) {
                format!("No Source ICC · fallback: {}", identity.description)
            } else {
                format!("Embedded: {}", identity.description)
            };
            (
                SourceProfileState::Embedded(identity),
                CapturedSourceProfile::Embedded,
                label,
            )
        }
        Ok(None) => (
            SourceProfileState::Missing,
            CapturedSourceProfile::Embedded,
            "Missing production Source ICC".to_owned(),
        ),
        Err(error) => (
            SourceProfileState::Invalid(error),
            CapturedSourceProfile::Embedded,
            "Invalid embedded production Source ICC".to_owned(),
        ),
    }
}

pub(crate) fn build_conversion_recipe(
    target: &ConversionTargetState,
    inspection: &ConversionFaceInspection,
    transparency_policy: Option<SourceTransparencyPolicy>,
) -> Result<ConversionRecipe, String> {
    let stored = target
        .target_profile
        .as_ref()
        .ok_or_else(|| "Select a production Output ICC or DeviceLink.".to_owned())?;
    verify_production_profile_candidate(
        IccProfileRegistry,
        &stored.path,
        &stored.identity,
        target.engine_mode,
        conversion_color_model(inspection.source_model),
    )?;
    let verified = verify_production_target_profile(
        &stored.path,
        &stored.identity,
        target.engine_mode,
        conversion_color_model(inspection.source_model),
    )?;
    validate_target_channel_names(&target.channel_names, verified.output_channel_count)?;
    if !verified.channel_names_authoritative && !target.channel_names_confirmed {
        return Err("Confirm the real production channel order.".to_owned());
    }
    if target.target_name.trim().is_empty() {
        return Err("Target name cannot be empty.".to_owned());
    }
    if !matches!(target.output_bit_depth, 8 | 16) {
        return Err("Output bit depth must be 8 or 16.".to_owned());
    }
    let source_profile_identity = inspection
        .profile_identity
        .clone()
        .ok_or_else(|| "Source ICC identity is not ready.".to_owned())?;
    let profile_path = verified.path.to_string_lossy().into_owned();
    let profile_identity = verified.identity.clone();
    let (output_profile_path, output_profile_identity, device_link_path, device_link_identity) =
        match target.engine_mode {
            ConversionEngineMode::Icc => {
                (Some(profile_path), Some(profile_identity), None, None)
            }
            ConversionEngineMode::DeviceLink => {
                (None, None, Some(profile_path), Some(profile_identity))
            }
            ConversionEngineMode::CustomOptimizer => {
                return Err("Custom Optimizer unified execution is not enabled.".to_owned());
            }
        };
    let recipe = ConversionRecipe {
        source_transparency_policy: transparency_policy,
        schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
        engine_mode: target.engine_mode,
        source_profile_identity,
        target: ConversionTargetDefinition {
            name: target.target_name.trim().to_owned(),
            channels: target
                .channel_names
                .iter()
                .enumerate()
                .map(|(index, name)| TargetChannelDefinition {
                    name: name.trim().to_owned(),
                    display_rgb: Some(target_channel_rgb(name, index)),
                    solidity: 1.0,
                    max_coverage: None,
                })
                .collect(),
            bit_depth: target.output_bit_depth,
            output_profile_identity,
            output_profile_path,
            device_link_identity,
            device_link_path,
            characterization_id: None,
            total_ink_limit: None,
        },
        rendering_intent: target.rendering_intent,
        black_point_compensation: target.engine_mode == ConversionEngineMode::Icc
            && target.black_point_compensation,
        strategy: SeparationStrategy::default(),
        custom_optimizer_solver: None,
    };
    recipe.validate().map_err(|errors| errors.join(" "))?;
    Ok(recipe)
}

pub(crate) fn build_unified_plan(
    app: &ShadeApp,
    scope: ConversionBatchScope,
    inspections: &[ConversionFaceInspection],
    transparency_policies: &BTreeMap<usize, SourceTransparencyPolicy>,
    target: &ConversionTargetState,
    output_folder: &Path,
    destination_mode: UnifiedDestinationMode,
    selected_existing: Option<&Path>,
    candidates: &[ProductionDestinationCandidate],
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

    let output_paths = match deterministic_output_paths(app, output_folder, inspections) {
        Ok(paths) => paths,
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    };

    let frozen = match destination_mode {
        UnifiedDestinationMode::CreateNew => {
            FrozenProductionDestination::create_new(deterministic_production_project_path(
                output_folder,
                &app.project.name,
                &target.target_name,
            ))
        }
        UnifiedDestinationMode::AppendExisting => selected_existing
            .and_then(|path| candidates.iter().find(|candidate| candidate.path == path))
            .ok_or_else(|| "Select a compatible linked Production project.".to_owned())
            .and_then(|candidate| {
                FrozenProductionDestination::append_existing(candidate, &recipes[0])
            }),
    };
    let frozen = match frozen {
        Ok(frozen) => Some(frozen),
        Err(error) => {
            errors.push(error);
            None
        }
    };

    if !errors.is_empty() {
        return Err(errors);
    }
    let frozen = frozen.expect("frozen destination exists when plan validation passes");
    let _ = scope;
    Ok(UnifiedConversionPlan {
        production_project_path: frozen.production_project_path,
        disposition: frozen.disposition,
        output_paths,
        recipes,
    })
}

fn deterministic_output_paths(
    app: &ShadeApp,
    folder: &Path,
    inspections: &[ConversionFaceInspection],
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
        if output.exists() {
            return Err(format!(
                "Deterministic output already exists: {}. Route-owned transactional replacement is handled by conversion-route safety (#373); Shade Editor will not create a versioned duplicate.",
                output.display()
            ));
        }
        output_paths.push(output);
    }
    Ok(output_paths)
}

fn source_stem_occurrences(app: &ShadeApp, source_path: &Path) -> usize {
    let stem = source_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    app.faces
        .iter()
        .filter(|face| {
            face.path
                .file_stem()
                .map(|candidate| candidate.to_string_lossy().to_ascii_lowercase() == stem)
                .unwrap_or(false)
        })
        .count()
}

fn deterministic_production_project_path(
    folder: &Path,
    project_name: &str,
    target_name: &str,
) -> PathBuf {
    let project = safe_component(project_name, "Source");
    let target = safe_component(target_name, "Production");
    folder.join(format!("{project} - {target}.shade"))
}

fn safe_component(value: &str, fallback: &str) -> String {
    let mut result = value
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ' ') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    while result.contains("  ") {
        result = result.replace("  ", " ");
    }
    result = result.trim_matches([' ', '_']).to_owned();
    if result.is_empty() {
        result = fallback.to_owned();
    }
    result.chars().take(96).collect()
}

pub(crate) fn production_candidates(app: &ShadeApp) -> Vec<ProductionDestinationCandidate> {
    let Some(source_project_path) = app.project_path.as_deref() else {
        return Vec::new();
    };
    let Ok(value) = serde_json::to_value(&app.project) else {
        return Vec::new();
    };
    let Ok(source_project) =
        serde_json::from_value::<windows_shade_editor::model::ShadeProject>(value)
    else {
        return Vec::new();
    };
    inspect_linked_production_destinations(&source_project, source_project_path)
}

pub(crate) fn default_output_folder(app: &ShadeApp) -> Option<PathBuf> {
    app.project_path
        .as_deref()
        .and_then(Path::parent)
        .or_else(|| app.faces.get(app.current_face).and_then(|face| face.path.parent()))
        .map(|parent| parent.join("Production"))
}

pub(crate) fn conversion_color_model(model: RuntimeColorModel) -> ConversionColorModel {
    match model {
        RuntimeColorModel::Gray => ConversionColorModel::Gray,
        RuntimeColorModel::Rgb => ConversionColorModel::Rgb,
        RuntimeColorModel::Cmyk => ConversionColorModel::Cmyk,
        RuntimeColorModel::Other => ConversionColorModel::Other,
    }
}

fn execution_supported(format: SourceImageFormat, model: DesignSourceColorModel) -> bool {
    matches!(
        (format, model),
        (SourceImageFormat::Tiff, DesignSourceColorModel::Rgb)
            | (SourceImageFormat::Tiff, DesignSourceColorModel::Cmyk)
            | (SourceImageFormat::Png, DesignSourceColorModel::Rgb)
            | (SourceImageFormat::Jpeg, DesignSourceColorModel::Rgb)
    )
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase()
}

pub(crate) fn target_channel_rgb(name: &str, index: usize) -> [u8; 3] {
    let name = name.trim().to_ascii_lowercase();
    if name.contains("cyan") {
        [0, 174, 239]
    } else if name.contains("magenta") || name.contains("pink") {
        [236, 0, 140]
    } else if name.contains("yellow") {
        [255, 221, 0]
    } else if name.contains("black") || name == "k" {
        [28, 28, 28]
    } else if name.contains("blue") {
        [33, 102, 214]
    } else if name.contains("green") {
        [34, 160, 90]
    } else if name.contains("brown") {
        [139, 90, 43]
    } else if name.contains("beige") {
        [211, 184, 142]
    } else if name.contains("orange") {
        [239, 126, 34]
    } else if name.contains("red") {
        [214, 51, 63]
    } else {
        const FALLBACK: [[u8; 3]; 8] = [
            [39, 126, 220],
            [214, 65, 75],
            [45, 164, 103],
            [224, 151, 38],
            [145, 91, 201],
            [36, 166, 181],
            [191, 96, 51],
            [105, 113, 127],
        ];
        FALLBACK[index % FALLBACK.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_selection_is_deterministic() {
        let selected = BTreeSet::from([5, 1, 3, 99]);
        assert_eq!(
            scope_indices(ConversionBatchScope::SelectedFaces, 0, 6, &selected),
            vec![1, 3, 5]
        );
        assert_eq!(
            scope_indices(ConversionBatchScope::CurrentFace, 2, 6, &selected),
            vec![2]
        );
        assert_eq!(
            scope_indices(ConversionBatchScope::AllFaces, 2, 4, &selected),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn safe_project_component_is_windows_friendly_and_bounded() {
        let value = safe_component("Source: A / Durst 7C * target?", "Production");
        assert_eq!(value, "Source_ A _ Durst 7C _ target");
        assert!(value.len() <= 96);
    }

    #[test]
    fn target_display_color_is_shared_by_preview_and_recipe() {
        assert_eq!(target_channel_rgb("Cyan", 0), [0, 174, 239]);
        assert_eq!(target_channel_rgb("Black", 3), [28, 28, 28]);
    }
}
