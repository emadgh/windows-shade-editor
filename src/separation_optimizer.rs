use std::collections::BTreeMap;

use crate::color_conversion::{ConversionTargetDefinition, SeparationStrategy};

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
    pub chroma: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CandidateEvaluation {
    pub score: f32,
    pub total_ink: f32,
    pub black_coverage: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CandidateRejection {
    ChannelCountMismatch { expected: usize, actual: usize },
    InvalidCoverage { channel: String, value: f32 },
    ChannelLimitExceeded { channel: String, value: f32, limit: f32 },
    TotalInkLimitExceeded { value: f32, limit: f32 },
    ColorErrorExceeded { value: f32, limit: f32 },
    UnknownBlackChannel(String),
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

    if let Some(limit) = effective_total_ink_limit(target.total_ink_limit, strategy.total_ink_limit) {
        if total_ink > limit {
            return Err(CandidateRejection::TotalInkLimitExceeded { value: total_ink, limit });
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
                (index, strategy.per_ink_bias.get(&channel.name).copied().unwrap_or(0.0)),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut score = candidate.delta_e00.max(0.0) * weights.color_error;
    score += total_ink * weights.total_ink;
    for (_name, (index, bias)) in bias_map {
        score -= bias * candidate.coverages[index] * weights.ink_preference;
    }

    if candidate.chroma <= strategy.neutral_chroma_threshold {
        if let Some(coverage) = black_coverage {
            let usable_black = coverage.min(strategy.black_max.max(0.0));
            score -= usable_black
                * strategy.black_generation_strength.clamp(0.0, 1.0)
                * weights.neutral_black;
        }
    }

    Ok(CandidateEvaluation {
        score,
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
            device_link_identity: None,
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

    #[test]
    fn hard_delta_e_constraint_rejects_visible_compromise() {
        let candidate = SeparationCandidate {
            coverages: vec![0.05, 0.02, 0.02, 0.50],
            delta_e00: 4.2,
            chroma: 1.0,
        };
        assert!(matches!(
            evaluate_candidate(&target(), &balanced_strategy(), weights(10.0, 1.0, 1.0), &candidate),
            Err(CandidateRejection::ColorErrorExceeded { .. })
        ));
    }

    #[test]
    fn balanced_prefers_more_accurate_candidate() {
        let candidates = vec![
            SeparationCandidate { coverages: vec![0.30, 0.15, 0.25, 0.05], delta_e00: 0.30, chroma: 2.0 },
            SeparationCandidate { coverages: vec![0.10, 0.05, 0.08, 0.32], delta_e00: 0.80, chroma: 2.0 },
        ];
        let winner = choose_best_candidate(&target(), &balanced_strategy(), weights(10.0, 0.0, 0.0), &candidates).unwrap();
        assert_eq!(winner.0, 0);
    }

    #[test]
    fn black_focused_can_choose_black_heavier_neutral_within_limits() {
        let candidates = vec![
            SeparationCandidate { coverages: vec![0.30, 0.15, 0.25, 0.05], delta_e00: 0.30, chroma: 2.0 },
            SeparationCandidate { coverages: vec![0.10, 0.05, 0.08, 0.32], delta_e00: 0.80, chroma: 2.0 },
        ];
        let mut strategy = balanced_strategy();
        strategy.preset_name = "Black-focused".to_owned();
        strategy.black_channel = Some("Black".to_owned());
        strategy.black_generation_strength = 1.0;
        strategy.black_max = 0.7;
        strategy.neutral_chroma_threshold = 8.0;
        strategy.per_ink_bias.insert("Blue".to_owned(), -0.8);
        strategy.per_ink_bias.insert("Black".to_owned(), 0.9);

        let winner = choose_best_candidate(&target(), &strategy, weights(2.0, 5.0, 8.0), &candidates).unwrap();
        assert_eq!(winner.0, 1);
        assert_eq!(winner.1.black_coverage, Some(0.32));
    }

    #[test]
    fn black_focus_does_not_reward_high_chroma_colors() {
        let candidates = vec![
            SeparationCandidate { coverages: vec![0.30, 0.15, 0.25, 0.05], delta_e00: 0.30, chroma: 30.0 },
            SeparationCandidate { coverages: vec![0.10, 0.05, 0.08, 0.32], delta_e00: 0.80, chroma: 30.0 },
        ];
        let mut strategy = balanced_strategy();
        strategy.black_channel = Some("Black".to_owned());
        strategy.black_generation_strength = 1.0;
        strategy.neutral_chroma_threshold = 8.0;
        let winner = choose_best_candidate(&target(), &strategy, weights(10.0, 0.0, 100.0), &candidates).unwrap();
        assert_eq!(winner.0, 0);
    }

    #[test]
    fn total_and_channel_limits_are_hard_constraints() {
        let per_channel = SeparationCandidate { coverages: vec![0.81, 0.10, 0.10, 0.10], delta_e00: 0.2, chroma: 1.0 };
        assert!(matches!(
            evaluate_candidate(&target(), &balanced_strategy(), weights(1.0, 1.0, 1.0), &per_channel),
            Err(CandidateRejection::ChannelLimitExceeded { .. })
        ));
        let total = SeparationCandidate { coverages: vec![0.50, 0.50, 0.40, 0.20], delta_e00: 0.2, chroma: 1.0 };
        assert!(matches!(
            evaluate_candidate(&target(), &balanced_strategy(), weights(1.0, 1.0, 1.0), &total),
            Err(CandidateRejection::TotalInkLimitExceeded { .. })
        ));
    }
}
