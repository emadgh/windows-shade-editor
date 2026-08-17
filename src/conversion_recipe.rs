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
}
