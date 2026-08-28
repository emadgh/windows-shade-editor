use std::path::{Path, PathBuf};

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_preset_library::{PresetLibraryError, SeparationPresetLibrary};
use crate::conversion_preset_store::{
    PresetStoreError, compose_runtime_preset_library, default_conversion_preset_path,
    load_user_preset_library, save_user_preset_library,
};
use crate::conversion_presets::{
    PresetApplyError, PresetCompatibility, PresetOrigin, SeparationPresetDefinition,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresetApplicationAvailability {
    Available,
    EngineDoesNotConsumeSeparationStrategy,
    CustomOptimizerNotProductionAuthorized,
}

impl PresetApplicationAvailability {
    pub fn reason(self) -> Option<&'static str> {
        match self {
            Self::Available => None,
            Self::EngineDoesNotConsumeSeparationStrategy => Some(
                "The selected ICC/DeviceLink engine does not consume Shade Editor separation-strategy controls. Applying this preset would change recipe/provenance without changing rendered pixels.",
            ),
            Self::CustomOptimizerNotProductionAuthorized => Some(
                "Custom Optimizer strategy presets remain unavailable until the production-authorized optimizer path is enabled with approved measured evidence.",
            ),
        }
    }
}

/// Central fail-closed policy for the unified Production Color Conversion UI.
///
/// ICC and DeviceLink transforms currently own their separation semantics, so
/// Shade Editor strategy presets must not be presented as pixel-affecting
/// controls for those engines. Custom Optimizer may consume the same persisted
/// strategy only after the caller confirms that its production path is actually
/// authorized/enabled.
pub fn unified_strategy_preset_availability(
    engine_mode: ConversionEngineMode,
    custom_optimizer_production_authorized: bool,
) -> PresetApplicationAvailability {
    match engine_mode {
        ConversionEngineMode::Icc | ConversionEngineMode::DeviceLink => {
            PresetApplicationAvailability::EngineDoesNotConsumeSeparationStrategy
        }
        ConversionEngineMode::CustomOptimizer if !custom_optimizer_production_authorized => {
            PresetApplicationAvailability::CustomOptimizerNotProductionAuthorized
        }
        ConversionEngineMode::CustomOptimizer => PresetApplicationAvailability::Available,
    }
}

#[derive(Debug)]
pub enum PresetRuntimeError {
    Store(PresetStoreError),
    Library(PresetLibraryError),
    Apply(PresetApplyError),
    UnknownPreset(String),
    InvalidRecipe(Vec<String>),
    ApplicationUnavailable(PresetApplicationAvailability),
}

impl std::fmt::Display for PresetRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Library(error) => write!(formatter, "Invalid preset library operation: {error:?}"),
            Self::Apply(error) => write!(formatter, "Cannot apply conversion preset: {error:?}"),
            Self::UnknownPreset(id) => write!(formatter, "Unknown conversion preset '{id}'."),
            Self::InvalidRecipe(errors) => write!(
                formatter,
                "Cannot capture conversion preset from current recipe: {}",
                errors.join(" ")
            ),
            Self::ApplicationUnavailable(availability) => formatter.write_str(
                availability
                    .reason()
                    .unwrap_or("Conversion preset application is unavailable."),
            ),
        }
    }
}

impl From<PresetStoreError> for PresetRuntimeError {
    fn from(value: PresetStoreError) -> Self {
        Self::Store(value)
    }
}

impl From<PresetLibraryError> for PresetRuntimeError {
    fn from(value: PresetLibraryError) -> Self {
        Self::Library(value)
    }
}

impl From<PresetApplyError> for PresetRuntimeError {
    fn from(value: PresetApplyError) -> Self {
        Self::Apply(value)
    }
}

#[derive(Clone, Debug)]
pub struct PresetRuntimeController {
    path: PathBuf,
    user_library: SeparationPresetLibrary,
}

impl PresetRuntimeController {
    pub fn load_default() -> Result<Self, PresetRuntimeError> {
        Self::load(default_conversion_preset_path())
    }

