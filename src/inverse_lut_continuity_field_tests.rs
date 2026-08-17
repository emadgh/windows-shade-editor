use crate::color_conversion::{
    ConversionTargetDefinition, SeparationStrategy, TargetChannelDefinition,
};
use crate::custom_optimizer_config::{
    ContinuityDistanceMetric, ContinuityPreferenceConfig, CustomOptimizerSolverConfig,
    CustomOptimizerSolverMethod,
};
use crate::device_characterization::{
    CharacterizationIdentity, DeviceForwardModel, LabColor, delta_e_2000,
};
use crate::gradient_continuity::{
    GradientContinuityPolicy, GradientSampleDiagnostic, build_report,
};
use crate::gradient_validation::{GradientCurvaturePolicy, analyze_gradient_curvature};
use crate::inverse_lut_continuity_field::{
    ContinuityFieldNode, JACOBI_SIX_NEIGHBOR_POLICY_SCHEMA_VERSION, JacobiGridShape,
    JacobiSixNeighborPolicy, build_jacobi_six_neighbor_field,
};
use crate::inverse_separation_solver::{
    InverseSolverStats, solve_inverse_separation, solve_inverse_separation_with_reference,
};
use crate::separation_optimizer::CandidateScoringWeights;

struct BranchingTwoInkModel {
    identity: CharacterizationIdentity,
}

impl BranchingTwoInkModel {
    fn new() -> Self {
        Self {
            identity: CharacterizationIdentity {
                id: "jacobi-branching-fixture".to_owned(),
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
            b: 80.0 * a * b,
        })
    }
}

fn target() -> ConversionTargetDefinition {
    ConversionTargetDefinition {
        name: "Jacobi branching fixture".to_owned(),
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
        characterization_id: Some("jacobi-branching-fixture".to_owned()),
        total_ink_limit: Some(1.0),
    }
}

fn strategy() -> SeparationStrategy {
    SeparationStrategy {
        max_delta_e00: Some(1.5),
        ..SeparationStrategy::default()
    }
}

fn weights() -> CandidateScoringWeights {
    CandidateScoringWeights {
        color_error: 1.0,
        ink_preference: 0.0,
        neutral_black: 0.0,
        total_ink: 0.0,
    }
}

