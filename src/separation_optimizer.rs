use std::collections::BTreeMap;

use crate::color_conversion::{ConversionTargetDefinition, SeparationStrategy};
use crate::device_characterization::{DeviceForwardModel, LabColor, evaluate_characterized_color};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CandidateScoringWeights {
    pub color_error: f32,
    pub ink_preference: f32,
    pub neutral_black: f32,
    pub total_ink: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeparationCandidate {
    pub coverages: Vec<f32>,
    pub delta_e00: f32,
    /// Chroma of the requested PCS/Lab target, not a value inferred from inks.
    pub chroma: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CandidateEvaluation {
    /// Full weighted objective retained for diagnostics/backward-compatible ranking.
    pub score: f32,
    /// Color-only contribution to `score`.
    pub color_error_score: f32,
    /// Non-color production preference contribution: total ink, per-ink bias,
    /// and neutral-Black preference. The inverse solver may rank this component
    /// only inside its explicit color-equivalence window.
    pub preference_score: f32,
    pub total_ink: f32,
    pub black_coverage: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CandidateRejection {
    ChannelCountMismatch {
        expected: usize,
        actual: usize,
    },
    InvalidCoverage {
        channel: String,
        value: f32,
    },
    ChannelLimitExceeded {
        channel: String,
        value: f32,
        limit: f32,
    },
    TotalInkLimitExceeded {
        value: f32,
        limit: f32,
    },
    ColorErrorExceeded {
        value: f32,
        limit: f32,
    },
    UnknownBlackChannel(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CharacterizedCandidateError {
    MissingTargetCharacterization,
    CharacterizationIdentityMismatch {
        target: String,
        model: String,
    },
    ChannelTopologyMismatch {
        target: Vec<String>,
        model: Vec<String>,
    },
    InvalidTargetLab,
    ForwardModel(String),
}

/// Build a separation candidate from an authoritative characterized forward
/// model instead of accepting caller-supplied color error or neutrality data.
///
/// The target characterization identity and exact channel order must match the
/// model. This prevents a measured model for one ceramic ink set/RIP state from
/// being silently reused for another topology. Neutral classification uses the
/// requested PCS color's chroma, so an ink vector cannot make itself eligible
/// for the neutral-Black reward.
pub fn characterize_candidate(
    target: &ConversionTargetDefinition,
    model: &dyn DeviceForwardModel,
    target_lab: LabColor,
    coverages: Vec<f32>,
) -> Result<SeparationCandidate, CharacterizedCandidateError> {
    let target_characterization = target
        .characterization_id
        .as_deref()
        .filter(|identity| !identity.trim().is_empty())
        .ok_or(CharacterizedCandidateError::MissingTargetCharacterization)?;

    if target_characterization != model.identity().id {
        return Err(
            CharacterizedCandidateError::CharacterizationIdentityMismatch {
                target: target_characterization.to_owned(),
                model: model.identity().id.clone(),
            },
        );
    }

    let target_channels = target
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect::<Vec<_>>();
    if target_channels != model.identity().channel_names {
        return Err(CharacterizedCandidateError::ChannelTopologyMismatch {
            target: target_channels,
            model: model.identity().channel_names.clone(),
        });
    }

    let chroma = target_lab.chroma();
    if !chroma.is_finite() {
        return Err(CharacterizedCandidateError::InvalidTargetLab);
    }

    let evaluation = evaluate_characterized_color(model, target_lab, &coverages)
        .map_err(CharacterizedCandidateError::ForwardModel)?;
    if !evaluation.delta_e00.is_finite() || evaluation.delta_e00 > f64::from(f32::MAX) {
        return Err(CharacterizedCandidateError::ForwardModel(
            "Characterization produced an invalid CIEDE2000 value.".to_owned(),
        ));
    }

    Ok(SeparationCandidate {
        coverages,
        delta_e00: evaluation.delta_e00 as f32,
        chroma: chroma as f32,
    })
}

/// Reject production-invalid candidates and rank the remaining characterized
/// N-ink alternatives. This never multiplies or edits channels after conversion.
pub fn evaluate_candidate(
    target: &ConversionTargetDefinition,
    strategy: &SeparationStrategy,
    weights: CandidateScoringWeights,
    candidate: &SeparationCandidate,
) -> Result<CandidateEvaluation, CandidateRejection> {
    if candidate.coverages.len() != target.channels.len() {
        return Err(CandidateRejection::ChannelCountMismatch {
            expected: target.channels.len(),
            actual: candidate.coverages.len(),
        });
    }

    let mut total_ink = 0.0f32;
    for (index, value) in candidate.coverages.iter().copied().enumerate() {
        let channel = &target.channels[index];
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(CandidateRejection::InvalidCoverage {
                channel: channel.name.clone(),
                value,
            });
        }
        if let Some(limit) = channel.max_coverage {
            if value > limit {
                return Err(CandidateRejection::ChannelLimitExceeded {
                    channel: channel.name.clone(),
                    value,
                    limit,
                });
            }
        }
        total_ink += value;
    }

    if let Some(limit) = effective_total_ink_limit(target.total_ink_limit, strategy.total_ink_limit)
    {
        if total_ink > limit {
            return Err(CandidateRejection::TotalInkLimitExceeded {
                value: total_ink,
                limit,
            });
        }
    }
    if let Some(limit) = strategy.max_delta_e00 {
        if candidate.delta_e00 > limit {
            return Err(CandidateRejection::ColorErrorExceeded {
                value: candidate.delta_e00,
                limit,
            });
        }
    }

    let black_index = strategy
        .black_channel
        .as_deref()
        .map(|name| {
            target
                .channels
                .iter()
                .position(|channel| channel.name == name)
                .ok_or_else(|| CandidateRejection::UnknownBlackChannel(name.to_owned()))
        })
        .transpose()?;
    let black_coverage = black_index.map(|index| candidate.coverages[index]);

    let bias_map = target
        .channels
        .iter()
        .enumerate()
        .map(|(index, channel)| {
            (
                channel.name.as_str(),
                (
                    index,
                    strategy
                        .per_ink_bias
                        .get(&channel.name)
                        .copied()
                        .unwrap_or(0.0),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let color_error_score = candidate.delta_e00.max(0.0) * weights.color_error;
    let mut preference_score = total_ink * weights.total_ink;
    for (_name, (index, bias)) in bias_map {
        preference_score -= bias * candidate.coverages[index] * weights.ink_preference;
    }

    if candidate.chroma <= strategy.neutral_chroma_threshold {
        if let Some(coverage) = black_coverage {
            let usable_black = coverage.min(strategy.black_max.max(0.0));
            preference_score -= usable_black
                * strategy.black_generation_strength.clamp(0.0, 1.0)
                * weights.neutral_black;
        }
    }
    let score = color_error_score + preference_score;

    Ok(CandidateEvaluation {
        score,
        color_error_score,
        preference_score,
        total_ink,
        black_coverage,
    })
}

pub fn choose_best_candidate<'a>(
    target: &ConversionTargetDefinition,
    strategy: &SeparationStrategy,
    weights: CandidateScoringWeights,
    candidates: &'a [SeparationCandidate],
) -> Option<(usize, CandidateEvaluation, &'a SeparationCandidate)> {
    candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            evaluate_candidate(target, strategy, weights, candidate)
                .ok()
                .map(|evaluation| (index, evaluation, candidate))
        })
        .min_by(|left, right| {
            left.1
                .score
                .partial_cmp(&right.1.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

fn effective_total_ink_limit(target: Option<f32>, strategy: Option<f32>) -> Option<f32> {
    match (target, strategy) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::TargetChannelDefinition;
    use crate::device_characterization::CharacterizationIdentity;

    fn target() -> ConversionTargetDefinition {
        ConversionTargetDefinition {
            name: "Ceramic neutral test".to_owned(),
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
            characterization_id: Some("fixture".to_owned()),
            total_ink_limit: Some(1.5),
        }
    }

    fn balanced_strategy() -> SeparationStrategy {
        SeparationStrategy {
            max_delta_e00: Some(2.0),
            ..SeparationStrategy::default()
        }
    }

    fn weights(color: f32, bias: f32, black: f32) -> CandidateScoringWeights {
        CandidateScoringWeights {
            color_error: color,
            ink_preference: bias,
            neutral_black: black,
            total_ink: 0.0,
        }
    }

    struct SyntheticModel {
        identity: CharacterizationIdentity,
    }

    impl SyntheticModel {
        fn matching() -> Self {
            Self {
                identity: CharacterizationIdentity {
                    id: "fixture".to_owned(),
                    channel_names: vec![
                        "Blue".to_owned(),
                        "Brown".to_owned(),
                        "Beige".to_owned(),
                        "Black".to_owned(),
                    ],
                },
            }
        }
    }

    impl DeviceForwardModel for SyntheticModel {
        fn identity(&self) -> &CharacterizationIdentity {
            &self.identity
        }

        fn predict_lab(&self, coverages: &[f32]) -> Result<LabColor, String> {
            let total = coverages.iter().map(|value| f64::from(*value)).sum::<f64>();
            Ok(LabColor {
                l: 90.0 - 35.0 * total,
                a: 0.5,
                b: -0.5,
            })
        }
    }

    #[test]
    fn characterized_candidate_derives_delta_e_and_target_chroma() {
        let target_lab = LabColor {
            l: 60.0,
            a: 3.0,
            b: 4.0,
        };
        let candidate = characterize_candidate(
            &target(),
            &SyntheticModel::matching(),
            target_lab,
            vec![0.2, 0.1, 0.1, 0.2],
        )
        .unwrap();

        assert!(candidate.delta_e00.is_finite());
        assert!(candidate.delta_e00 > 0.0);
        assert_eq!(candidate.chroma, 5.0);
    }

    #[test]
    fn characterized_candidate_rejects_wrong_characterization_identity() {
        let mut model = SyntheticModel::matching();
        model.identity.id = "other-printer-state".to_owned();
        let error = characterize_candidate(
            &target(),
            &model,
            LabColor {
                l: 60.0,
                a: 0.0,
                b: 0.0,
            },
            vec![0.2, 0.1, 0.1, 0.2],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            CharacterizedCandidateError::CharacterizationIdentityMismatch { .. }
        ));
    }

    #[test]
    fn characterized_candidate_rejects_reordered_ink_topology() {
        let mut model = SyntheticModel::matching();
        model.identity.channel_names.swap(0, 3);
        let error = characterize_candidate(
            &target(),
            &model,
            LabColor {
                l: 60.0,
                a: 0.0,
                b: 0.0,
            },
            vec![0.2, 0.1, 0.1, 0.2],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            CharacterizedCandidateError::ChannelTopologyMismatch { .. }
        ));
    }

    #[test]
    fn characterized_candidate_rejects_invalid_coverage_before_scoring() {
        let error = characterize_candidate(
            &target(),
            &SyntheticModel::matching(),
            LabColor {
                l: 60.0,
                a: 0.0,
                b: 0.0,
            },
            vec![0.2, 0.1, 0.1, 1.1],
        )
        .unwrap_err();
        assert!(matches!(
            error,
            CharacterizedCandidateError::ForwardModel(_)
        ));
    }

    #[test]
    fn hard_delta_e_constraint_rejects_visible_compromise() {
        let candidate = SeparationCandidate {
            coverages: vec![0.05, 0.02, 0.02, 0.50],
            delta_e00: 4.2,
            chroma: 1.0,
        };
        assert!(matches!(
            evaluate_candidate(
                &target(),
                &balanced_strategy(),
                weights(10.0, 1.0, 1.0),
                &candidate
            ),
            Err(CandidateRejection::ColorErrorExceeded { .. })
        ));
    }

    #[test]
    fn balanced_prefers_more_accurate_candidate() {
        let candidates = vec![
            SeparationCandidate {
                coverages: vec![0.30, 0.15, 0.25, 0.05],
                delta_e00: 0.30,
                chroma: 2.0,
            },
            SeparationCandidate {
                coverages: vec![0.10, 0.05, 0.08, 0.32],
                delta_e00: 0.80,
                chroma: 2.0,
            },
        ];
        let winner = choose_best_candidate(
            &target(),
            &balanced_strategy(),
            weights(10.0, 0.0, 0.0),
            &candidates,
        )
        .unwrap();
        assert_eq!(winner.0, 0);
    }

    #[test]
    fn black_focused_can_choose_black_heavier_neutral_within_limits() {
        let candidates = vec![
            SeparationCandidate {
                coverages: vec![0.30, 0.15, 0.25, 0.05],
                delta_e00: 0.30,
                chroma: 2.0,
            },
            SeparationCandidate {
                coverages: vec![0.10, 0.05, 0.08, 0.32],
                delta_e00: 0.80,
                chroma: 2.0,
            },
        ];
        let mut strategy = balanced_strategy();
        strategy.preset_name = "Black-focused".to_owned();
        strategy.black_channel = Some("Black".to_owned());
        strategy.black_generation_strength = 1.0;
        strategy.black_max = 0.7;
        strategy.neutral_chroma_threshold = 8.0;
        strategy.per_ink_bias.insert("Blue".to_owned(), -0.8);
        strategy.per_ink_bias.insert("Black".to_owned(), 0.9);

        let winner =
            choose_best_candidate(&target(), &strategy, weights(2.0, 5.0, 8.0), &candidates)
                .unwrap();
        assert_eq!(winner.0, 1);
        assert_eq!(winner.1.black_coverage, Some(0.32));
    }

    #[test]
    fn black_focus_does_not_reward_high_chroma_colors() {
        let candidates = vec![
            SeparationCandidate {
                coverages: vec![0.30, 0.15, 0.25, 0.05],
                delta_e00: 0.30,
                chroma: 30.0,
            },
            SeparationCandidate {
                coverages: vec![0.10, 0.05, 0.08, 0.32],
                delta_e00: 0.80,
                chroma: 30.0,
            },
        ];
        let mut strategy = balanced_strategy();
        strategy.black_channel = Some("Black".to_owned());
        strategy.black_generation_strength = 1.0;
        strategy.neutral_chroma_threshold = 8.0;
        let winner =
            choose_best_candidate(&target(), &strategy, weights(10.0, 0.0, 100.0), &candidates)
                .unwrap();
        assert_eq!(winner.0, 0);
    }

    #[test]
    fn total_and_channel_limits_are_hard_constraints() {
        let per_channel = SeparationCandidate {
            coverages: vec![0.81, 0.10, 0.10, 0.10],
            delta_e00: 0.2,
            chroma: 1.0,
        };
        assert!(matches!(
            evaluate_candidate(
                &target(),
                &balanced_strategy(),
                weights(1.0, 1.0, 1.0),
                &per_channel
            ),
            Err(CandidateRejection::ChannelLimitExceeded { .. })
        ));
        let total = SeparationCandidate {
            coverages: vec![0.50, 0.50, 0.40, 0.20],
            delta_e00: 0.2,
            chroma: 1.0,
        };
        assert!(matches!(
            evaluate_candidate(
                &target(),
                &balanced_strategy(),
                weights(1.0, 1.0, 1.0),
                &total
            ),
            Err(CandidateRejection::TotalInkLimitExceeded { .. })
        ));
    }
}