    pub fn load(path: PathBuf) -> Result<Self, PresetRuntimeError> {
        let user_library = load_user_preset_library(&path)?;
        Ok(Self { path, user_library })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn user_library(&self) -> &SeparationPresetLibrary {
        &self.user_library
    }

    pub fn runtime_library(
        &self,
        built_ins: &[SeparationPresetDefinition],
    ) -> Result<SeparationPresetLibrary, PresetRuntimeError> {
        Ok(compose_runtime_preset_library(
            built_ins.iter().cloned(),
            &self.user_library,
        )?)
    }

    pub fn compatibility(
        &self,
        preset_id: &str,
        recipe: &ConversionRecipe,
        built_ins: &[SeparationPresetDefinition],
    ) -> Result<PresetCompatibility, PresetRuntimeError> {
        let runtime = self.runtime_library(built_ins)?;
        let preset = runtime
            .get(preset_id)
            .ok_or_else(|| PresetRuntimeError::UnknownPreset(preset_id.to_owned()))?;
        Ok(preset.compatibility_with_recipe(recipe))
    }

    pub fn apply_to_recipe(
        &self,
        preset_id: &str,
        recipe: &ConversionRecipe,
        built_ins: &[SeparationPresetDefinition],
        availability: PresetApplicationAvailability,
    ) -> Result<ConversionRecipe, PresetRuntimeError> {
        require_available(availability)?;
        let runtime = self.runtime_library(built_ins)?;
        let preset = runtime
            .get(preset_id)
            .ok_or_else(|| PresetRuntimeError::UnknownPreset(preset_id.to_owned()))?;
        Ok(preset.apply_to_recipe(recipe)?)
    }

    pub fn save_current_recipe_as_user(
        &mut self,
        recipe: &ConversionRecipe,
        id: impl Into<String>,
        name: impl Into<String>,
        notes: Option<String>,
        built_ins: &[SeparationPresetDefinition],
        availability: PresetApplicationAvailability,
    ) -> Result<SeparationPresetDefinition, PresetRuntimeError> {
        require_available(availability)?;
        let preset = SeparationPresetDefinition::capture_from_recipe(
            id,
            name,
            notes,
            PresetOrigin::User,
            recipe,
        )
        .map_err(PresetRuntimeError::InvalidRecipe)?;
        self.insert_user_transactional(preset.clone(), built_ins)?;
        Ok(preset)
    }

    pub fn duplicate_as_user(
        &mut self,
        source_id: &str,
        new_id: impl Into<String>,
        new_name: impl Into<String>,
        built_ins: &[SeparationPresetDefinition],
    ) -> Result<SeparationPresetDefinition, PresetRuntimeError> {
        let runtime = self.runtime_library(built_ins)?;
        let mut preset = runtime
            .get(source_id)
            .cloned()
            .ok_or_else(|| PresetRuntimeError::UnknownPreset(source_id.to_owned()))?;
        preset.id = new_id.into();
        preset.name = new_name.into();
        preset.origin = PresetOrigin::User;
        preset.strategy.preset_name = preset.name.clone();
        self.insert_user_transactional(preset.clone(), built_ins)?;
        Ok(preset)
    }

    pub fn rename_user(
        &mut self,
        id: &str,
        new_name: impl Into<String>,
        built_ins: &[SeparationPresetDefinition],
    ) -> Result<(), PresetRuntimeError> {
        let mut candidate = self.user_library.clone();
        candidate.rename_user(id, new_name)?;
        self.commit_candidate(candidate, built_ins)
    }

    pub fn delete_user(
        &mut self,
        id: &str,
        built_ins: &[SeparationPresetDefinition],
    ) -> Result<(), PresetRuntimeError> {
        let mut candidate = self.user_library.clone();
        candidate.delete_user(id)?;
        self.commit_candidate(candidate, built_ins)
    }

    pub fn import_user_json(
        &mut self,
        json: &str,
        built_ins: &[SeparationPresetDefinition],
    ) -> Result<SeparationPresetDefinition, PresetRuntimeError> {
        let mut candidate = self.user_library.clone();
        let imported = candidate.import_user_preset_json(json)?.clone();
        self.commit_candidate(candidate, built_ins)?;
        Ok(imported)
    }

    pub fn export_preset_json(
        &self,
        id: &str,
        built_ins: &[SeparationPresetDefinition],
    ) -> Result<String, PresetRuntimeError> {
        let runtime = self.runtime_library(built_ins)?;
        Ok(runtime.export_preset_json(id)?)
    }

    fn insert_user_transactional(
        &mut self,
        preset: SeparationPresetDefinition,
        built_ins: &[SeparationPresetDefinition],
    ) -> Result<(), PresetRuntimeError> {
        let mut candidate = self.user_library.clone();
        candidate.insert(preset)?;
        self.commit_candidate(candidate, built_ins)
    }

    fn commit_candidate(
        &mut self,
        candidate: SeparationPresetLibrary,
        built_ins: &[SeparationPresetDefinition],
    ) -> Result<(), PresetRuntimeError> {
        // Validate the exact runtime composition before touching disk so a user
        // definition can never shadow or impersonate binary-owned built-ins.
        compose_runtime_preset_library(built_ins.iter().cloned(), &candidate)?;
        save_user_preset_library(&self.path, &candidate)?;
        self.user_library = candidate;
        Ok(())
    }
}

fn require_available(
    availability: PresetApplicationAvailability,
) -> Result<(), PresetRuntimeError> {
    if availability == PresetApplicationAvailability::Available {
        Ok(())
    } else {
        Err(PresetRuntimeError::ApplicationUnavailable(availability))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent,
        ConversionTargetDefinition, SeparationStrategy, TargetChannelDefinition,
    };
    use crate::model::IccProfileIdentity;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn target() -> ConversionTargetDefinition {
        ConversionTargetDefinition {
            name: "Ceramic CMYK".to_owned(),
            channels: ["Cyan", "Magenta", "Yellow", "Black"]
                .into_iter()
                .map(|name| TargetChannelDefinition {
                    name: name.to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: Some(0.9),
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: Some(IccProfileIdentity {
                description: "Target".to_owned(),
                sha256: hash('b'),
            }),
            output_profile_path: Some(r"C:\Color\Press.icc".to_owned()),
            device_link_identity: None,
            device_link_path: None,
            characterization_id: None,
            total_ink_limit: Some(3.2),
        }
    }

    fn recipe() -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: hash('a'),
            },
            target: target(),
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: true,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: None,
        }
    }

