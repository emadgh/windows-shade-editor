use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::color_conversion::ConversionEngineMode;
use crate::production_project_compat::ProductionCompatibilityKey;

/// Immutable destination policy captured with a production conversion job.
///
/// This must never be inferred from whether `production_project_path` happens
/// to exist at execution time: queue replay must preserve the operator's exact
/// intent.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ProductionProjectDisposition {
    #[default]
    CreateNew,
    AppendExisting {
        expected_project_sha256: String,
        expected_compatibility: CapturedProductionCompatibilityKey,
    },
    UpdateExistingRoute {
        expected_project_sha256: String,
        expected_compatibility: CapturedProductionCompatibilityKey,
        route_policy_sha256: String,
        allow_production_work_discard: bool,
    },
    MigrateExistingRoute {
        expected_project_sha256: String,
        previous_compatibility: CapturedProductionCompatibilityKey,
        new_compatibility: CapturedProductionCompatibilityKey,
        previous_route_policy_sha256: String,
        new_route_policy_sha256: String,
        route_faces: Vec<CapturedRouteFaceOwnership>,
        migration_ordinal: usize,
        migration_face_count: usize,
        confirm_destructive_migration: bool,
        allow_production_work_discard: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapturedRouteFaceOwnership {
    pub source_face_path: String,
    pub output_path: String,
    pub previous_recipe_sha256: String,
}

impl CapturedRouteFaceOwnership {
    pub fn validate(&self) -> Result<(), String> {
        if self.source_face_path.trim().is_empty() || self.output_path.trim().is_empty() {
            return Err("Route migration Face ownership paths cannot be empty.".to_owned());
        }
        if !is_bare_sha256(&self.previous_recipe_sha256) {
            return Err(
                "Route migration Face ownership requires canonical previous recipe SHA-256."
                    .to_owned(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapturedProductionCompatibilityKey {
    pub engine_mode: ConversionEngineMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_profile_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_link_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub characterization_id: Option<String>,
    pub channel_names: Vec<String>,
    pub bit_depth: u8,
}

impl CapturedProductionCompatibilityKey {
    pub fn from_runtime(key: &ProductionCompatibilityKey) -> Self {
        Self {
            engine_mode: key.engine_mode,
            output_profile_sha256: key.output_profile_sha256.clone(),
            device_link_sha256: key.device_link_sha256.clone(),
            characterization_id: key.characterization_id.clone(),
            channel_names: key.channel_names.clone(),
            bit_depth: key.bit_depth,
        }
    }

    pub fn matches_runtime(&self, key: &ProductionCompatibilityKey) -> bool {
        self.engine_mode == key.engine_mode
            && self.output_profile_sha256 == key.output_profile_sha256
            && self.device_link_sha256 == key.device_link_sha256
            && self.characterization_id == key.characterization_id
            && self.channel_names == key.channel_names
            && self.bit_depth == key.bit_depth
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.channel_names.is_empty() {
            return Err("Captured Production compatibility requires target channels.".to_owned());
        }
        if self.channel_names.iter().any(|name| name.trim().is_empty()) {
            return Err(
                "Captured Production compatibility cannot contain an empty target channel name."
                    .to_owned(),
            );
        }
        let mut normalized = self
            .channel_names
            .iter()
            .map(|name| name.trim().to_ascii_lowercase())
            .collect::<Vec<_>>();
        normalized.sort();
        normalized.dedup();
        if normalized.len() != self.channel_names.len() {
            return Err(
                "Captured Production compatibility target channel names must be unique."
                    .to_owned(),
            );
        }
        if !matches!(self.bit_depth, 8 | 16) {
            return Err(format!(
                "Captured Production compatibility bit depth {} is unsupported.",
                self.bit_depth
            ));
        }
        match self.engine_mode {
            ConversionEngineMode::Icc => {
                if !has_identity(self.output_profile_sha256.as_deref()) {
                    return Err(
                        "Captured ICC Production compatibility requires target output ICC identity."
                            .to_owned(),
                    );
                }
            }
            ConversionEngineMode::DeviceLink => {
                if !has_identity(self.device_link_sha256.as_deref()) {
                    return Err(
                        "Captured DeviceLink Production compatibility requires DeviceLink identity."
                            .to_owned(),
                    );
                }
            }
            ConversionEngineMode::CustomOptimizer => {
                if !has_identity(self.characterization_id.as_deref()) {
                    return Err(
                        "Captured Custom Optimizer Production compatibility requires characterization identity."
                            .to_owned(),
                    );
                }
            }
        }
        Ok(())
    }
}

impl ProductionProjectDisposition {
    pub fn append_existing(
        expected_project_sha256: impl Into<String>,
        compatibility: &ProductionCompatibilityKey,
    ) -> Result<Self, String> {
        let disposition = Self::AppendExisting {
            expected_project_sha256: expected_project_sha256.into(),
            expected_compatibility: CapturedProductionCompatibilityKey::from_runtime(compatibility),
        };
        disposition.validate()?;
        Ok(disposition)
    }

    pub fn update_existing_route(
        expected_project_sha256: impl Into<String>,
        compatibility: &ProductionCompatibilityKey,
        route_policy_sha256: impl Into<String>,
        allow_production_work_discard: bool,
    ) -> Result<Self, String> {
        let disposition = Self::UpdateExistingRoute {
            expected_project_sha256: expected_project_sha256.into(),
            expected_compatibility: CapturedProductionCompatibilityKey::from_runtime(compatibility),
            route_policy_sha256: route_policy_sha256.into(),
            allow_production_work_discard,
        };
        disposition.validate()?;
        Ok(disposition)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn migrate_existing_route(
        expected_project_sha256: impl Into<String>,
        previous_compatibility: &ProductionCompatibilityKey,
        new_compatibility: &ProductionCompatibilityKey,
        previous_route_policy_sha256: impl Into<String>,
        new_route_policy_sha256: impl Into<String>,
        route_faces: Vec<CapturedRouteFaceOwnership>,
        migration_ordinal: usize,
        migration_face_count: usize,
        confirm_destructive_migration: bool,
        allow_production_work_discard: bool,
    ) -> Result<Self, String> {
        let disposition = Self::MigrateExistingRoute {
            expected_project_sha256: expected_project_sha256.into(),
            previous_compatibility: CapturedProductionCompatibilityKey::from_runtime(
                previous_compatibility,
            ),
            new_compatibility: CapturedProductionCompatibilityKey::from_runtime(new_compatibility),
            previous_route_policy_sha256: previous_route_policy_sha256.into(),
            new_route_policy_sha256: new_route_policy_sha256.into(),
            route_faces,
            migration_ordinal,
            migration_face_count,
            confirm_destructive_migration,
            allow_production_work_discard,
        };
        disposition.validate()?;
        Ok(disposition)
    }

    pub fn is_route_migration(&self) -> bool {
        matches!(self, Self::MigrateExistingRoute { .. })
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::CreateNew => Ok(()),
            Self::AppendExisting {
                expected_project_sha256,
                expected_compatibility,
            } => {
                if !is_bare_sha256(expected_project_sha256) {
                    return Err(
                        "Append-existing Production capture requires canonical lowercase project SHA-256."
                            .to_owned(),
                    );
                }
                expected_compatibility.validate()
            }
            Self::UpdateExistingRoute {
                expected_project_sha256,
                expected_compatibility,
                route_policy_sha256,
                ..
            } => {
                if !is_bare_sha256(expected_project_sha256) {
                    return Err(
                        "Existing-route update requires canonical lowercase project SHA-256."
                            .to_owned(),
                    );
                }
                if !is_bare_sha256(route_policy_sha256) {
                    return Err(
                        "Existing-route update requires canonical lowercase route-policy SHA-256."
                            .to_owned(),
                    );
                }
                expected_compatibility.validate()
            }
            Self::MigrateExistingRoute {
                expected_project_sha256,
                previous_compatibility,
                new_compatibility,
                previous_route_policy_sha256,
                new_route_policy_sha256,
                route_faces,
                migration_ordinal,
                migration_face_count,
                confirm_destructive_migration,
                ..
            } => {
                if !is_bare_sha256(expected_project_sha256) {
                    return Err(
                        "Route migration requires canonical lowercase project SHA-256."
                            .to_owned(),
                    );
                }
                if !is_bare_sha256(previous_route_policy_sha256)
                    || !is_bare_sha256(new_route_policy_sha256)
                {
                    return Err(
                        "Route migration requires canonical previous/new policy SHA-256 identities."
                            .to_owned(),
                    );
                }
                if !*confirm_destructive_migration {
                    return Err(
                        "Route migration requires explicit destructive migration confirmation."
                            .to_owned(),
                    );
                }
                if *migration_face_count == 0
                    || *migration_ordinal >= *migration_face_count
                    || route_faces.len() != *migration_face_count
                {
                    return Err(
                        "Route migration ordinal/count does not match the frozen route Face set."
                            .to_owned(),
                    );
                }
                previous_compatibility.validate()?;
                new_compatibility.validate()?;
                let mut sources = BTreeSet::new();
                let mut outputs = BTreeSet::new();
                for face in route_faces {
                    face.validate()?;
                    if !sources.insert(path_key(&face.source_face_path)) {
                        return Err(
                            "Route migration contains duplicate Source Face ownership."
                                .to_owned(),
                        );
                    }
                    if !outputs.insert(path_key(&face.output_path)) {
                        return Err(
                            "Route migration contains duplicate output path ownership."
                                .to_owned(),
                        );
                    }
                }
                Ok(())
            }
        }
    }
}

fn has_identity(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}

fn is_bare_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn path_key(value: &str) -> String {
    value.trim().replace('/', "\\").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime_key() -> ProductionCompatibilityKey {
        ProductionCompatibilityKey {
            engine_mode: ConversionEngineMode::Icc,
            output_profile_sha256: Some("a".repeat(64)),
            device_link_sha256: None,
            characterization_id: None,
            channel_names: vec![
                "Cyan".to_owned(),
                "Magenta".to_owned(),
                "Yellow".to_owned(),
                "Black".to_owned(),
            ],
            bit_depth: 16,
        }
    }

    fn route_faces() -> Vec<CapturedRouteFaceOwnership> {
        vec![CapturedRouteFaceOwnership {
            source_face_path: r"C:\Design\Face.tif".to_owned(),
            output_path: r"C:\Production\Face.tif".to_owned(),
            previous_recipe_sha256: "f".repeat(64),
        }]
    }

    #[test]
    fn create_new_is_backward_default() {
        assert_eq!(
            ProductionProjectDisposition::default(),
            ProductionProjectDisposition::CreateNew
        );
    }

    #[test]
    fn append_capture_round_trips_and_matches_runtime_key() {
        let key = runtime_key();
        let disposition = ProductionProjectDisposition::append_existing("b".repeat(64), &key)
            .expect("valid append disposition");
        let json = serde_json::to_string(&disposition).unwrap();
        let restored: ProductionProjectDisposition = serde_json::from_str(&json).unwrap();
        assert_eq!(disposition, restored);
        let ProductionProjectDisposition::AppendExisting {
            expected_compatibility,
            ..
        } = restored
        else {
            panic!("append disposition expected");
        };
        assert!(expected_compatibility.matches_runtime(&key));
    }

    #[test]
    fn append_capture_rejects_invalid_project_hash() {
        let error = ProductionProjectDisposition::append_existing("not-a-hash", &runtime_key())
            .expect_err("invalid project hash must fail");
        assert!(error.contains("SHA-256"));
    }

    #[test]
    fn compatibility_snapshot_preserves_channel_order() {
        let key = runtime_key();
        let mut captured = CapturedProductionCompatibilityKey::from_runtime(&key);
        captured.channel_names.swap(0, 1);
        assert!(!captured.matches_runtime(&key));
    }

    #[test]
    fn route_update_freezes_project_target_policy_and_confirmation_intent() {
        let key = runtime_key();
        let disposition = ProductionProjectDisposition::update_existing_route(
            "b".repeat(64),
            &key,
            "c".repeat(64),
            true,
        )
        .unwrap();
        let json = serde_json::to_string(&disposition).unwrap();
        let restored: ProductionProjectDisposition = serde_json::from_str(&json).unwrap();
        assert_eq!(disposition, restored);
        assert!(matches!(
            restored,
            ProductionProjectDisposition::UpdateExistingRoute {
                allow_production_work_discard: true,
                ..
            }
        ));
    }

    #[test]
    fn route_migration_requires_explicit_confirmation_and_freezes_face_ownership() {
        let previous = runtime_key();
        let mut new = runtime_key();
        new.output_profile_sha256 = Some("d".repeat(64));
        let denied = ProductionProjectDisposition::migrate_existing_route(
            "b".repeat(64),
            &previous,
            &new,
            "c".repeat(64),
            "e".repeat(64),
            route_faces(),
            0,
            1,
            false,
            true,
        )
        .expect_err("migration confirmation is mandatory");
        assert!(denied.contains("explicit destructive"));

        let disposition = ProductionProjectDisposition::migrate_existing_route(
            "b".repeat(64),
            &previous,
            &new,
            "c".repeat(64),
            "e".repeat(64),
            route_faces(),
            0,
            1,
            true,
            true,
        )
        .unwrap();
        assert!(disposition.is_route_migration());
        let json = serde_json::to_string(&disposition).unwrap();
        let restored: ProductionProjectDisposition = serde_json::from_str(&json).unwrap();
        assert_eq!(disposition, restored);
    }

    #[test]
    fn route_migration_allows_same_target_policy_when_per_face_recipe_changes() {
        let key = runtime_key();
        let disposition = ProductionProjectDisposition::migrate_existing_route(
            "b".repeat(64),
            &key,
            &key,
            "c".repeat(64),
            "c".repeat(64),
            route_faces(),
            0,
            1,
            true,
            false,
        )
        .unwrap();
        assert!(disposition.is_route_migration());
    }
}
