use std::fs;
use std::path::{Path, PathBuf};

use crate::conversion_preset_library::{
    PresetLibraryError, SeparationPresetLibrary,
};
use crate::conversion_presets::{PresetOrigin, SeparationPresetDefinition};
use crate::safe_fs;

pub const CONVERSION_PRESET_FILENAME: &str = "conversion-presets.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresetStoreError {
    Library(PresetLibraryError),
    Io(String),
    BuiltInPersisted(String),
    RuntimeEntryNotBuiltIn(String),
    UserIdConflictsWithBuiltIn(String),
}

impl std::fmt::Display for PresetStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Library(error) => write!(formatter, "Invalid preset library: {error:?}"),
            Self::Io(error) => formatter.write_str(error),
            Self::BuiltInPersisted(id) => write!(
                formatter,
                "Persisted user preset library cannot contain built-in preset '{id}'."
            ),
            Self::RuntimeEntryNotBuiltIn(id) => write!(
                formatter,
                "Runtime built-in preset list contains non-built-in entry '{id}'."
            ),
            Self::UserIdConflictsWithBuiltIn(id) => write!(
                formatter,
                "User preset id '{id}' conflicts with a runtime built-in preset."
            ),
        }
    }
}

impl From<PresetLibraryError> for PresetStoreError {
    fn from(value: PresetLibraryError) -> Self {
        Self::Library(value)
    }
}

pub fn default_conversion_preset_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("ShadeEditor").join(CONVERSION_PRESET_FILENAME)
}

/// Load only operator-authored presets. Built-ins are binary-owned authority and
/// are intentionally reconstructed at runtime rather than trusted from disk.
pub fn load_user_preset_library(path: &Path) -> Result<SeparationPresetLibrary, PresetStoreError> {
    if !path.exists() {
        return Ok(SeparationPresetLibrary::new());
    }
    let json = fs::read_to_string(path).map_err(|error| {
        PresetStoreError::Io(format!(
            "Cannot read conversion preset library {}: {error}",
            path.display()
        ))
    })?;
    let library = SeparationPresetLibrary::from_json(&json)?;
    validate_user_only(&library)?;
    Ok(library)
}

pub fn save_user_preset_library(
    path: &Path,
    library: &SeparationPresetLibrary,
) -> Result<(), PresetStoreError> {
    library.validate()?;
    validate_user_only(library)?;
    let json = library.to_json_pretty()?;
    safe_fs::atomic_write(path, json.as_bytes(), None).map_err(|error| {
        PresetStoreError::Io(format!(
            "Cannot persist conversion preset library {}: {error}",
            path.display()
        ))
    })
}

/// Compose runtime built-ins with persisted user definitions. This is the only
/// supported direction: disk content can never grant `BuiltIn` authority.
pub fn compose_runtime_preset_library(
    built_ins: impl IntoIterator<Item = SeparationPresetDefinition>,
    user_library: &SeparationPresetLibrary,
) -> Result<SeparationPresetLibrary, PresetStoreError> {
    user_library.validate()?;
    validate_user_only(user_library)?;

    let mut runtime = SeparationPresetLibrary::new();
    for preset in built_ins {
        if preset.origin != PresetOrigin::BuiltIn {
            return Err(PresetStoreError::RuntimeEntryNotBuiltIn(preset.id));
        }
        runtime.insert(preset)?;
    }
    for preset in &user_library.presets {
        if runtime.get(&preset.id).is_some() {
            return Err(PresetStoreError::UserIdConflictsWithBuiltIn(
                preset.id.clone(),
            ));
        }
        runtime.insert(preset.clone())?;
    }
    Ok(runtime)
}

