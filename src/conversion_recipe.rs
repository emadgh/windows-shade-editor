use sha2::{Digest, Sha256};

use crate::color_conversion::ConversionRecipe;

/// Produce the stable identity used to bind conversion provenance, queue jobs
/// and cached diagnostics to an exact serialized recipe.
///
/// `ConversionRecipe` uses ordered struct fields and `BTreeMap` for per-ink
/// settings, so serde_json produces deterministic bytes for equivalent recipe
/// values. The hash is an identity/fingerprint, not a signature.
pub fn recipe_sha256(recipe: &ConversionRecipe) -> Result<String, String> {
    let bytes = serde_json::to_vec(recipe)
        .map_err(|err| format!("Cannot serialize conversion recipe for fingerprinting: {err}"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn recipe_matches_hash(recipe: &ConversionRecipe, expected_sha256: &str) -> Result<bool, String> {
    Ok(recipe_sha256(recipe)?.eq_ignore_ascii_case(expected_sha256.trim()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::color_conversion::{
        ConversionEngineMode, ConversionRenderingIntent, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition, CONVERSION_RECIPE_SCHEMA_VERSION,
        LEGACY_CONVERSION_RECIPE_SCHEMA_VERSION, SOLVER_CONVERSION_RECIPE_SCHEMA_VERSION,
    };
    use crate::model::IccProfileIdentity;

    fn identity(description: &str, hash: &str) -> IccProfileIdentity {
        IccProfileIdentity {
            description: description.to_owned(),
            sha256: hash.to_owned(),
        }
    }

    fn recipe() -> ConversionRecipe {
        let mut bias = BTreeMap::new();
        bias.insert("Blue".to_owned(), -0.5);
        bias.insert("Black".to_owned(), 0.8);

        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: identity("Adobe RGB", "source-hash"),
            target: ConversionTargetDefinition {
                name: "Ceramic 4C".to_owned(),
                channels: ["Blue", "Brown", "Beige", "Black"]
                    .into_iter()
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: Some(0.8),
                    })
                    .collect(),
                bit_depth: 16,
                output_profile_identity: None,
                output_profile_path: None,
                device_link_identity: None,
                device_link_path: None,
                characterization_id: Some("measurement-v1".to_owned()),
                total_ink_limit: Some(1.6),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy {
                preset_name: "Black-focused".to_owned(),
                black_channel: Some("Black".to_owned()),
                black_generation_strength: 0.8,
                black_start: 0.2,
                black_max: 0.7,
                neutral_chroma_threshold: 8.0,
                per_ink_bias: bias,
                total_ink_limit: Some(1.4),
                max_delta_e00: Some(2.0),
            },
            custom_optimizer_solver: Some(crate::custom_optimizer_config::CustomOptimizerSolverConfig::default()),
        }
    }

    #[test]
    fn identical_recipe_has_identical_hash() {
        let first = recipe();
        let second = first.clone();
        assert_eq!(recipe_sha256(&first).unwrap(), recipe_sha256(&second).unwrap());
    }

    #[test]
    fn semantically_relevant_strategy_change_changes_hash() {
        let first = recipe();
        let mut second = first.clone();
        second.strategy.black_generation_strength = 0.9;
        assert_ne!(recipe_sha256(&first).unwrap(), recipe_sha256(&second).unwrap());
    }

    #[test]
    fn btree_map_order_does_not_make_recipe_identity_unstable() {
        let first = recipe();
        let mut second = recipe();
        second.strategy.per_ink_bias.clear();
        second.strategy.per_ink_bias.insert("Black".to_owned(), 0.8);
        second.strategy.per_ink_bias.insert("Blue".to_owned(), -0.5);
        assert_eq!(recipe_sha256(&first).unwrap(), recipe_sha256(&second).unwrap());
    }

    #[test]
    fn match_is_case_insensitive_for_hex_transport() {
        let recipe = recipe();
        let hash = recipe_sha256(&recipe).unwrap();
        assert!(recipe_matches_hash(&recipe, &hash.to_uppercase()).unwrap());
    }

    fn legacy_icc_recipe() -> ConversionRecipe {
        let mut legacy = recipe();
        legacy.schema_version = LEGACY_CONVERSION_RECIPE_SCHEMA_VERSION;
        legacy.engine_mode = ConversionEngineMode::Icc;
        legacy.target.output_profile_identity = Some(identity("Legacy output", "legacy-output-hash"));
        legacy.target.output_profile_path = Some(r"C:\Color\Legacy.icc".to_owned());
        legacy.target.device_link_identity = None;
        legacy.target.device_link_path = None;
        legacy.custom_optimizer_solver = None;
        legacy
    }

    #[test]
    fn legacy_icc_json_round_trip_preserves_recipe_hash_and_omits_solver_field() {
        let legacy = legacy_icc_recipe();
        legacy.validate().expect("legacy ICC recipe remains valid");
        let before = recipe_sha256(&legacy).unwrap();
        let json = serde_json::to_string(&legacy).unwrap();
        assert!(!json.contains("custom_optimizer_solver"));
        let restored: ConversionRecipe = serde_json::from_str(&json).unwrap();
        restored.validate().expect("restored legacy ICC recipe remains valid");
        assert_eq!(before, recipe_sha256(&restored).unwrap());
    }

    #[test]
    fn legacy_custom_optimizer_deserializes_but_fails_closed_without_solver_provenance() {
        let mut legacy = recipe();
        legacy.schema_version = LEGACY_CONVERSION_RECIPE_SCHEMA_VERSION;
        legacy.custom_optimizer_solver = None;
        let json = serde_json::to_string(&legacy).unwrap();
        assert!(!json.contains("custom_optimizer_solver"));
        let restored: ConversionRecipe = serde_json::from_str(&json).unwrap();
        let errors = restored.validate().expect_err("legacy optimizer must be recaptured");
        assert!(errors.iter().any(|error| error.contains("solver provenance")));
    }

    #[test]
    fn current_schema_custom_optimizer_requires_explicit_solver_config() {
        let mut missing = recipe();
        missing.custom_optimizer_solver = None;
        let errors = missing.validate().expect_err("schema-v2 optimizer needs solver config");
        assert!(errors.iter().any(|error| error.contains("requires explicit solver")));
    }

    #[test]
    fn schema_v2_without_policy_round_trips_and_omits_transparency_field() {
        let mut schema_v2 = recipe();
        schema_v2.schema_version = SOLVER_CONVERSION_RECIPE_SCHEMA_VERSION;
        schema_v2.source_transparency_policy = None;
        schema_v2
            .validate()
            .expect("schema-v2 recipe without alpha policy remains valid");
        let before = recipe_sha256(&schema_v2).unwrap();
        let json = serde_json::to_string(&schema_v2).unwrap();
        assert!(!json.contains("source_transparency_policy"));
        let restored: ConversionRecipe = serde_json::from_str(&json).unwrap();
        restored
            .validate()
            .expect("restored schema-v2 recipe remains valid");
        assert_eq!(before, recipe_sha256(&restored).unwrap());
    }

    #[test]
    fn schema_v2_rejects_a_transparency_policy_it_cannot_define() {
        let mut schema_v2 = recipe();
        schema_v2.schema_version = SOLVER_CONVERSION_RECIPE_SCHEMA_VERSION;
        schema_v2.source_transparency_policy = Some(
            crate::source_transparency::SourceTransparencyPolicy::FlattenSolidRgb16 {
                background_rgb: [u16::MAX; 3],
            },
        );
        let errors = schema_v2
            .validate()
            .expect_err("schema-v2 must fail closed when a v3 alpha policy is injected");
        assert!(errors.iter().any(|error| error.contains("schema 3 or newer")));
    }

    #[test]
    fn schema_v3_policy_round_trip_preserves_policy_and_recipe_hash() {
        let mut with_policy = recipe();
        let policy = crate::source_transparency::SourceTransparencyPolicy::FlattenSolidRgb16 {
            background_rgb: [65535, 32768, 1234],
        };
        with_policy.source_transparency_policy = Some(policy);
        with_policy.validate().expect("schema-v3 alpha policy is valid");
        let before = recipe_sha256(&with_policy).unwrap();
        let json = serde_json::to_string(&with_policy).unwrap();
        assert!(json.contains("source_transparency_policy"));
        let restored: ConversionRecipe = serde_json::from_str(&json).unwrap();
        restored.validate().expect("restored schema-v3 recipe is valid");
        assert_eq!(restored.source_transparency_policy, Some(policy));
        assert_eq!(before, recipe_sha256(&restored).unwrap());
    }

    #[test]
    fn transparency_background_change_changes_recipe_identity() {
        let first = recipe();
        let mut second = first.clone();
        second.source_transparency_policy = Some(
            crate::source_transparency::SourceTransparencyPolicy::FlattenSolidRgb16 {
                background_rgb: [u16::MAX, u16::MAX, u16::MAX],
            },
        );
        let mut third = second.clone();
        third.source_transparency_policy = Some(
            crate::source_transparency::SourceTransparencyPolicy::FlattenSolidRgb16 {
                background_rgb: [0, 0, 0],
            },
        );
        assert_ne!(recipe_sha256(&first).unwrap(), recipe_sha256(&second).unwrap());
        assert_ne!(recipe_sha256(&second).unwrap(), recipe_sha256(&third).unwrap());
    }

    #[test]
    fn solver_policy_change_changes_recipe_identity() {
        let first = recipe();
        let mut second = first.clone();
        second
            .custom_optimizer_solver
            .as_mut()
            .expect("custom optimizer config")
            .initial_samples += 32;
        assert_ne!(recipe_sha256(&first).unwrap(), recipe_sha256(&second).unwrap());
    }

    #[test]
    fn icc_recipe_rejects_custom_optimizer_solver_state() {
        let mut invalid = legacy_icc_recipe();
        invalid.schema_version = CONVERSION_RECIPE_SCHEMA_VERSION;
        invalid.custom_optimizer_solver = Some(
            crate::custom_optimizer_config::CustomOptimizerSolverConfig::default(),
        );
        let errors = invalid.validate().expect_err("ICC must not carry optimizer solver state");
        assert!(errors.iter().any(|error| error.contains("ICC recipes must not carry")));
    }

}
