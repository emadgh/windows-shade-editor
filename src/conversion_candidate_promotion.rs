use crate::color_conversion::ConversionRecipe;
use crate::conversion_candidate_comparison::CandidateComparisonSnapshot;
use crate::conversion_candidate_preview::CandidatePreviewResult;
use crate::conversion_recipe::recipe_sha256;

/// Bounded, immutable identity for a Candidate that may later be explicitly
/// promoted to final conversion authority.
///
/// The snapshot retains only the exact validated recipe and the already-bounded
/// comparison analytics/identity. It never clones Candidate raster data.
#[derive(Clone, Debug)]
pub struct CandidatePromotionSnapshot {
    comparison: CandidateComparisonSnapshot,
    recipe: ConversionRecipe,
}

impl CandidatePromotionSnapshot {
    /// Freeze an exact Candidate recipe only when it is cryptographically bound
    /// to the already-rendered Candidate result and the same Source-state identity.
    pub fn from_preview(
        source_state_id: impl Into<String>,
        recipe: &ConversionRecipe,
        result: &CandidatePreviewResult,
    ) -> Result<Self, String> {
        recipe
            .validate()
            .map_err(|errors| format!("Candidate promotion recipe is invalid: {}", errors.join(" ")))?;

        let expected_recipe_sha256 = recipe_sha256(recipe)?;
        if !expected_recipe_sha256.eq_ignore_ascii_case(result.recipe_sha256.trim()) {
            return Err(format!(
                "Candidate promotion recipe identity {} does not match rendered Candidate identity {}.",
                expected_recipe_sha256,
                result.recipe_sha256.trim()
            ));
        }

        let comparison = CandidateComparisonSnapshot::from_preview(source_state_id, result)?;
        if !comparison
            .recipe_sha256
            .eq_ignore_ascii_case(&expected_recipe_sha256)
        {
            return Err(
                "Candidate promotion comparison identity drifted from the exact rendered recipe."
                    .to_owned(),
            );
        }

        Ok(Self {
            comparison,
            recipe: recipe.clone(),
        })
    }

    pub fn source_state_id(&self) -> &str {
        &self.comparison.source_state_id
    }

    pub fn recipe_sha256(&self) -> &str {
        &self.comparison.recipe_sha256
    }

    pub fn comparison(&self) -> &CandidateComparisonSnapshot {
        &self.comparison
    }

    pub fn recipe(&self) -> &ConversionRecipe {
        &self.recipe
    }

    pub fn into_recipe(self) -> ConversionRecipe {
        self.recipe
    }

    /// Re-check that a later rendered result still represents the exact frozen
    /// Source state + recipe identity before an operator-facing promotion action.
    pub fn matches_preview(
        &self,
        source_state_id: &str,
        result: &CandidatePreviewResult,
    ) -> Result<bool, String> {
        let candidate = CandidateComparisonSnapshot::from_preview(source_state_id, result)?;
        Ok(candidate.source_state_id == self.comparison.source_state_id
            && candidate.recipe_sha256.eq_ignore_ascii_case(&self.comparison.recipe_sha256)
            && candidate.width == self.comparison.width
            && candidate.height == self.comparison.height
            && candidate.channel_names == self.comparison.channel_names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRenderingIntent,
        ConversionTargetDefinition, SeparationStrategy, TargetChannelDefinition,
    };
    use crate::conversion_analytics::{
        ChannelUsageStats, ConversionUsageReport, CoveragePercentiles,
    };
    use crate::model::IccProfileIdentity;

    fn identity(description: &str, value: char) -> IccProfileIdentity {
        IccProfileIdentity {
            description: description.to_owned(),
            sha256: value.to_string().repeat(64),
        }
    }

    fn recipe(intent: ConversionRenderingIntent) -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: identity("Source RGB", 'a'),
            target: ConversionTargetDefinition {
                name: "Ceramic Black".to_owned(),
                channels: vec![TargetChannelDefinition {
                    name: "Black".to_owned(),
                    display_rgb: Some([0, 0, 0]),
                    solidity: 1.0,
                    max_coverage: None,
                }],
                bit_depth: 16,
                output_profile_identity: Some(identity("Output ICC", 'b')),
                output_profile_path: Some(r"C:\Color\Output.icc".to_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: None,
            },
            rendering_intent: intent,
            black_point_compensation: true,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: None,
        }
    }

