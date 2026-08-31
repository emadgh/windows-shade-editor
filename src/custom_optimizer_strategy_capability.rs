use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_recipe::recipe_sha256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustomOptimizerStrategyAuthorityKind {
    MeasuredProduction,
    ProfileBackedOutputIcc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomOptimizerStrategyCapability {
    kind: CustomOptimizerStrategyAuthorityKind,
    recipe_sha256: String,
}

impl CustomOptimizerStrategyCapability {
    pub fn kind(&self) -> CustomOptimizerStrategyAuthorityKind {
        self.kind
    }

    pub fn recipe_sha256(&self) -> &str {
        &self.recipe_sha256
    }

    /// Mint an editing capability from exact recipe authority shape, never a UI boolean.
    /// This capability authorizes strategy/objective mutation only; final raster execution
    /// must still rebuild/revalidate measured or profile-backed execution authority for the
    /// resulting changed recipe.
    pub fn for_recipe(
        recipe: &ConversionRecipe,
        requested: CustomOptimizerStrategyAuthorityKind,
    ) -> Result<Self, String> {
        recipe.validate().map_err(|errors| errors.join(" "))?;
        if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
            return Err(
                "ICC/DeviceLink engines do not consume Shade Editor Custom Optimizer strategy controls."
                    .to_owned(),
            );
        }

        let measured = recipe
            .target
            .characterization_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
        match (measured, requested) {
            (true, CustomOptimizerStrategyAuthorityKind::MeasuredProduction) => {}
            (true, CustomOptimizerStrategyAuthorityKind::ProfileBackedOutputIcc) => {
                return Err(
                    "Measured Custom Optimizer recipe cannot mint profile-backed strategy capability."
                        .to_owned(),
                );
            }
            (false, CustomOptimizerStrategyAuthorityKind::MeasuredProduction) => {
                return Err(
                    "Profile-backed Custom Optimizer recipe cannot mint measured strategy capability."
                        .to_owned(),
                );
            }
            (false, CustomOptimizerStrategyAuthorityKind::ProfileBackedOutputIcc) => {
                let identity = recipe.target.output_profile_identity.as_ref().ok_or_else(|| {
                    "Profile-backed strategy capability requires an exact Output ICC identity."
                        .to_owned()
                })?;
                if identity.sha256.len() != 64
                    || !identity.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err(
                        "Profile-backed strategy capability requires a SHA-256 Output ICC identity."
                            .to_owned(),
                    );
                }
                if recipe
                    .target
                    .output_profile_path
                    .as_deref()
                    .is_none_or(|path| path.trim().is_empty())
                {
                    return Err(
                        "Profile-backed strategy capability requires the exact Output ICC path."
                            .to_owned(),
                    );
                }
            }
        }

        Ok(Self {
            kind: requested,
            recipe_sha256: recipe_sha256(recipe)?,
        })
    }

    pub fn validate_for_recipe(&self, recipe: &ConversionRecipe) -> Result<(), String> {
        let expected = Self::for_recipe(recipe, self.kind)?;
        if self.recipe_sha256 == expected.recipe_sha256 {
            Ok(())
        } else {
            Err(
                "Custom Optimizer strategy capability is stale because the immutable recipe changed."
                    .to_owned(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
    use crate::model::IccProfileIdentity;

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn profile_recipe() -> ConversionRecipe {
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: hash('a'),
            },
            source_transparency_policy: None,
            target: ConversionTargetDefinition {
                name: "Output-backed".to_owned(),
                channels: ["Blue", "Brown", "Beige", "Black"]
                    .into_iter()
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: Some(1.0),
                    })
                    .collect(),
                bit_depth: 16,
                output_profile_identity: Some(IccProfileIdentity {
                    description: "Output".to_owned(),
                    sha256: hash('b'),
                }),
                output_profile_path: Some(r"C:\Color\Output.icc".to_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: Some(4.0),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
        }
    }

    #[test]
    fn profile_backed_capability_is_minted_without_measurement() {
        let recipe = profile_recipe();
        let capability = CustomOptimizerStrategyCapability::for_recipe(
            &recipe,
            CustomOptimizerStrategyAuthorityKind::ProfileBackedOutputIcc,
        )
        .unwrap();
        assert_eq!(
            capability.kind(),
            CustomOptimizerStrategyAuthorityKind::ProfileBackedOutputIcc
        );
        assert_eq!(capability.recipe_sha256(), recipe_sha256(&recipe).unwrap());
        assert!(capability.validate_for_recipe(&recipe).is_ok());
    }

    #[test]
    fn profile_backed_recipe_cannot_fake_measured_capability() {
        let error = CustomOptimizerStrategyCapability::for_recipe(
            &profile_recipe(),
            CustomOptimizerStrategyAuthorityKind::MeasuredProduction,
        )
        .unwrap_err();
        assert!(error.contains("cannot mint measured"));
    }

    #[test]
    fn capability_becomes_stale_after_strategy_change() {
        let recipe = profile_recipe();
        let capability = CustomOptimizerStrategyCapability::for_recipe(
            &recipe,
            CustomOptimizerStrategyAuthorityKind::ProfileBackedOutputIcc,
        )
        .unwrap();
        let mut changed = recipe;
        changed.strategy.black_channel = Some("Black".to_owned());
        changed.strategy.black_generation_strength = 0.7;
        assert!(capability.validate_for_recipe(&changed).is_err());
    }

    #[test]
    fn standard_icc_cannot_mint_optimizer_strategy_capability() {
        let mut recipe = profile_recipe();
        recipe.engine_mode = ConversionEngineMode::Icc;
        recipe.custom_optimizer_solver = None;
        assert!(CustomOptimizerStrategyCapability::for_recipe(
            &recipe,
            CustomOptimizerStrategyAuthorityKind::ProfileBackedOutputIcc,
        )
        .is_err());
    }
}