fn baseline_config() -> CustomOptimizerSolverConfig {
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

fn continuity_config() -> CustomOptimizerSolverConfig {
    CustomOptimizerSolverConfig {
        method: CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2,
        continuity_preference: Some(ContinuityPreferenceConfig {
            weight: 30.0,
            distance_metric: ContinuityDistanceMetric::NormalizedL1,
            max_normalized_channel_jump: 0.20,
            dominant_channel_switch_penalty: 1.0,
        }),
        ..baseline_config()
    }
}

fn path() -> [LabColor; 3] {
    [0.42_f64, 0.50, 0.58].map(|darkness| LabColor {
        l: 95.0 - 40.0 * darkness,
        a: 0.0,
        b: 0.0,
    })
}

fn diagnostic_policy() -> GradientContinuityPolicy {
    GradientContinuityPolicy {
        max_channel_jump: 2.0,
        max_normalized_channel_jump: 2.0,
        max_vector_l1_jump: 2.0,
        max_vector_l2_jump: 2.0,
        max_total_ink_jump: 2.0,
    }
}

fn curvature_policy() -> GradientCurvaturePolicy {
    GradientCurvaturePolicy {
        max_channel_second_difference: 2.0,
        max_normalized_channel_second_difference: 2.0,
        max_vector_l1_second_difference: 4.0,
        max_vector_l2_second_difference: 4.0,
        max_total_ink_second_difference: 2.0,
    }
}

fn sample_from_coverages(
    index: usize,
    target_lab: LabColor,
    coverages: Vec<f32>,
    model: &BranchingTwoInkModel,
) -> GradientSampleDiagnostic {
    let predicted = model.predict_lab(&coverages).unwrap();
    GradientSampleDiagnostic {
        index,
        target_lab,
        delta_e00: delta_e_2000(target_lab, predicted) as f32,
        total_ink: coverages.iter().sum(),
        coverages,
        solver_stats: InverseSolverStats::default(),
    }
}

#[test]
fn jacobi_v2_field_reduces_branch_discontinuity_without_relaxing_color_or_ink_limits() {
    let target = target();
    let strategy = strategy();
    let weights = weights();
    let model = BranchingTwoInkModel::new();
    let path = path();

    let mut baseline_samples = Vec::new();
    let mut initial_nodes = Vec::new();
    for (index, target_lab) in path.iter().copied().enumerate() {
        let solved = solve_inverse_separation(
            &target,
            &strategy,
            weights,
            &model,
            target_lab,
            baseline_config(),
        )
        .unwrap();
        initial_nodes.push(ContinuityFieldNode::valid(
            solved.candidate.coverages.clone(),
        ));
        baseline_samples.push(GradientSampleDiagnostic {
            index,
            target_lab,
            coverages: solved.candidate.coverages,
            delta_e00: solved.candidate.delta_e00,
            total_ink: solved.evaluation.total_ink,
            solver_stats: solved.stats,
        });
    }
    let baseline_report = build_report(&target, &strategy, baseline_samples, &diagnostic_policy());
    assert!(
        baseline_report.dominant_channel_switches >= 1,
        "fixture must exhibit a baseline branch switch: {baseline_report:?}"
    );

    let field_policy = JacobiSixNeighborPolicy {
        schema_version: JACOBI_SIX_NEIGHBOR_POLICY_SCHEMA_VERSION,
        iterations: 6,
        self_weight: 0.35,
    };
    let shape = JacobiGridShape { l: 3, a: 1, b: 1 };
    let field =
        build_jacobi_six_neighbor_field(&initial_nodes, shape, field_policy, |index, reference| {
            solve_inverse_separation_with_reference(
                &target,
                &strategy,
                weights,
                &model,
                path[index],
                continuity_config(),
                Some(reference),
            )
            .map(|result| result.candidate.coverages)
            .map_err(|error| format!("{error:?}"))
        })
        .unwrap();

    let field_samples = field
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            assert!(node.valid);
            sample_from_coverages(index, path[index], node.coverages.clone(), &model)
        })
        .collect::<Vec<_>>();
    let field_report = build_report(&target, &strategy, field_samples, &diagnostic_policy());
    let baseline_curvature =
        analyze_gradient_curvature(&target, &strategy, &baseline_report, &curvature_policy())
            .unwrap();
    let field_curvature =
        analyze_gradient_curvature(&target, &strategy, &field_report, &curvature_policy()).unwrap();

    assert!(
        field_report.max_vector_l1_jump < baseline_report.max_vector_l1_jump,
        "Jacobi field did not reduce L1 jump: baseline={} field={}",
        baseline_report.max_vector_l1_jump,
        field_report.max_vector_l1_jump,
    );
    assert!(
        field_report.dominant_channel_switches <= baseline_report.dominant_channel_switches,
        "Jacobi field increased dominant-channel switching"
    );
    assert!(
        field_curvature.max_vector_l1_second_difference
            <= baseline_curvature.max_vector_l1_second_difference + 1.0e-6,
        "Jacobi field increased L1 curvature: baseline={} field={}",
        baseline_curvature.max_vector_l1_second_difference,
        field_curvature.max_vector_l1_second_difference,
    );
    assert!(
        field_report
            .samples
            .iter()
            .all(|sample| sample.delta_e00 <= 1.5)
    );
    assert!(
        field_report
            .samples
            .iter()
            .all(|sample| sample.total_ink <= 1.0 + 1.0e-6)
    );

    let second =
        build_jacobi_six_neighbor_field(&initial_nodes, shape, field_policy, |index, reference| {
            solve_inverse_separation_with_reference(
                &target,
                &strategy,
                weights,
                &model,
                path[index],
                continuity_config(),
                Some(reference),
            )
            .map(|result| result.candidate.coverages)
            .map_err(|error| format!("{error:?}"))
        })
        .unwrap();
    assert_eq!(field, second);
}
