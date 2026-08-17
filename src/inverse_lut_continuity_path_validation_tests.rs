use crate::color_conversion::{
    ConversionTargetDefinition, SeparationStrategy, TargetChannelDefinition,
};
use crate::device_characterization::LabColor;
use crate::gradient_continuity::{
    GradientContinuityPolicy, GradientSampleDiagnostic, build_report,
};
use crate::gradient_validation::{GradientCurvaturePolicy, analyze_gradient_curvature};
use crate::inverse_lut_continuity_field::{
    ContinuityFieldNode, JACOBI_SIX_NEIGHBOR_POLICY_SCHEMA_VERSION, JacobiGridShape,
    JacobiSixNeighborPolicy, build_jacobi_six_neighbor_field,
};
use crate::inverse_separation_solver::InverseSolverStats;

const AXIS: usize = 5;

fn shape() -> JacobiGridShape {
    JacobiGridShape {
        l: AXIS,
        a: AXIS,
        b: AXIS,
    }
}

fn index(l: usize, a: usize, b: usize) -> usize {
    (l * AXIS + a) * AXIS + b
}

fn coordinates(index: usize) -> (usize, usize, usize) {
    let l = index / (AXIS * AXIS);
    let remainder = index % (AXIS * AXIS);
    let a = remainder / AXIS;
    let b = remainder % AXIS;
    (l, a, b)
}

fn desired(index: usize) -> [f32; 2] {
    let (l, a, b) = coordinates(index);
    let t = (l + a + b) as f32 / (3 * (AXIS - 1)) as f32;
    [t, 1.0 - t]
}

fn branchy_independent_seed() -> Vec<ContinuityFieldNode> {
    (0..shape().node_count().unwrap())
        .map(|node_index| {
            let [first, second] = desired(node_index);
            let (l, a, b) = coordinates(node_index);
            let coverages = if (l + a + b) % 2 == 0 {
                vec![first, second]
            } else {
                vec![second, first]
            };
            ContinuityFieldNode::valid(coverages)
        })
        .collect()
}

fn deterministic_reference_solver(
    node_index: usize,
    reference: &[f32],
) -> Result<Vec<f32>, String> {
    let [first, second] = desired(node_index);
    Ok(vec![
        (0.75 * reference[0] + 0.25 * first).clamp(0.0, 1.0),
        (0.75 * reference[1] + 0.25 * second).clamp(0.0, 1.0),
    ])
}

fn target() -> ConversionTargetDefinition {
    ConversionTargetDefinition {
        name: "Jacobi path diagnostic fixture".to_owned(),
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
        characterization_id: Some("jacobi-path-diagnostic-fixture".to_owned()),
        total_ink_limit: Some(1.0),
    }
}

fn continuity_policy() -> GradientContinuityPolicy {
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

fn path_report(
    nodes: &[ContinuityFieldNode],
    path: &[usize],
) -> (
    crate::gradient_continuity::GradientContinuityReport,
    crate::gradient_validation::GradientCurvatureReport,
) {
    let target = target();
    let strategy = SeparationStrategy::default();
    let samples = path
        .iter()
        .copied()
        .enumerate()
        .map(|(sample_index, node_index)| {
            let node = &nodes[node_index];
            assert!(node.valid);
            GradientSampleDiagnostic {
                index: sample_index,
                target_lab: LabColor {
                    l: 20.0 + 15.0 * sample_index as f64,
                    a: sample_index as f64 - 2.0,
                    b: 2.0 * sample_index as f64 - 4.0,
                },
                delta_e00: 0.0,
                total_ink: node.coverages.iter().sum(),
                coverages: node.coverages.clone(),
                solver_stats: InverseSolverStats::default(),
            }
        })
        .collect::<Vec<_>>();
    let report = build_report(&target, &strategy, samples, &continuity_policy());
    let curvature = analyze_gradient_curvature(&target, &strategy, &report, &curvature_policy())
        .expect("deterministic path must have a valid curvature report");
    (report, curvature)
}

#[test]
fn jacobi_field_reduces_axis_and_diagonal_jump_and_curvature_metrics() {
    let seed = branchy_independent_seed();
    let field = build_jacobi_six_neighbor_field(
        &seed,
        shape(),
        JacobiSixNeighborPolicy {
            schema_version: JACOBI_SIX_NEIGHBOR_POLICY_SCHEMA_VERSION,
            iterations: 8,
            self_weight: 0.35,
        },
        deterministic_reference_solver,
    )
    .unwrap();

    let center = AXIS / 2;
    let axis_path = (0..AXIS)
        .map(|l| index(l, center, center))
        .collect::<Vec<_>>();
    let diagonal_path = (0..AXIS).map(|i| index(i, i, i)).collect::<Vec<_>>();

    for (name, path) in [("axis", axis_path), ("diagonal", diagonal_path)] {
        let (baseline, baseline_curvature) = path_report(&seed, &path);
        let (smoothed, smoothed_curvature) = path_report(&field.nodes, &path);

        assert!(
            smoothed.max_vector_l1_jump < baseline.max_vector_l1_jump * 0.5,
            "{name} L1 jump did not improve enough: baseline={} smoothed={}",
            baseline.max_vector_l1_jump,
            smoothed.max_vector_l1_jump,
        );
        assert!(
            smoothed_curvature.max_vector_l1_second_difference
                < baseline_curvature.max_vector_l1_second_difference * 0.25,
            "{name} L1 curvature did not improve enough: baseline={} smoothed={}",
            baseline_curvature.max_vector_l1_second_difference,
            smoothed_curvature.max_vector_l1_second_difference,
        );
        assert!(
            smoothed.dominant_channel_switches <= baseline.dominant_channel_switches,
            "{name} dominant-channel switching increased"
        );
        assert!(
            smoothed
                .samples
                .iter()
                .all(|sample| (sample.total_ink - 1.0).abs() <= 1.0e-5),
            "{name} path violated the synthetic total-ink invariant"
        );
    }
}