    fn built_ins() -> Vec<SeparationPresetDefinition> {
        vec![SeparationPresetDefinition::built_in_balanced(
            &target(),
            ConversionEngineMode::Icc,
        )]
    }

    fn temp_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("shade-editor-preset-runtime-{}-{nonce}", std::process::id()))
            .join(name)
    }

    #[test]
    fn icc_and_devicelink_strategy_application_fail_closed() {
        for engine in [ConversionEngineMode::Icc, ConversionEngineMode::DeviceLink] {
            let availability = unified_strategy_preset_availability(engine, false);
            assert_eq!(
                availability,
                PresetApplicationAvailability::EngineDoesNotConsumeSeparationStrategy
            );
            assert!(availability.reason().unwrap().contains("rendered pixels"));
        }
        assert_eq!(
            unified_strategy_preset_availability(ConversionEngineMode::CustomOptimizer, false),
            PresetApplicationAvailability::CustomOptimizerNotProductionAuthorized
        );
        assert_eq!(
            unified_strategy_preset_availability(ConversionEngineMode::CustomOptimizer, true),
            PresetApplicationAvailability::Available
        );
    }

    #[test]
    fn unavailable_application_cannot_change_recipe_or_provenance_only() {
        let path = temp_path("conversion-presets.json");
        let controller = PresetRuntimeController::load(path.clone()).unwrap();
        let error = controller
            .apply_to_recipe(
                "builtin:balanced:v1",
                &recipe(),
                &built_ins(),
                PresetApplicationAvailability::EngineDoesNotConsumeSeparationStrategy,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            PresetRuntimeError::ApplicationUnavailable(
                PresetApplicationAvailability::EngineDoesNotConsumeSeparationStrategy
            )
        ));
        assert!(!path.exists());
    }

    #[test]
    fn lifecycle_mutations_are_persisted_transactionally() {
        let path = temp_path("conversion-presets.json");
        let built_ins = built_ins();
        let mut controller = PresetRuntimeController::load(path.clone()).unwrap();
        controller
            .duplicate_as_user(
                "builtin:balanced:v1",
                "user:press",
                "Press preset",
                &built_ins,
            )
            .unwrap();
        controller
            .rename_user("user:press", "Press renamed", &built_ins)
            .unwrap();

        let restored = PresetRuntimeController::load(path.clone()).unwrap();
        assert_eq!(
            restored.user_library().get("user:press").unwrap().name,
            "Press renamed"
        );

        controller.delete_user("user:press", &built_ins).unwrap();
        let restored = PresetRuntimeController::load(path.clone()).unwrap();
        assert!(restored.user_library().presets.is_empty());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn import_collision_with_runtime_builtin_rolls_back_without_disk_mutation() {
        let path = temp_path("conversion-presets.json");
        let built_ins = built_ins();
        let mut controller = PresetRuntimeController::load(path.clone()).unwrap();
        let json = serde_json::to_string(&built_ins[0]).unwrap();
        let error = controller.import_user_json(&json, &built_ins).unwrap_err();
        assert!(matches!(error, PresetRuntimeError::Store(_)));
        assert!(controller.user_library().presets.is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn portable_preset_export_contains_identity_not_profile_payload_or_path() {
        let path = temp_path("conversion-presets.json");
        let controller = PresetRuntimeController::load(path).unwrap();
        let json = controller
            .export_preset_json("builtin:balanced:v1", &built_ins())
            .unwrap();
        assert!(json.contains(&hash('b')));
        assert!(!json.contains("Press.icc"));
        assert!(!json.contains("output_profile_path"));
        assert!(!json.contains("device_link_path"));
    }

    #[test]
    fn compatibility_is_exposed_separately_from_engine_application_availability() {
        let path = temp_path("conversion-presets.json");
        let controller = PresetRuntimeController::load(path).unwrap();
        assert_eq!(
            controller
                .compatibility("builtin:balanced:v1", &recipe(), &built_ins())
                .unwrap(),
            PresetCompatibility::Compatible
        );
        assert_eq!(
            unified_strategy_preset_availability(ConversionEngineMode::Icc, false),
            PresetApplicationAvailability::EngineDoesNotConsumeSeparationStrategy
        );
    }
}