    fn usage(mean: f32) -> ConversionUsageReport {
        ConversionUsageReport {
            pixel_count: 2,
            channels: vec![ChannelUsageStats {
                name: "Black".to_owned(),
                mean_coverage: mean,
                peak_coverage: 1.0,
                percentiles: CoveragePercentiles {
                    p50: mean,
                    p95: 0.9,
                    p99: 0.98,
                },
                nonzero_percent: 50.0,
                limit_hit_percent: None,
                integrated_coverage: f64::from(mean) * 2.0,
            }],
            mean_total_ink: mean,
            peak_total_ink: 1.0,
            total_ink_percentiles: CoveragePercentiles {
                p50: mean,
                p95: 0.9,
                p99: 0.98,
            },
            total_ink_limit_hit_percent: None,
            neutral_black_share: None,
        }
    }

    fn preview(recipe: &ConversionRecipe, mean: f32) -> CandidatePreviewResult {
        CandidatePreviewResult {
            width: 2,
            height: 1,
            recipe_sha256: recipe_sha256(recipe).unwrap(),
            channels: recipe.target.channels.clone(),
            planes: vec![vec![0, u16::MAX]],
            histograms: vec![[0; 256]],
            usage: usage(mean),
        }
    }

    #[test]
    fn promotion_freezes_exact_recipe_and_bounded_candidate_identity() {
        let recipe = recipe(ConversionRenderingIntent::RelativeColorimetric);
        let result = preview(&recipe, 0.5);
        let frozen = CandidatePromotionSnapshot::from_preview("source-state-1", &recipe, &result)
            .expect("exact rendered Candidate can be frozen for promotion");

        assert_eq!(frozen.source_state_id(), "source-state-1");
        assert_eq!(frozen.recipe_sha256(), result.recipe_sha256);
        assert_eq!(
            recipe_sha256(frozen.recipe()).unwrap(),
            result.recipe_sha256
        );
        assert!(frozen.matches_preview("source-state-1", &result).unwrap());

        let source = include_str!("conversion_candidate_promotion.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(!runtime.contains("result.planes"));
        assert!(!runtime.contains("CandidatePreviewResult::clone"));
    }

    #[test]
    fn promotion_rejects_recipe_that_did_not_render_the_candidate() {
        let rendered_recipe = recipe(ConversionRenderingIntent::RelativeColorimetric);
        let promoted_recipe = recipe(ConversionRenderingIntent::Perceptual);
        let result = preview(&rendered_recipe, 0.4);

        let error = CandidatePromotionSnapshot::from_preview(
            "source-state-1",
            &promoted_recipe,
            &result,
        )
        .unwrap_err();
        assert!(error.contains("does not match rendered Candidate identity"));
    }

    #[test]
    fn promotion_rejects_empty_source_identity_through_bounded_snapshot_contract() {
        let recipe = recipe(ConversionRenderingIntent::RelativeColorimetric);
        let result = preview(&recipe, 0.4);
        let error = CandidatePromotionSnapshot::from_preview("", &recipe, &result).unwrap_err();
        assert!(error.contains("non-empty Source-state identity"));
    }

    #[test]
    fn frozen_promotion_does_not_match_a_different_source_state_or_recipe() {
        let baseline_recipe = recipe(ConversionRenderingIntent::RelativeColorimetric);
        let result = preview(&baseline_recipe, 0.4);
        let frozen = CandidatePromotionSnapshot::from_preview("source-a", &baseline_recipe, &result)
            .unwrap();
        assert!(!frozen.matches_preview("source-b", &result).unwrap());

        let different = recipe(ConversionRenderingIntent::Perceptual);
        let different_result = preview(&different, 0.4);
        assert!(!frozen
            .matches_preview("source-a", &different_result)
            .unwrap());
    }
}
