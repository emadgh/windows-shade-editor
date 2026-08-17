use crate::inverse_lut_continuity_field::{
    ContinuityFieldNode, JACOBI_SIX_NEIGHBOR_POLICY_SCHEMA_VERSION, JacobiFieldResult,
    JacobiGridShape, JacobiSixNeighborPolicy, build_jacobi_six_neighbor_field,
};

fn shape() -> JacobiGridShape {
    JacobiGridShape { l: 3, a: 3, b: 3 }
}

fn policy(iterations: u16) -> JacobiSixNeighborPolicy {
    JacobiSixNeighborPolicy {
        schema_version: JACOBI_SIX_NEIGHBOR_POLICY_SCHEMA_VERSION,
        iterations,
        self_weight: 0.35,
    }
}

fn independent_seed(swapped: bool) -> Vec<ContinuityFieldNode> {
    let count = shape().node_count().unwrap();
    (0..count)
        .map(|index| {
            let phase = index as f32 / (count - 1) as f32;
            let coverages = if swapped {
                vec![1.0 - phase, phase]
            } else {
                vec![phase, 1.0 - phase]
            };
            ContinuityFieldNode::valid(coverages)
        })
        .collect()
}

fn deterministic_solver(index: usize, reference: &[f32]) -> Result<Vec<f32>, String> {
    let target = (index % 7) as f32 / 6.0;
    Ok(vec![
        (0.7 * reference[0] + 0.3 * target).clamp(0.0, 1.0),
        (0.7 * reference[1] + 0.3 * (1.0 - target)).clamp(0.0, 1.0),
    ])
}

fn build(seed: &[ContinuityFieldNode], iterations: u16) -> JacobiFieldResult {
    build_jacobi_six_neighbor_field(seed, shape(), policy(iterations), deterministic_solver)
        .unwrap()
}

fn max_l1(left: &[ContinuityFieldNode], right: &[ContinuityFieldNode]) -> f32 {
    assert_eq!(left.len(), right.len());
    left.iter()
        .zip(right)
        .map(|(left, right)| {
            assert_eq!(left.valid, right.valid);
            left.coverages
                .iter()
                .zip(&right.coverages)
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>()
        })
        .fold(0.0f32, f32::max)
}

#[test]
fn alternate_seed_sensitivity_is_measured_and_decays_for_the_frozen_stencil() {
    let seed_a = independent_seed(false);
    let seed_b = independent_seed(true);
    let initial = max_l1(&seed_a, &seed_b);
    let after_one = max_l1(&build(&seed_a, 1).nodes, &build(&seed_b, 1).nodes);
    let after_six = max_l1(&build(&seed_a, 6).nodes, &build(&seed_b, 6).nodes);

    assert!(
        initial > 1.9,
        "fixture did not create meaningful alternate seeds: {initial}"
    );
    assert!(
        after_one < initial,
        "one Jacobi iteration did not reduce alternate-seed sensitivity: initial={initial}, after_one={after_one}"
    );
    assert!(
        after_six < after_one && after_six < 0.08,
        "alternate-seed sensitivity did not decay enough: after_one={after_one}, after_six={after_six}"
    );
}

#[test]
fn iteration_count_sensitivity_is_measured_before_v1_freeze() {
    let seed = independent_seed(false);
    let at_two = build(&seed, 2);
    let at_four = build(&seed, 4);
    let at_six = build(&seed, 6);
    let at_eight = build(&seed, 8);
    let early = max_l1(&at_two.nodes, &at_four.nodes);
    let late = max_l1(&at_six.nodes, &at_eight.nodes);

    assert!(
        early > 0.1,
        "fixture does not exercise iteration-count sensitivity: {early}"
    );
    assert!(
        late < early * 0.25 && late < 0.04,
        "iteration-count sensitivity did not settle: early={early}, late={late}"
    );
}

#[test]
fn period_two_oscillation_metric_is_quantified_at_the_selected_horizon() {
    let result = build(&independent_seed(false), 8);
    let last = result
        .diagnostics
        .last()
        .expect("eight iterations produce diagnostics");
    let two_back = last
        .max_l1_from_two_back
        .expect("iteration eight has an N-vs-N-2 diagnostic");

    assert_eq!(last.iteration, 8);
    assert!(
        two_back < 0.04,
        "period-two oscillation remains too large at iteration eight: {two_back}"
    );
}