fn validate_user_only(library: &SeparationPresetLibrary) -> Result<(), PresetStoreError> {
    if let Some(preset) = library
        .presets
        .iter()
        .find(|preset| preset.origin != PresetOrigin::User)
    {
        return Err(PresetStoreError::BuiltInPersisted(preset.id.clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        ConversionEngineMode, ConversionTargetDefinition, TargetChannelDefinition,
    };
    use crate::conversion_presets::SeparationPresetDefinition;
    use crate::model::IccProfileIdentity;
    use std::time::{SystemTime, UNIX_EPOCH};

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
                sha256: "target-hash".to_owned(),
            }),
            output_profile_path: None,
            device_link_identity: None,
            device_link_path: None,
            characterization_id: None,
            total_ink_limit: Some(3.2),
        }
    }

    fn user_preset(id: &str, name: &str) -> SeparationPresetDefinition {
        let mut preset = SeparationPresetDefinition::built_in_balanced(
            &target(),
            ConversionEngineMode::Icc,
        );
        preset.id = id.to_owned();
        preset.name = name.to_owned();
        preset.origin = PresetOrigin::User;
        preset.strategy.preset_name = name.to_owned();
        preset
    }

    fn temp_path(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("shade-editor-preset-store-{}-{nonce}", std::process::id()))
            .join(name)
    }

    #[test]
    fn default_path_is_dedicated_versioned_app_data_file() {
        let path = default_conversion_preset_path();
        assert_eq!(path.file_name().unwrap(), CONVERSION_PRESET_FILENAME);
        assert_eq!(path.parent().unwrap().file_name().unwrap(), "ShadeEditor");
    }

    #[test]
    fn missing_library_loads_as_empty_user_library() {
        let path = temp_path(CONVERSION_PRESET_FILENAME);
        let library = load_user_preset_library(&path).unwrap();
        assert!(library.presets.is_empty());
    }

    #[test]
    fn malformed_json_fails_closed_as_typed_library_error() {
        let path = temp_path(CONVERSION_PRESET_FILENAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ definitely-not-json").unwrap();
        assert!(matches!(
            load_user_preset_library(&path),
            Err(PresetStoreError::Library(PresetLibraryError::InvalidJson(_)))
        ));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn unsupported_legacy_library_schema_fails_closed() {
        let path = temp_path(CONVERSION_PRESET_FILENAME);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"schema_version":0,"presets":[]}"#).unwrap();
        assert_eq!(
            load_user_preset_library(&path),
            Err(PresetStoreError::Library(
                PresetLibraryError::UnsupportedLibrarySchema(0)
            ))
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn user_library_round_trips_through_atomic_store() {
        let path = temp_path(CONVERSION_PRESET_FILENAME);
        let mut library = SeparationPresetLibrary::new();
        library.insert(user_preset("user:press", "Press")).unwrap();
        save_user_preset_library(&path, &library).unwrap();
        let restored = load_user_preset_library(&path).unwrap();
        assert_eq!(restored, library);
        assert!(!safe_fs::temp_path(&path).exists());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn persisted_built_in_authority_is_rejected() {
        let path = temp_path(CONVERSION_PRESET_FILENAME);
        let mut library = SeparationPresetLibrary::new();
        library
            .insert(SeparationPresetDefinition::built_in_balanced(
                &target(),
                ConversionEngineMode::Icc,
            ))
            .unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, library.to_json_pretty().unwrap()).unwrap();
        assert!(matches!(
            load_user_preset_library(&path),
            Err(PresetStoreError::BuiltInPersisted(_))
        ));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn runtime_composition_rejects_user_id_collision_with_built_in() {
        let built_in = SeparationPresetDefinition::built_in_balanced(
            &target(),
            ConversionEngineMode::Icc,
        );
        let mut users = SeparationPresetLibrary::new();
        users
            .insert(user_preset("builtin:balanced:v1", "Imposter"))
            .unwrap();
        assert!(matches!(
            compose_runtime_preset_library([built_in], &users),
            Err(PresetStoreError::UserIdConflictsWithBuiltIn(_))
        ));
    }

    #[test]
    fn runtime_composition_keeps_built_in_and_user_origins_distinct() {
        let built_in = SeparationPresetDefinition::built_in_balanced(
            &target(),
            ConversionEngineMode::Icc,
        );
        let mut users = SeparationPresetLibrary::new();
        users.insert(user_preset("user:press", "Press")).unwrap();
        let runtime = compose_runtime_preset_library([built_in], &users).unwrap();
        assert_eq!(runtime.presets.len(), 2);
        assert_eq!(runtime.presets[0].origin, PresetOrigin::BuiltIn);
        assert_eq!(runtime.presets[1].origin, PresetOrigin::User);
    }
}
