use crate::color_conversion::{ConversionTargetDefinition, SeparationStrategy};
use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
use crate::device_characterization::{DeviceForwardModel, LabColor};
use crate::inverse_separation_solver::{
    InverseSolveError, InverseSolverStats, solve_inverse_separation,
    solve_inverse_separation_with_reference,
};
use crate::separation_optimizer::CandidateScoringWeights;

#[derive(Clone, Debug, PartialEq)]
pub struct GradientContinuityPolicy {
    pub max_channel_jump: f32,
    pub max_normalized_channel_jump: f32,
    pub max_vector_l1_jump: f32,
    pub max_vector_l2_jump: f32,
    pub max_total_ink_jump: f32,
}

impl GradientContinuityPolicy {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        validate_threshold("max_channel_jump", self.max_channel_jump, &mut errors);
        validate_threshold(
            "max_normalized_channel_jump",
            self.max_normalized_channel_jump,
            &mut errors,
        );
        validate_threshold("max_vector_l1_jump", self.max_vector_l1_jump, &mut errors);
        validate_threshold("max_vector_l2_jump", self.max_vector_l2_jump, &mut errors);
        validate_threshold("max_total_ink_jump", self.max_total_ink_jump, &mut errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

fn validate_threshold(name: &str, value: f32, errors: &mut Vec<String>) {
    if !value.is_finite() || value < 0.0 {
        errors.push(format!("{name} must be finite and >= 0."));
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GradientSampleDiagnostic {
    pub index: usize,
    pub target_lab: LabColor,
    pub coverages: Vec<f32>,
    pub delta_e00: f32,
    pub total_ink: f32,
    pub solver_stats: InverseSolverStats,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GradientContinuityViolation {
    ChannelJump {
        channel_index: usize,
        value: f32,
        limit: f32,
    },
    NormalizedChannelJump {
        channel_index: usize,
        value: f32,
        limit: f32,
    },
    VectorL1Jump {
        value: f32,
        limit: f32,
    },
    VectorL2Jump {
        value: f32,
        limit: f32,
    },
    TotalInkJump {
        value: f32,
        limit: f32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct GradientTransitionDiagnostic {
    pub from_index: usize,
    pub to_index: usize,
    pub per_channel_abs_delta: Vec<f32>,
    pub per_channel_normalized_delta: Vec<f32>,
    pub max_channel_jump: f32,
    pub max_normalized_channel_jump: f32,
    pub vector_l1_jump: f32,
    pub vector_l2_jump: f32,
    pub total_ink_jump: f32,
    pub dominant_channel_before: Option<usize>,
    pub dominant_channel_after: Option<usize>,
    pub dominant_channel_changed: bool,
    pub violations: Vec<GradientContinuityViolation>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GradientContinuityReport {
    pub samples: Vec<GradientSampleDiagnostic>,
    pub transitions: Vec<GradientTransitionDiagnostic>,
    pub max_channel_jump: f32,
    pub max_normalized_channel_jump: f32,
    pub max_vector_l1_jump: f32,
    pub max_vector_l2_jump: f32,
    pub max_total_ink_jump: f32,
    pub dominant_channel_switches: usize,
    pub violation_count: usize,
}

impl GradientContinuityReport {
    pub fn passes(&self) -> bool {
        self.violation_count == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GradientContinuityError {
    EmptyPath,
    InvalidPolicy(Vec<String>),
    SolveFailed {
        index: usize,
        error: InverseSolveError,
    },
    SampleTopologyMismatch {
        index: usize,
        expected: usize,
        actual: usize,
    },
}

/// Evaluate separation continuity along an ordered PCS/Lab path.
///
/// The diagnostic deliberately operates on reference-solver output before raster
/// quantization. Every sample is solved independently through the exact measured
/// forward model and the supplied persisted solver configuration. An unsupported
/// or infeasible path sample fails closed instead of being bridged or interpolated.
pub fn analyze_gradient_path(
    target: &ConversionTargetDefinition,
    strategy: &SeparationStrategy,
    weights: CandidateScoringWeights,
    model: &dyn DeviceForwardModel,
    path: &[LabColor],
    solver_config: &CustomOptimizerSolverConfig,
    policy: &GradientContinuityPolicy,
) -> Result<GradientContinuityReport, GradientContinuityError> {
    if path.is_empty() {
        return Err(GradientContinuityError::EmptyPath);
    }
    policy
        .validate()
        .map_err(GradientContinuityError::InvalidPolicy)?;

    let mut samples = Vec::with_capacity(path.len());
    for (index, target_lab) in path.iter().copied().enumerate() {
        let result = solve_inverse_separation(
            target,
            strategy,
            weights,
            model,
            target_lab,
            solver_config.clone(),
        )
        .map_err(|error| GradientContinuityError::SolveFailed { index, error })?;

        if result.candidate.coverages.len() != target.channels.len() {
            return Err(GradientContinuityError::SampleTopologyMismatch {
                index,
                expected: target.channels.len(),
                actual: result.candidate.coverages.len(),
            });
        }

        samples.push(GradientSampleDiagnostic {
            index,
            target_lab,
            coverages: result.candidate.coverages,
            delta_e00: result.candidate.delta_e00,
            total_ink: result.evaluation.total_ink,
            solver_stats: result.stats,
        });
    }

    Ok(build_report(target, strategy, samples, policy))
}

/// Evaluate an ordered PCS/Lab path with V2 continuity preference using explicit
/// reference state owned by this diagnostic traversal.
///
/// `initial_reference_coverages` is the explicit reference for sample zero. Each
/// later sample receives the immediately previous selected separation. The solver
/// itself remains stateless; no process-global or hidden path history is used.
pub fn analyze_continuity_aware_gradient_path(
    target: &ConversionTargetDefinition,
    strategy: &SeparationStrategy,
    weights: CandidateScoringWeights,
    model: &dyn DeviceForwardModel,
    path: &[LabColor],
    solver_config: &CustomOptimizerSolverConfig,
    initial_reference_coverages: &[f32],
    policy: &GradientContinuityPolicy,
) -> Result<GradientContinuityReport, GradientContinuityError> {
    if path.is_empty() {
        return Err(GradientContinuityError::EmptyPath);
    }
    policy
        .validate()
        .map_err(GradientContinuityError::InvalidPolicy)?;

    let mut reference_coverages = initial_reference_coverages.to_vec();
    let mut samples = Vec::with_capacity(path.len());
    for (index, target_lab) in path.iter().copied().enumerate() {
        let result = solve_inverse_separation_with_reference(
            target,
            strategy,
            weights,
            model,
            target_lab,
            *solver_config,
            Some(&reference_coverages),
        )
        .map_err(|error| GradientContinuityError::SolveFailed { index, error })?;

        if result.candidate.coverages.len() != target.channels.len() {
            return Err(GradientContinuityError::SampleTopologyMismatch {
                index,
                expected: target.channels.len(),
                actual: result.candidate.coverages.len(),
            });
        }

        let coverages = result.candidate.coverages;
        reference_coverages = coverages.clone();
        samples.push(GradientSampleDiagnostic {
            index,
            target_lab,
            coverages,
            delta_e00: result.candidate.delta_e00,
            total_ink: result.evaluation.total_ink,
            solver_stats: result.stats,
        });
    }

    Ok(build_report(target, strategy, samples, policy))
}

pub(crate) fn build_report(
    target: &ConversionTargetDefinition,
    strategy: &SeparationStrategy,
    samples: Vec<GradientSampleDiagnostic>,
    policy: &GradientContinuityPolicy,
) -> GradientContinuityReport {
    let maxima = effective_channel_maxima(target, strategy);
    let mut transitions = Vec::with_capacity(samples.len().saturating_sub(1));

    for pair in samples.windows(2) {
        transitions.push(diagnose_transition(&pair[0], &pair[1], &maxima, policy));
    }

    let max_channel_jump = transitions
        .iter()
        .map(|item| item.max_channel_jump)
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let max_normalized_channel_jump = transitions
        .iter()
        .map(|item| item.max_normalized_channel_jump)
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let max_vector_l1_jump = transitions
        .iter()
        .map(|item| item.vector_l1_jump)
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let max_vector_l2_jump = transitions
        .iter()
        .map(|item| item.vector_l2_jump)
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let max_total_ink_jump = transitions
        .iter()
        .map(|item| item.total_ink_jump)
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let dominant_channel_switches = transitions
        .iter()
        .filter(|item| item.dominant_channel_changed)
        .count();
    let violation_count = transitions.iter().map(|item| item.violations.len()).sum();

    GradientContinuityReport {
        samples,
        transitions,
        max_channel_jump,
        max_normalized_channel_jump,
        max_vector_l1_jump,
        max_vector_l2_jump,
        max_total_ink_jump,
        dominant_channel_switches,
        violation_count,
    }
}

fn diagnose_transition(
    before: &GradientSampleDiagnostic,
    after: &GradientSampleDiagnostic,
    maxima: &[f32],
    policy: &GradientContinuityPolicy,
) -> GradientTransitionDiagnostic {
    debug_assert_eq!(before.coverages.len(), after.coverages.len());
    debug_assert_eq!(before.coverages.len(), maxima.len());

    let per_channel_abs_delta = before
        .coverages
        .iter()
        .zip(&after.coverages)
        .map(|(left, right)| (right - left).abs())
        .collect::<Vec<_>>();
    let per_channel_normalized_delta = per_channel_abs_delta
        .iter()
        .copied()
        .zip(maxima.iter().copied())
        .map(|(delta, maximum)| {
            if maximum > f32::EPSILON {
                delta / maximum
            } else {
                0.0
            }
        })
        .collect::<Vec<_>>();

    let max_channel_jump = per_channel_abs_delta
        .iter()
        .copied()
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let max_normalized_channel_jump = per_channel_normalized_delta
        .iter()
        .copied()
        .max_by(f32::total_cmp)
        .unwrap_or(0.0);
    let vector_l1_jump = per_channel_abs_delta.iter().sum::<f32>();
    let vector_l2_jump = per_channel_abs_delta
        .iter()
        .map(|delta| delta * delta)
        .sum::<f32>()
        .sqrt();
    let total_ink_jump = (after.total_ink - before.total_ink).abs();
    let dominant_channel_before = dominant_channel(&before.coverages);
    let dominant_channel_after = dominant_channel(&after.coverages);
    let dominant_channel_changed = dominant_channel_before != dominant_channel_after;

    let mut violations = Vec::new();
    for (channel_index, value) in per_channel_abs_delta.iter().copied().enumerate() {
        if value > policy.max_channel_jump {
            violations.push(GradientContinuityViolation::ChannelJump {
                channel_index,
                value,
                limit: policy.max_channel_jump,
            });
        }
    }
    for (channel_index, value) in per_channel_normalized_delta.iter().copied().enumerate() {
        if value > policy.max_normalized_channel_jump {
            violations.push(GradientContinuityViolation::NormalizedChannelJump {
                channel_index,
                value,
                limit: policy.max_normalized_channel_jump,
            });
        }
    }
    if vector_l1_jump > policy.max_vector_l1_jump {
        violations.push(GradientContinuityViolation::VectorL1Jump {
            value: vector_l1_jump,
            limit: policy.max_vector_l1_jump,
        });
    }
    if vector_l2_jump > policy.max_vector_l2_jump {
        violations.push(GradientContinuityViolation::VectorL2Jump {
            value: vector_l2_jump,
            limit: policy.max_vector_l2_jump,
        });
    }
    if total_ink_jump > policy.max_total_ink_jump {
        violations.push(GradientContinuityViolation::TotalInkJump {
            value: total_ink_jump,
            limit: policy.max_total_ink_jump,
        });
    }

    GradientTransitionDiagnostic {
        from_index: before.index,
        to_index: after.index,
        per_channel_abs_delta,
        per_channel_normalized_delta,
        max_channel_jump,
        max_normalized_channel_jump,
        vector_l1_jump,
        vector_l2_jump,
        total_ink_jump,
        dominant_channel_before,
        dominant_channel_after,
        dominant_channel_changed,
        violations,
    }
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

fn effective_channel_maxima(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::TargetChannelDefinition;
    use crate::custom_optimizer_config::{
        ContinuityDistanceMetric, ContinuityPreferenceConfig, CustomOptimizerSolverMethod,
    };
    use crate::device_characterization::CharacterizationIdentity;

    struct LinearOneInkModel {
        identity: CharacterizationIdentity,
    }

    impl LinearOneInkModel {
        fn new() -> Self {
            Self {
                identity: CharacterizationIdentity {
                    id: "gradient-fixture".to_owned(),
                    channel_names: vec!["Ink".to_owned()],
                },
            }
        }
    }

    impl DeviceForwardModel for LinearOneInkModel {
        fn identity(&self) -> &CharacterizationIdentity {
            &self.identity
        }

        fn predict_lab(&self, coverages: &[f32]) -> Result<LabColor, String> {
            if coverages.len() != 1
                || !coverages[0].is_finite()
                || !(0.0..=1.0).contains(&coverages[0])
            {
                return Err("fixture coverage outside domain".to_owned());
            }
            Ok(LabColor {
                l: 100.0 - 100.0 * f64::from(coverages[0]),
                a: 0.0,
                b: 0.0,
            })
        }
    }

    struct BranchingTwoInkModel {
        identity: CharacterizationIdentity,
    }

    impl BranchingTwoInkModel {
        fn new() -> Self {
            Self {
                identity: CharacterizationIdentity {
                    id: "branching-gradient-fixture".to_owned(),
                    channel_names: vec!["A".to_owned(), "B".to_owned()],
                },
            }
        }

        fn a_response(value: f64) -> f64 {
            value + 0.10 * (std::f64::consts::TAU * value).sin()
        }

        fn b_response(value: f64) -> f64 {
            value - 0.10 * (std::f64::consts::TAU * value).sin()
        }
    }

    impl DeviceForwardModel for BranchingTwoInkModel {
        fn identity(&self) -> &CharacterizationIdentity {
            &self.identity
        }

        fn predict_lab(&self, coverages: &[f32]) -> Result<LabColor, String> {
            if coverages.len() != 2
                || coverages
                    .iter()
                    .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
            {
                return Err("branching fixture coverage outside domain".to_owned());
            }
            let a = f64::from(coverages[0]);
            let b = f64::from(coverages[1]);
            let darkness = Self::a_response(a) + Self::b_response(b);
            Ok(LabColor {
                l: 95.0 - 40.0 * darkness,
                a: 0.0,
                // Penalize mixed-branch solutions while leaving both one-ink
                // branches on the target b*=0 path.
                b: 80.0 * a * b,
            })
        }
    }

    fn target(channel_names: &[&str]) -> ConversionTargetDefinition {
        ConversionTargetDefinition {
            name: "Gradient fixture".to_owned(),
            channels: channel_names
                .iter()
                .map(|name| TargetChannelDefinition {
                    name: (*name).to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: Some(1.0),
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: None,
            output_profile_path: None,
            device_link_identity: None,
            device_link_path: None,
            characterization_id: Some("gradient-fixture".to_owned()),
            total_ink_limit: None,
        }
    }

    fn branching_target() -> ConversionTargetDefinition {
        ConversionTargetDefinition {
            name: "Branching gradient fixture".to_owned(),
            channels: ["A", "B"]
                .into_iter()
                .map(|name| TargetChannelDefinition {
                    name: name.to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: Some(1.0),
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: None,
            output_profile_path: None,
            device_link_identity: None,
            device_link_path: None,
            characterization_id: Some("branching-gradient-fixture".to_owned()),
            total_ink_limit: Some(1.0),
        }
    }

    fn branching_solver_config() -> CustomOptimizerSolverConfig {
        CustomOptimizerSolverConfig {
            initial_samples: 1024,
            beam_width: 64,
            refinement_rounds: 6,
            initial_step_fraction: 0.12,
            step_decay: 0.5,
            preference_delta_e00: 0.50,
            ..CustomOptimizerSolverConfig::default()
        }
    }

    fn branching_continuity_config() -> CustomOptimizerSolverConfig {
        CustomOptimizerSolverConfig {
            method: CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2,
            continuity_preference: Some(ContinuityPreferenceConfig {
                weight: 30.0,
                distance_metric: ContinuityDistanceMetric::NormalizedL1,
                max_normalized_channel_jump: 0.20,
                dominant_channel_switch_penalty: 1.0,
            }),
            ..branching_solver_config()
        }
    }

    fn policy(limit: f32) -> GradientContinuityPolicy {
        GradientContinuityPolicy {
            max_channel_jump: limit,
            max_normalized_channel_jump: limit,
            max_vector_l1_jump: limit,
            max_vector_l2_jump: limit,
            max_total_ink_jump: limit,
        }
    }

    fn sample(index: usize, coverages: Vec<f32>, delta_e00: f32) -> GradientSampleDiagnostic {
        GradientSampleDiagnostic {
            index,
            target_lab: LabColor {
                l: 50.0,
                a: 0.0,
                b: 0.0,
            },
            total_ink: coverages.iter().sum(),
            coverages,
            delta_e00,
            solver_stats: InverseSolverStats::default(),
        }
    }

    #[test]
    fn policy_rejects_non_finite_or_negative_thresholds() {
        let mut invalid = policy(0.1);
        invalid.max_vector_l2_jump = f32::NAN;
        invalid.max_total_ink_jump = -0.1;
        let errors = invalid.validate().expect_err("invalid policy must fail");
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn abrupt_substitution_is_reported_even_with_equal_color_error() {
        let target = target(&["A", "B"]);
        let strategy = SeparationStrategy::default();
        let samples = vec![
            sample(0, vec![0.5, 0.0], 0.1),
            sample(1, vec![0.0, 0.5], 0.1),
        ];
        let report = build_report(&target, &strategy, samples, &policy(0.2));

        assert!(!report.passes());
        assert_eq!(report.dominant_channel_switches, 1);
        assert!((report.max_channel_jump - 0.5).abs() < 1e-6);
        assert!((report.max_vector_l1_jump - 1.0).abs() < 1e-6);
        assert!((report.max_vector_l2_jump - 0.5_f32.sqrt()).abs() < 1e-6);
        assert_eq!(report.samples[0].delta_e00, report.samples[1].delta_e00);
    }

    #[test]
    fn continuity_v2_reduces_branch_switch_jump_on_synthetic_gradient() {
        let target = branching_target();
        let strategy = SeparationStrategy {
            max_delta_e00: Some(1.5),
            ..SeparationStrategy::default()
        };
        let weights = CandidateScoringWeights {
            color_error: 1.0,
            ink_preference: 0.0,
            neutral_black: 0.0,
            total_ink: 0.0,
        };
        let model = BranchingTwoInkModel::new();
        let path = [0.42_f64, 0.50, 0.58].map(|darkness| LabColor {
            l: 95.0 - 40.0 * darkness,
            a: 0.0,
            b: 0.0,
        });
        let diagnostic_policy = policy(2.0);

        let baseline = analyze_gradient_path(
            &target,
            &strategy,
            weights,
            &model,
            &path,
            &branching_solver_config(),
            &diagnostic_policy,
        )
        .expect("baseline branching diagnostic");
        let continuity = analyze_continuity_aware_gradient_path(
            &target,
            &strategy,
            weights,
            &model,
            &path,
            &branching_continuity_config(),
            &[0.32, 0.0],
            &diagnostic_policy,
        )
        .expect("continuity-aware branching diagnostic");

        assert!(
            baseline.dominant_channel_switches >= 1,
            "fixture must exhibit a baseline branch switch: {baseline:?}"
        );
        assert!(
            continuity.max_vector_l1_jump < baseline.max_vector_l1_jump,
            "continuity did not reduce L1 jump: baseline={} continuity={} baseline_switches={} continuity_switches={}",
            baseline.max_vector_l1_jump,
            continuity.max_vector_l1_jump,
            baseline.dominant_channel_switches,
            continuity.dominant_channel_switches,
        );
        assert!(
            continuity.dominant_channel_switches <= baseline.dominant_channel_switches,
            "continuity increased branch switching"
        );
        assert!(
            continuity
                .samples
                .iter()
                .all(|sample| sample.delta_e00 <= 1.5)
        );
        assert!(
            continuity
                .samples
                .iter()
                .all(|sample| sample.total_ink <= 1.0 + 1.0e-6)
        );
    }

    #[test]
    fn reference_path_report_is_deterministic() {
        let target = target(&["Ink"]);
        let strategy = SeparationStrategy::default();
        let weights = CandidateScoringWeights {
            color_error: 1.0,
            ink_preference: 0.0,
            neutral_black: 0.0,
            total_ink: 0.0,
        };
        let model = LinearOneInkModel::new();
        let path = [
            LabColor {
                l: 90.0,
                a: 0.0,
                b: 0.0,
            },
            LabColor {
                l: 85.0,
                a: 0.0,
                b: 0.0,
            },
            LabColor {
                l: 80.0,
                a: 0.0,
                b: 0.0,
            },
        ];
        let config = CustomOptimizerSolverConfig {
            initial_samples: 64,
            beam_width: 12,
            refinement_rounds: 3,
            initial_step_fraction: 0.10,
            step_decay: 0.5,
            preference_delta_e00: 0.0,
            ..CustomOptimizerSolverConfig::default()
        };
        let policy = policy(0.2);

        let first =
            analyze_gradient_path(&target, &strategy, weights, &model, &path, &config, &policy)
                .expect("first diagnostic");
        let second =
            analyze_gradient_path(&target, &strategy, weights, &model, &path, &config, &policy)
                .expect("second diagnostic");

        assert_eq!(first, second);
        assert_eq!(first.samples.len(), 3);
        assert_eq!(first.transitions.len(), 2);
    }
}
