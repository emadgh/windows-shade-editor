use std::collections::BTreeSet;

use crate::color_conversion::{ConversionTargetDefinition, SeparationStrategy};
pub use crate::custom_optimizer_config::CustomOptimizerSolverConfig as InverseSolverConfig;
use crate::custom_optimizer_config::{
    ContinuityDistanceMetric, ContinuityPreferenceConfig, CustomOptimizerSolverMethod,
};
use crate::device_characterization::{DeviceForwardModel, LabColor};
use crate::icc_device_forward_model::{
    target_accepts_forward_model_identity, target_forward_model_authorities,
};
use crate::separation_optimizer::{
    CandidateEvaluation, CandidateScoringWeights, SeparationCandidate, characterize_candidate,
    evaluate_candidate,
};

const HALTON_PRIMES: [u32; 12] = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37];

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
    /// Retained for API compatibility; now means no accepted target forward-model
    /// identity exists (neither measured characterization nor Output ICC SHA).
    MissingTargetCharacterization,
    /// Retained for API compatibility; `target` may list both accepted authority
    /// identities when measured and profile-backed models coexist.
    CharacterizationIdentityMismatch {
        target: String,
        model: String,
    },
    ChannelTopologyMismatch {
        target: Vec<String>,
        model: Vec<String>,
    },
    MissingContinuityReference,
    ContinuityReferenceTopologyMismatch {
        expected: usize,
        actual: usize,
    },
    InvalidContinuityReference {
        channel_index: usize,
    },
    ContinuityReferenceTotalInkExceeded,
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
/// A model may be measured-characterization-backed or Output-ICC-backed. Unsupported
/// forward-model regions fail closed. The algorithm never edits a completed
/// ICC/DeviceLink separation and never infers ink behavior from display colors.
pub fn solve_inverse_separation(
    target: &ConversionTargetDefinition,
    strategy: &SeparationStrategy,
    weights: CandidateScoringWeights,
    model: &dyn DeviceForwardModel,
    target_lab: LabColor,
    config: InverseSolverConfig,
) -> Result<InverseSolveResult, InverseSolveError> {
    solve_inverse_separation_with_reference(
        target, strategy, weights, model, target_lab, config, None,
    )
}

#[derive(Clone, Copy)]
struct ContinuityContext<'a> {
    policy: ContinuityPreferenceConfig,
    reference_coverages: &'a [f32],
}

/// V2 entry point with an explicit neighboring/reference separation.
///
/// V1 ignores this reference by contract. V2 with zero continuity weight also
/// bypasses all continuity logic so its candidate ordering is identical to V1.
pub fn solve_inverse_separation_with_reference(
    target: &ConversionTargetDefinition,
    strategy: &SeparationStrategy,
    weights: CandidateScoringWeights,
    model: &dyn DeviceForwardModel,
    target_lab: LabColor,
    config: InverseSolverConfig,
    reference_coverages: Option<&[f32]>,
) -> Result<InverseSolveResult, InverseSolveError> {
    validate_identity(target, model)?;
    config
        .validate(target.channels.len())
        .map_err(InverseSolveError::InvalidConfiguration)?;

    let maxima = channel_maxima(target, strategy);
    let total_limit = effective_total_ink_limit(target.total_ink_limit, strategy.total_ink_limit);
    let continuity = continuity_context(&config, reference_coverages, &maxima, total_limit)?;
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

    retain_best(
        &mut ranked,
        config.beam_width,
        config.preference_delta_e00,
        &maxima,
        continuity,
    );
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

        retain_best(
            &mut expanded,
            config.beam_width,
            config.preference_delta_e00,
            &maxima,
            continuity,
        );
        ranked = expanded;
        step_fraction *= config.step_decay;
    }

    let best = ranked
        .into_iter()
        .next()
        .ok_or(InverseSolveError::NoFeasibleCandidate)?;
    Ok(InverseSolveResult {
        candidate: best.candidate,
        evaluation: best.evaluation,
        stats,
    })
}

