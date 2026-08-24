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

    #[test]
    fn create_new_is_backward_default() {
        assert_eq!(ProductionProjectDisposition::default(), ProductionProjectDisposition::CreateNew);
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
}
