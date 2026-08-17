use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::color_conversion::{ConversionTargetDefinition, SeparationStrategy};
use crate::device_characterization::{DeviceForwardModel, LabColor};
use crate::separation_optimizer::{
    CandidateEvaluation, CandidateScoringWeights, SeparationCandidate, characterize_candidate,
    evaluate_candidate,
};

const HALTON_PRIMES: [u32; 12] = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37];

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct InverseSolverConfig {
    /// Deterministic low-discrepancy samples used for broad device-space search.
    pub initial_samples: usize,
    /// Number of best candidates retained between refinement rounds.
    pub beam_width: usize,
    /// Number of local coordinate/pair-transfer refinement passes.
    pub refinement_rounds: usize,
    /// First local step as a fraction of each channel's allowed coverage range.
    pub initial_step_fraction: f32,
    /// Multiplicative step reduction after every refinement round.
    pub step_decay: f32,
    /// Candidate color differences within this CIEDE2000 distance from the
    /// best feasible color found in the current search stage are treated as
    /// colorimetrically equivalent for production-preference ranking.
    /// Set to 0 for strict minimum-DeltaE ranking.
    pub preference_delta_e00: f32,
}

impl Default for InverseSolverConfig {
    fn default() -> Self {
        Self {
            initial_samples: 384,
            beam_width: 24,
            refinement_rounds: 4,
            initial_step_fraction: 0.18,
            step_decay: 0.5,
            preference_delta_e00: 0.10,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InverseSolverStats {
    pub attempted: usize,
    pub characterized: usize,
    pub feasible: usize,
    pub forward_rejected: usize,
    pub constraint_rejected: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InverseSolveResult {
    pub candidate: SeparationCandidate,
    pub evaluation: CandidateEvaluation,
    pub stats: InverseSolverStats,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InverseSolveError {
    InvalidConfiguration(Vec<String>),
    MissingTargetCharacterization,
    CharacterizationIdentityMismatch { target: String, model: String },
    ChannelTopologyMismatch { target: Vec<String>, model: Vec<String> },
    NoFeasibleCandidate,
}

#[derive(Clone, Debug)]
struct RankedCandidate {
    candidate: SeparationCandidate,
    evaluation: CandidateEvaluation,
    balance_penalty: f32,
    key: Vec<u16>,
}

/// Deterministic bounded reference inverse solver for the Custom Optimizer path.
///
/// This is deliberately a reference/search implementation, not the final raster
/// engine. It samples device space with a Halton sequence and then refines a small
/// beam of feasible solutions with coordinate and pair-transfer moves. Every
/// candidate is evaluated through the authoritative `DeviceForwardModel` and the
/// same production separation constraints used by `separation_optimizer`.
///
/// Unsupported forward-model regions fail closed. The algorithm never edits a
/// completed ICC/DeviceLink separation and never infers ink behavior from display
/// colors.
pub fn solve_inverse_separation(
    target: &ConversionTargetDefinition,
    strategy: &SeparationStrategy,
    weights: CandidateScoringWeights,
    model: &dyn DeviceForwardModel,
    target_lab: LabColor,
    config: InverseSolverConfig,
) -> Result<InverseSolveResult, InverseSolveError> {
    validate_identity(target, model)?;
    let config_errors = validate_config(config, target.channels.len());
    if !config_errors.is_empty() {
        return Err(InverseSolveError::InvalidConfiguration(config_errors));
    }

    let maxima = channel_maxima(target, strategy);
    let total_limit = effective_total_ink_limit(target.total_ink_limit, strategy.total_ink_limit);
    let mut stats = InverseSolverStats::default();
    let mut seen = BTreeSet::new();
    let mut ranked = Vec::new();

    for seed in canonical_seeds(&maxima, total_limit, target, strategy) {
        try_candidate(
            target,
            strategy,
            weights,
            model,
            target_lab,
            seed,
            &mut stats,
            &mut seen,
            &mut ranked,
        );
    }

    for sample_index in 1..=config.initial_samples {
        let mut coverages = maxima
            .iter()
            .enumerate()
            .map(|(channel_index, maximum)| {
                let base = HALTON_PRIMES[channel_index];
                (halton(sample_index as u64, base) as f32) * *maximum
            })
            .collect::<Vec<_>>();
        fit_total_limit(&mut coverages, total_limit);
        try_candidate(
            target,
            strategy,
            weights,
            model,
            target_lab,
            coverages,
            &mut stats,
            &mut seen,
            &mut ranked,
        );
    }

    retain_best(&mut ranked, config.beam_width, config.preference_delta_e00);
    if ranked.is_empty() {
        return Err(InverseSolveError::NoFeasibleCandidate);
    }

    let mut step_fraction = config.initial_step_fraction;
    for _ in 0..config.refinement_rounds {
        let parents = ranked.clone();
        let mut expanded = ranked;

        for parent in &parents {
            for channel in 0..maxima.len() {
                let delta = maxima[channel] * step_fraction;
                for direction in [-1.0f32, 1.0] {
                    let mut next = parent.candidate.coverages.clone();
                    next[channel] = (next[channel] + direction * delta).clamp(0.0, maxima[channel]);
                    fit_total_limit(&mut next, total_limit);
                    try_candidate(
                        target,
                        strategy,
                        weights,
                        model,
                        target_lab,
                        next,
                        &mut stats,
                        &mut seen,
                        &mut expanded,
                    );
                }
            }

            // Pair-transfer moves are important for N-ink degeneracy: they let
            // the solver substitute one ink for another without relying on a
            // lucky global sample. The beam bound keeps the search finite.
            for from in 0..maxima.len() {
                for to in 0..maxima.len() {
                    if from == to {
                        continue;
                    }
                    let transfer = (maxima[from].min(maxima[to]) * step_fraction)
                        .min(parent.candidate.coverages[from]);
                    if transfer <= 0.0 {
                        continue;
                    }
                    let mut next = parent.candidate.coverages.clone();
                    next[from] -= transfer;
                    next[to] = (next[to] + transfer).min(maxima[to]);
                    fit_total_limit(&mut next, total_limit);
                    try_candidate(
                        target,
                        strategy,
                        weights,
                        model,
                        target_lab,
                        next,
                        &mut stats,
                        &mut seen,
                        &mut expanded,
                    );
                }
            }
        }

        retain_best(&mut expanded, config.beam_width, config.preference_delta_e00);
        ranked = expanded;
        step_fraction *= config.step_decay;
    }

    let best = ranked.into_iter().next().ok_or(InverseSolveError::NoFeasibleCandidate)?;
    Ok(InverseSolveResult {
        candidate: best.candidate,
        evaluation: best.evaluation,
        stats,
    })
}

fn validate_identity(
    target: &ConversionTargetDefinition,
    model: &dyn DeviceForwardModel,
) -> Result<(), InverseSolveError> {
    let target_id = target
        .characterization_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(InverseSolveError::MissingTargetCharacterization)?;
    if target_id != model.identity().id {
        return Err(InverseSolveError::CharacterizationIdentityMismatch {
            target: target_id.to_owned(),
            model: model.identity().id.clone(),
        });
    }

    let target_channels = target
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect::<Vec<_>>();
    if target_channels != model.identity().channel_names {
        return Err(InverseSolveError::ChannelTopologyMismatch {
            target: target_channels,
            model: model.identity().channel_names.clone(),
        });
    }
    Ok(())
}

fn validate_config(config: InverseSolverConfig, channel_count: usize) -> Vec<String> {
    let mut errors = Vec::new();
    if !(1..=HALTON_PRIMES.len()).contains(&channel_count) {
        errors.push(format!(
            "Reference inverse solver supports 1..={} channels, got {channel_count}.",
            HALTON_PRIMES.len()
        ));
    }
    if !(32..=16_384).contains(&config.initial_samples) {
        errors.push("Inverse solver initial_samples must be in 32..=16384.".to_owned());
    }
    if !(4..=256).contains(&config.beam_width) {
        errors.push("Inverse solver beam_width must be in 4..=256.".to_owned());
    }
    if config.refinement_rounds > 8 {
        errors.push("Inverse solver refinement_rounds must be <= 8.".to_owned());
    }
    if !config.initial_step_fraction.is_finite()
        || !(0.005..=0.5).contains(&config.initial_step_fraction)
    {
        errors.push("Inverse solver initial_step_fraction must be in 0.005..=0.5.".to_owned());
    }
    if !config.step_decay.is_finite() || !(0.1..=0.95).contains(&config.step_decay) {
        errors.push("Inverse solver step_decay must be in 0.1..=0.95.".to_owned());
    }
    if !config.preference_delta_e00.is_finite()
        || !(0.0..=1.0).contains(&config.preference_delta_e00)
    {
        errors.push(
            "Inverse solver preference_delta_e00 must be finite and in 0..=1.0."
                .to_owned(),
        );
    }
    errors
}

fn channel_maxima(
    target: &ConversionTargetDefinition,
    strategy: &SeparationStrategy,
) -> Vec<f32> {
    target
        .channels
        .iter()
        .map(|channel| {
            let mut maximum = channel.max_coverage.unwrap_or(1.0).clamp(0.0, 1.0);
            if strategy.black_channel.as_deref() == Some(channel.name.as_str()) {
                maximum = maximum.min(strategy.black_max.clamp(0.0, 1.0));
            }
            maximum
        })
        .collect()
}

fn canonical_seeds(
    maxima: &[f32],
    total_limit: Option<f32>,
    target: &ConversionTargetDefinition,
    strategy: &SeparationStrategy,
) -> Vec<Vec<f32>> {
    let mut seeds = Vec::new();
    seeds.push(vec![0.0; maxima.len()]);

    let mut midpoint = maxima.iter().map(|value| value * 0.5).collect::<Vec<_>>();
    fit_total_limit(&mut midpoint, total_limit);
    seeds.push(midpoint);

    for index in 0..maxima.len() {
        for fraction in [0.35f32, 0.7, 1.0] {
            let mut one_hot = vec![0.0; maxima.len()];
            one_hot[index] = maxima[index] * fraction;
            fit_total_limit(&mut one_hot, total_limit);
            seeds.push(one_hot);
        }
    }

    if let Some(black_name) = strategy.black_channel.as_deref() {
        if let Some(index) = target.channels.iter().position(|channel| channel.name == black_name) {
            let mut black_seed = vec![0.0; maxima.len()];
            black_seed[index] = maxima[index] * 0.75;
            fit_total_limit(&mut black_seed, total_limit);
            seeds.push(black_seed);
        }
    }

    let mut bias_seed = Vec::with_capacity(maxima.len());
    for (index, channel) in target.channels.iter().enumerate() {
        let bias = strategy.per_ink_bias.get(&channel.name).copied().unwrap_or(0.0);
        let fraction = (0.5 + 0.35 * bias).clamp(0.05, 0.95);
        bias_seed.push(maxima[index] * fraction);
    }
    fit_total_limit(&mut bias_seed, total_limit);
    seeds.push(bias_seed);
    seeds
}

#[allow(clippy::too_many_arguments)]
fn try_candidate(
    target: &ConversionTargetDefinition,
    strategy: &SeparationStrategy,
    weights: CandidateScoringWeights,
    model: &dyn DeviceForwardModel,
    target_lab: LabColor,
    coverages: Vec<f32>,
    stats: &mut InverseSolverStats,
    seen: &mut BTreeSet<Vec<u16>>,
    ranked: &mut Vec<RankedCandidate>,
) {
    let key = quantized_key(&coverages);
    if !seen.insert(key.clone()) {
        return;
    }
    stats.attempted += 1;

    let candidate = match characterize_candidate(target, model, target_lab, coverages) {
        Ok(candidate) => {
            stats.characterized += 1;
            candidate
        }
        Err(_) => {
            stats.forward_rejected += 1;
            return;
        }
    };

    let evaluation = match evaluate_candidate(target, strategy, weights, &candidate) {
        Ok(evaluation) => {
            stats.feasible += 1;
            evaluation
        }
        Err(_) => {
            stats.constraint_rejected += 1;
            return;
        }
    };

    let maxima = channel_maxima(target, strategy);
    let balance_penalty = normalized_coverage_dispersion(&candidate.coverages, &maxima);
    ranked.push(RankedCandidate {
        candidate,
        evaluation,
        balance_penalty,
        key,
    });
}

fn retain_best(
    ranked: &mut Vec<RankedCandidate>,
    beam_width: usize,
    preference_delta_e00: f32,
) {
    if ranked.is_empty() {
        return;
    }
    let best_delta_e00 = ranked
        .iter()
        .map(|candidate| candidate.candidate.delta_e00)
        .min_by(f32::total_cmp)
        .expect("non-empty candidate list");
    let preference_ceiling = best_delta_e00 + preference_delta_e00;
    ranked.retain(|candidate| candidate.candidate.delta_e00 <= preference_ceiling);
    ranked.sort_by(|left, right| {
        left.evaluation
            .preference_score
            .total_cmp(&right.evaluation.preference_score)
            .then_with(|| left.candidate.delta_e00.total_cmp(&right.candidate.delta_e00))
            .then_with(|| left.evaluation.total_ink.total_cmp(&right.evaluation.total_ink))
            .then_with(|| left.balance_penalty.total_cmp(&right.balance_penalty))
            .then_with(|| left.key.cmp(&right.key))
    });
    ranked.truncate(beam_width);
}

fn quantized_key(coverages: &[f32]) -> Vec<u16> {
    coverages
        .iter()
        .map(|value| (value.clamp(0.0, 1.0) * 65_535.0).round() as u16)
        .collect()
}

fn normalized_coverage_dispersion(coverages: &[f32], maxima: &[f32]) -> f32 {
    let normalized = coverages
        .iter()
        .copied()
        .zip(maxima.iter().copied())
        .filter_map(|(coverage, maximum)| {
            (maximum > 0.0).then_some((coverage / maximum).clamp(0.0, 1.0))
        })
        .collect::<Vec<_>>();
    if normalized.len() <= 1 {
        return 0.0;
    }
    let mean = normalized.iter().sum::<f32>() / normalized.len() as f32;
    normalized
        .iter()
        .map(|value| {
            let delta = *value - mean;
            delta * delta
        })
        .sum::<f32>()
        / normalized.len() as f32
}

fn fit_total_limit(coverages: &mut [f32], limit: Option<f32>) {
    let Some(limit) = limit else {
        return;
    };
    let total = coverages.iter().sum::<f32>();
    if total > limit && total > 0.0 {
        let scale = limit / total;
        for value in coverages {
            *value *= scale;
        }
    }
}

fn effective_total_ink_limit(target: Option<f32>, strategy: Option<f32>) -> Option<f32> {
    match (target, strategy) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn halton(mut index: u64, base: u32) -> f64 {
    let base = u64::from(base);
    let mut fraction = 1.0f64;
    let mut result = 0.0f64;
    while index > 0 {
        fraction /= base as f64;
        result += fraction * (index % base) as f64;
        index /= base;
    }
    result
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::color_conversion::TargetChannelDefinition;
    use crate::device_characterization::CharacterizationIdentity;

    struct DegenerateFourInkModel {
        identity: CharacterizationIdentity,
        black_efficiency: f64,
    }

    impl DegenerateFourInkModel {
        fn new(black_efficiency: f64) -> Self {
            Self {
                identity: CharacterizationIdentity {
                    id: "inverse-fixture".to_owned(),
                    channel_names: ["Blue", "Brown", "Beige", "Black"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                },
                black_efficiency,
            }
        }
    }

    impl DeviceForwardModel for DegenerateFourInkModel {
        fn identity(&self) -> &CharacterizationIdentity {
            &self.identity
        }

        fn predict_lab(&self, coverages: &[f32]) -> Result<LabColor, String> {
            if coverages.len() != 4 {
                return Err("fixture topology mismatch".to_owned());
            }
            if coverages.iter().any(|value| !value.is_finite() || !(0.0..=0.8).contains(value)) {
                return Err("fixture coverage outside measured domain".to_owned());
            }
            let chromatic = coverages[..3]
                .iter()
                .map(|value| f64::from(*value))
                .sum::<f64>();
            let black = f64::from(coverages[3]);
            let darkness = chromatic + black * self.black_efficiency;
            Ok(LabColor {
                l: 95.0 - 40.0 * darkness,
                a: 0.0,
                b: 0.0,
            })
        }
    }

    fn target() -> ConversionTargetDefinition {
        ConversionTargetDefinition {
            name: "Inverse fixture".to_owned(),
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
            characterization_id: Some("inverse-fixture".to_owned()),
            total_ink_limit: Some(1.5),
        }
    }

    fn target_lab() -> LabColor {
        LabColor {
            l: 75.0,
            a: 0.0,
            b: 0.0,
        }
    }

    fn weights(total_ink: f32, bias: f32, black: f32) -> CandidateScoringWeights {
        CandidateScoringWeights {
            color_error: 30.0,
            ink_preference: bias,
            neutral_black: black,
            total_ink,
        }
    }

    fn config() -> InverseSolverConfig {
        InverseSolverConfig {
            initial_samples: 256,
            beam_width: 20,
            refinement_rounds: 4,
            initial_step_fraction: 0.16,
            step_decay: 0.5,
            preference_delta_e00: 0.10,
        }
    }

    #[test]
    fn solver_is_deterministic_and_respects_target_limits() {
        let model = DegenerateFourInkModel::new(1.0);
        let strategy = SeparationStrategy {
            max_delta_e00: Some(1.5),
            ..SeparationStrategy::default()
        };
        let first = solve_inverse_separation(
            &target(),
            &strategy,
            weights(0.2, 0.0, 0.0),
            &model,
            target_lab(),
            config(),
        )
        .unwrap();
        let second = solve_inverse_separation(
            &target(),
            &strategy,
            weights(0.2, 0.0, 0.0),
            &model,
            target_lab(),
            config(),
        )
        .unwrap();

        assert_eq!(first.candidate.coverages, second.candidate.coverages);
        assert_eq!(first.evaluation, second.evaluation);
        assert!(first.candidate.delta_e00 <= 1.5);
        assert!(first.evaluation.total_ink <= 1.5);
        assert!(first.candidate.coverages.iter().all(|value| *value <= 0.8));
    }

    #[test]
    fn black_focused_and_black_avoiding_strategies_find_different_neutral_separations() {
        let model = DegenerateFourInkModel::new(1.0);
        let mut black_focused = SeparationStrategy {
            black_channel: Some("Black".to_owned()),
            black_generation_strength: 1.0,
            black_max: 0.8,
            neutral_chroma_threshold: 8.0,
            max_delta_e00: Some(1.5),
            ..SeparationStrategy::default()
        };
        black_focused.per_ink_bias.insert("Black".to_owned(), 1.0);

        let mut black_avoiding = SeparationStrategy {
            black_channel: Some("Black".to_owned()),
            black_generation_strength: 0.0,
            black_max: 0.8,
            neutral_chroma_threshold: 8.0,
            max_delta_e00: Some(1.5),
            per_ink_bias: BTreeMap::new(),
            ..SeparationStrategy::default()
        };
        black_avoiding.per_ink_bias.insert("Black".to_owned(), -1.0);
        black_avoiding.per_ink_bias.insert("Brown".to_owned(), 0.5);

        let focused = solve_inverse_separation(
            &target(),
            &black_focused,
            weights(0.1, 3.0, 4.0),
            &model,
            target_lab(),
            config(),
        )
        .unwrap();
        let avoiding = solve_inverse_separation(
            &target(),
            &black_avoiding,
            weights(0.1, 3.0, 4.0),
            &model,
            target_lab(),
            config(),
        )
        .unwrap();

        assert!(focused.candidate.delta_e00 <= 1.5);
        assert!(avoiding.candidate.delta_e00 <= 1.5);
        assert!(focused.candidate.coverages[3] > avoiding.candidate.coverages[3] + 0.10);
    }

    #[test]
    fn total_ink_objective_favors_more_efficient_separation() {
        let model = DegenerateFourInkModel::new(1.8);
        let strategy = SeparationStrategy {
            black_channel: Some("Black".to_owned()),
            black_max: 0.8,
            max_delta_e00: Some(2.0),
            ..SeparationStrategy::default()
        };

        let color_only = solve_inverse_separation(
            &target(),
            &strategy,
            weights(0.0, 0.0, 0.0),
            &model,
            target_lab(),
            config(),
        )
        .unwrap();
        let low_ink = solve_inverse_separation(
            &target(),
            &strategy,
            weights(8.0, 0.0, 0.0),
            &model,
            target_lab(),
            config(),
        )
        .unwrap();

        assert!(low_ink.candidate.delta_e00 <= 2.0);
        assert!(low_ink.evaluation.total_ink <= color_only.evaluation.total_ink + 1.0e-5);
    }

    #[test]
    fn negative_ink_bias_can_avoid_blue_without_post_transform_multiplication() {
        let model = DegenerateFourInkModel::new(1.0);
        let baseline = SeparationStrategy {
            max_delta_e00: Some(1.5),
            ..SeparationStrategy::default()
        };
        let mut avoid_blue = baseline.clone();
        avoid_blue.per_ink_bias.insert("Blue".to_owned(), -1.0);
        avoid_blue.per_ink_bias.insert("Brown".to_owned(), 0.8);

        let normal = solve_inverse_separation(
            &target(),
            &baseline,
            weights(0.0, 0.0, 0.0),
            &model,
            target_lab(),
            config(),
        )
        .unwrap();
        let avoided = solve_inverse_separation(
            &target(),
            &avoid_blue,
            weights(0.0, 4.0, 0.0),
            &model,
            target_lab(),
            config(),
        )
        .unwrap();

        assert!(avoided.candidate.delta_e00 <= 1.5);
        assert!(avoided.candidate.coverages[0] + 0.05 < normal.candidate.coverages[0]);
    }

    #[test]
    fn identity_mismatch_fails_before_search() {
        let mut wrong_target = target();
        wrong_target.characterization_id = Some("other-package".to_owned());
        let error = solve_inverse_separation(
            &wrong_target,
            &SeparationStrategy::default(),
            weights(0.0, 0.0, 0.0),
            &DegenerateFourInkModel::new(1.0),
            target_lab(),
            config(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            InverseSolveError::CharacterizationIdentityMismatch { .. }
        ));
    }
}