fn continuity_context<'a>(
    config: &InverseSolverConfig,
    reference_coverages: Option<&'a [f32]>,
    maxima: &[f32],
    total_limit: Option<f32>,
) -> Result<Option<ContinuityContext<'a>>, InverseSolveError> {
    if config.method != CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2 {
        return Ok(None);
    }
    let policy = config
        .continuity_preference
        .expect("validated V2 config must include continuity policy");
    if policy.weight == 0.0 {
        return Ok(None);
    }

    let reference_coverages =
        reference_coverages.ok_or(InverseSolveError::MissingContinuityReference)?;
    if reference_coverages.len() != maxima.len() {
        return Err(InverseSolveError::ContinuityReferenceTopologyMismatch {
            expected: maxima.len(),
            actual: reference_coverages.len(),
        });
    }
    for (channel_index, (value, maximum)) in reference_coverages
        .iter()
        .copied()
        .zip(maxima.iter().copied())
        .enumerate()
    {
        if !value.is_finite() || value < 0.0 || value > maximum + 1.0e-6 {
            return Err(InverseSolveError::InvalidContinuityReference { channel_index });
        }
    }
    if let Some(limit) = total_limit {
        if reference_coverages.iter().copied().sum::<f32>() > limit + 1.0e-6 {
            return Err(InverseSolveError::ContinuityReferenceTotalInkExceeded);
        }
    }

    Ok(Some(ContinuityContext {
        policy,
        reference_coverages,
    }))
}

fn validate_identity(
    target: &ConversionTargetDefinition,
    model: &dyn DeviceForwardModel,
) -> Result<(), InverseSolveError> {
    let authorities = target_forward_model_authorities(target);
    if authorities.is_empty() {
        return Err(InverseSolveError::MissingTargetCharacterization);
    }
    if !target_accepts_forward_model_identity(target, &model.identity().id) {
        return Err(InverseSolveError::CharacterizationIdentityMismatch {
            target: authorities.join(" | "),
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

fn channel_maxima(target: &ConversionTargetDefinition, strategy: &SeparationStrategy) -> Vec<f32> {
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
        if let Some(index) = target
            .channels
            .iter()
            .position(|channel| channel.name == black_name)
        {
            let mut black_seed = vec![0.0; maxima.len()];
            black_seed[index] = maxima[index] * 0.75;
            fit_total_limit(&mut black_seed, total_limit);
            seeds.push(black_seed);
        }
    }

    let mut bias_seed = Vec::with_capacity(maxima.len());
    for (index, channel) in target.channels.iter().enumerate() {
        let bias = strategy
            .per_ink_bias
            .get(&channel.name)
            .copied()
            .unwrap_or(0.0);
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
    maxima: &[f32],
    continuity: Option<ContinuityContext<'_>>,
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

    if let Some(context) = continuity {
        ranked.sort_by(|left, right| {
            let left_score = left.evaluation.preference_score
                + context.policy.weight * continuity_rank_score(left, maxima, context);
            let right_score = right.evaluation.preference_score
                + context.policy.weight * continuity_rank_score(right, maxima, context);
            left_score
                .total_cmp(&right_score)
                .then_with(|| {
                    left.candidate
                        .delta_e00
                        .total_cmp(&right.candidate.delta_e00)
                })
                .then_with(|| {
                    left.evaluation
                        .total_ink
                        .total_cmp(&right.evaluation.total_ink)
                })
                .then_with(|| left.balance_penalty.total_cmp(&right.balance_penalty))
                .then_with(|| left.key.cmp(&right.key))
        });
    } else {
        // Keep the historical V1 comparator byte-for-byte in semantic ordering.
        ranked.sort_by(|left, right| {
            left.evaluation
                .preference_score
                .total_cmp(&right.evaluation.preference_score)
                .then_with(|| {
                    left.candidate
                        .delta_e00
                        .total_cmp(&right.candidate.delta_e00)
                })
                .then_with(|| {
                    left.evaluation
                        .total_ink
                        .total_cmp(&right.evaluation.total_ink)
                })
                .then_with(|| left.balance_penalty.total_cmp(&right.balance_penalty))
                .then_with(|| left.key.cmp(&right.key))
        });
    }
    ranked.truncate(beam_width);
}

fn continuity_rank_score(
    candidate: &RankedCandidate,
    maxima: &[f32],
    context: ContinuityContext<'_>,
) -> f32 {
    let mut l1 = 0.0f32;
    let mut l2_squared = 0.0f32;
    let mut max_jump = 0.0f32;
    for ((coverage, reference), maximum) in candidate
        .candidate
        .coverages
        .iter()
        .copied()
        .zip(context.reference_coverages.iter().copied())
        .zip(maxima.iter().copied())
    {
        let normalized = if maximum > f32::EPSILON {
            ((coverage - reference).abs() / maximum).clamp(0.0, 1.0)
        } else {
            0.0
        };
        l1 += normalized;
        l2_squared += normalized * normalized;
        max_jump = max_jump.max(normalized);
    }

    let distance = match context.policy.distance_metric {
        ContinuityDistanceMetric::NormalizedL1 => l1,
        ContinuityDistanceMetric::NormalizedL2 => l2_squared.sqrt(),
    };
    let cap_excess = (max_jump - context.policy.max_normalized_channel_jump).max(0.0);
    let dominant_switch_penalty = if dominant_channel(&candidate.candidate.coverages)
        != dominant_channel(context.reference_coverages)
    {
        context.policy.dominant_channel_switch_penalty
    } else {
        0.0
    };

    distance + cap_excess + dominant_switch_penalty
}

fn dominant_channel(coverages: &[f32]) -> Option<usize> {
    let mut best_index = None;
    let mut best_value = f32::EPSILON;
    for (index, value) in coverages.iter().copied().enumerate() {
        if value > best_value {
            best_value = value;
            best_index = Some(index);
        }
    }
    best_index
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
    use crate::model::IccProfileIdentity;

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
            if coverages
                .iter()
                .any(|value| !value.is_finite() || !(0.0..=0.8).contains(value))
            {
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
            ..InverseSolverConfig::default()
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
    fn profile_backed_identity_can_drive_existing_inverse_solver() {
        let mut profile_target = target();
        profile_target.characterization_id = None;
        profile_target.output_profile_identity = Some(IccProfileIdentity {
            description: "Existing ceramic Output ICC".to_owned(),
            sha256: "a".repeat(64),
        });
        profile_target.output_profile_path = Some("existing.icc".to_owned());
        let model = DegenerateFourInkModel {
            identity: CharacterizationIdentity {
                id: "A".repeat(64),
                channel_names: ["Blue", "Brown", "Beige", "Black"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            },
            black_efficiency: 1.0,
        };
        let result = solve_inverse_separation(
            &profile_target,
            &SeparationStrategy {
                max_delta_e00: Some(1.5),
                ..SeparationStrategy::default()
            },
            weights(0.2, 0.0, 0.0),
            &model,
            target_lab(),
            config(),
        )
        .unwrap();
        assert!(result.candidate.delta_e00 <= 1.5);
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

    fn continuity_config(weight: f32) -> InverseSolverConfig {
        InverseSolverConfig {
            method: CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2,
            continuity_preference: Some(ContinuityPreferenceConfig {
                weight,
                distance_metric: ContinuityDistanceMetric::NormalizedL1,
                max_normalized_channel_jump: 0.20,
                dominant_channel_switch_penalty: 0.25,
            }),
            ..config()
        }
    }

    fn normalized_l1_to_reference(coverages: &[f32], reference: &[f32]) -> f32 {
        coverages
            .iter()
            .copied()
            .zip(reference.iter().copied())
            .map(|(left, right)| (left - right).abs() / 0.8)
            .sum()
    }

    #[test]
    fn zero_continuity_weight_reproduces_v1_without_reference() {
        let model = DegenerateFourInkModel::new(1.0);
        let strategy = SeparationStrategy {
            max_delta_e00: Some(1.5),
            ..SeparationStrategy::default()
        };
        let baseline = solve_inverse_separation(
            &target(),
            &strategy,
            weights(0.2, 0.0, 0.0),
            &model,
            target_lab(),
            config(),
        )
        .unwrap();
        let continuity = solve_inverse_separation_with_reference(
            &target(),
            &strategy,
            weights(0.2, 0.0, 0.0),
            &model,
            target_lab(),
            continuity_config(0.0),
            None,
        )
        .unwrap();

        assert_eq!(baseline, continuity);
    }

    #[test]
    fn positive_continuity_weight_requires_explicit_valid_reference() {
        let model = DegenerateFourInkModel::new(1.0);
        let strategy = SeparationStrategy::default();
        let missing = solve_inverse_separation_with_reference(
            &target(),
            &strategy,
            weights(0.0, 0.0, 0.0),
            &model,
            target_lab(),
            continuity_config(1.0),
            None,
        )
        .unwrap_err();
        assert_eq!(missing, InverseSolveError::MissingContinuityReference);

        let topology = solve_inverse_separation_with_reference(
            &target(),
            &strategy,
            weights(0.0, 0.0, 0.0),
            &model,
            target_lab(),
            continuity_config(1.0),
            Some(&[0.2, 0.2]),
        )
        .unwrap_err();
        assert!(matches!(
            topology,
            InverseSolveError::ContinuityReferenceTopologyMismatch { .. }
        ));

        let total_ink = solve_inverse_separation_with_reference(
            &target(),
            &strategy,
            weights(0.0, 0.0, 0.0),
            &model,
            target_lab(),
            continuity_config(1.0),
            Some(&[0.8, 0.8, 0.8, 0.8]),
        )
        .unwrap_err();
        assert_eq!(
            total_ink,
            InverseSolveError::ContinuityReferenceTotalInkExceeded
        );
    }

    #[test]
    fn continuity_prefers_nearby_equivalent_separation_without_relaxing_color_limit() {
        let model = DegenerateFourInkModel::new(1.0);
        let strategy = SeparationStrategy {
            max_delta_e00: Some(1.5),
            ..SeparationStrategy::default()
        };
        let mut baseline_config = config();
        baseline_config.preference_delta_e00 = 0.5;
        let baseline = solve_inverse_separation(
            &target(),
            &strategy,
            weights(0.0, 0.0, 0.0),
            &model,
            target_lab(),
            baseline_config,
        )
        .unwrap();

        let reference = [0.5, 0.0, 0.0, 0.0];
        let mut continuity = continuity_config(50.0);
        continuity.preference_delta_e00 = 0.5;
        continuity
            .continuity_preference
            .as_mut()
            .unwrap()
            .max_normalized_channel_jump = 1.0;
        continuity
            .continuity_preference
            .as_mut()
            .unwrap()
            .dominant_channel_switch_penalty = 0.0;
        let selected = solve_inverse_separation_with_reference(
            &target(),
            &strategy,
            weights(0.0, 0.0, 0.0),
            &model,
            target_lab(),
            continuity,
            Some(&reference),
        )
        .unwrap();

        assert!(selected.candidate.delta_e00 <= 1.5);
        assert!(selected.evaluation.total_ink <= 1.5);
        assert!(
            selected
                .candidate
                .coverages
                .iter()
                .all(|value| *value <= 0.8)
        );
        assert!(
            normalized_l1_to_reference(&selected.candidate.coverages, &reference)
                < normalized_l1_to_reference(&baseline.candidate.coverages, &reference)
        );
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
