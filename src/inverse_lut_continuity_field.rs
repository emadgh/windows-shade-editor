use serde::{Deserialize, Serialize};

pub const JACOBI_SIX_NEIGHBOR_POLICY_SCHEMA_VERSION: u32 = 1;
pub const MAX_JACOBI_ITERATIONS: u16 =
    crate::inverse_lut_identity::INVERSE_LUT_JACOBI_FIELD_METHOD_MAX_ITERATIONS;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JacobiSixNeighborPolicy {
    pub schema_version: u32,
    /// Fixed iteration count is part of field identity. It is deliberately not
    /// replaced by a floating-point convergence stop condition.
    pub iterations: u16,
    /// Convex weight assigned to the node's previous separation. Remaining
    /// weight is assigned to the arithmetic mean of valid axial neighbors.
    pub self_weight: f32,
}

impl JacobiSixNeighborPolicy {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != JACOBI_SIX_NEIGHBOR_POLICY_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported Jacobi continuity-field policy schema {} (expected {}).",
                self.schema_version, JACOBI_SIX_NEIGHBOR_POLICY_SCHEMA_VERSION
            ));
        }
        if !(1..=MAX_JACOBI_ITERATIONS).contains(&self.iterations) {
            errors.push(format!(
                "Jacobi continuity-field iterations must be in 1..={MAX_JACOBI_ITERATIONS}."
            ));
        }
        if !self.self_weight.is_finite() || !(0.0..=1.0).contains(&self.self_weight) {
            errors.push(
                "Jacobi continuity-field self_weight must be finite and in 0..=1.".to_owned(),
            );
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JacobiGridShape {
    pub l: usize,
    pub a: usize,
    pub b: usize,
}

impl JacobiGridShape {
    pub fn node_count(self) -> Option<usize> {
        self.l.checked_mul(self.a)?.checked_mul(self.b)
    }

    fn validate(self) -> Result<usize, String> {
        if self.l == 0 || self.a == 0 || self.b == 0 {
            return Err("Jacobi continuity-field grid axes must be non-zero.".to_owned());
        }
        self.node_count()
            .ok_or_else(|| "Jacobi continuity-field node count overflowed usize.".to_owned())
    }

    fn index(self, l: usize, a: usize, b: usize) -> usize {
        (l * self.a + a) * self.b + b
    }

    fn coordinates(self, index: usize) -> (usize, usize, usize) {
        let l = index / (self.a * self.b);
        let remainder = index % (self.a * self.b);
        let a = remainder / self.b;
        let b = remainder % self.b;
        (l, a, b)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContinuityFieldNode {
    pub valid: bool,
    pub coverages: Vec<f32>,
}

impl ContinuityFieldNode {
    pub fn valid(coverages: Vec<f32>) -> Self {
        Self {
            valid: true,
            coverages,
        }
    }

    pub fn unsupported(channel_count: usize) -> Self {
        Self {
            valid: false,
            coverages: vec![0.0; channel_count],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JacobiIterationDiagnostic {
    pub iteration: u16,
    pub max_l1_from_previous: f32,
    /// Difference from the state two iterations earlier. A very small value
    /// alongside a large adjacent delta is evidence of a period-two oscillation.
    pub max_l1_from_two_back: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct JacobiFieldResult {
    pub nodes: Vec<ContinuityFieldNode>,
    pub diagnostics: Vec<JacobiIterationDiagnostic>,
}

/// Prototype deterministic field smoother for #179.
///
/// Every iteration computes all references from one immutable previous snapshot,
/// then solves every node into a separate next snapshot. Consequently a correct,
/// deterministic `solve` callback cannot observe loop/raster update order.
/// Unsupported nodes are immutable and never become supported due to neighbors.
pub fn build_jacobi_six_neighbor_field<F>(
    initial: &[ContinuityFieldNode],
    shape: JacobiGridShape,
    policy: JacobiSixNeighborPolicy,
    solve: F,
) -> Result<JacobiFieldResult, String>
where
    F: Fn(usize, &[f32]) -> Result<Vec<f32>, String>,
{
    let order = (0..initial.len()).collect::<Vec<_>>();
    build_with_order(initial, shape, policy, &order, solve)
}

fn build_with_order<F>(
    initial: &[ContinuityFieldNode],
    shape: JacobiGridShape,
    policy: JacobiSixNeighborPolicy,
    order: &[usize],
    solve: F,
) -> Result<JacobiFieldResult, String>
where
    F: Fn(usize, &[f32]) -> Result<Vec<f32>, String>,
{
    policy.validate().map_err(|errors| errors.join("\n"))?;
    let expected = shape.validate()?;
    if initial.len() != expected {
        return Err(format!(
            "Jacobi continuity-field node count mismatch: expected {expected}, got {}.",
            initial.len()
        ));
    }
    validate_order(order, expected)?;
    let channel_count = validate_nodes(initial)?;

    let mut previous = initial.to_vec();
    let mut two_back: Option<Vec<ContinuityFieldNode>> = None;
    let mut diagnostics = Vec::with_capacity(usize::from(policy.iterations));

    for iteration in 1..=policy.iterations {
        // Reference construction is completed for the whole grid before any
        // callback can write the next state.
        let references = build_references(&previous, shape, channel_count, policy.self_weight)?;
        let mut next = previous.clone();
        for &index in order {
            if !previous[index].valid {
                continue;
            }
            let reference = references[index]
                .as_deref()
                .expect("validated supported node must have a reference");
            let coverages = solve(index, reference)
                .map_err(|error| format!("Jacobi solve failed at node {index}: {error}"))?;
            validate_coverage_vector(&coverages, channel_count, index)?;
            next[index] = ContinuityFieldNode::valid(coverages);
        }

        let max_l1_from_previous = max_l1_delta(&next, &previous)?;
        let max_l1_from_two_back = two_back
            .as_ref()
            .map(|state| max_l1_delta(&next, state))
            .transpose()?;
        diagnostics.push(JacobiIterationDiagnostic {
            iteration,
            max_l1_from_previous,
            max_l1_from_two_back,
        });
        two_back = Some(previous);
        previous = next;
    }

    Ok(JacobiFieldResult {
        nodes: previous,
        diagnostics,
    })
}

fn validate_order(order: &[usize], expected: usize) -> Result<(), String> {
    if order.len() != expected {
        return Err("Jacobi traversal order must contain every grid node exactly once.".to_owned());
    }
    let mut seen = vec![false; expected];
    for &index in order {
        if index >= expected || seen[index] {
            return Err("Jacobi traversal order contains an invalid or duplicate node.".to_owned());
        }
        seen[index] = true;
    }
    Ok(())
}

fn validate_nodes(nodes: &[ContinuityFieldNode]) -> Result<usize, String> {
    let channel_count = nodes
        .first()
        .map(|node| node.coverages.len())
        .ok_or_else(|| "Jacobi continuity-field requires at least one node.".to_owned())?;
    if channel_count == 0 {
        return Err("Jacobi continuity-field requires at least one channel.".to_owned());
    }
    for (index, node) in nodes.iter().enumerate() {
        validate_coverage_vector(&node.coverages, channel_count, index)?;
    }
    Ok(channel_count)
}

fn validate_coverage_vector(
    coverages: &[f32],
    channel_count: usize,
    node_index: usize,
) -> Result<(), String> {
    if coverages.len() != channel_count {
        return Err(format!(
            "Jacobi node {node_index} channel count mismatch: expected {channel_count}, got {}.",
            coverages.len()
        ));
    }
    for (channel_index, value) in coverages.iter().copied().enumerate() {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(format!(
                "Jacobi node {node_index} channel {channel_index} coverage must be finite and in 0..=1."
            ));
        }
    }
    Ok(())
}

fn build_references(
    nodes: &[ContinuityFieldNode],
    shape: JacobiGridShape,
    channel_count: usize,
    self_weight: f32,
) -> Result<Vec<Option<Vec<f32>>>, String> {
    let mut references = Vec::with_capacity(nodes.len());
    for index in 0..nodes.len() {
        let node = &nodes[index];
        if !node.valid {
            references.push(None);
            continue;
        }

        let (l, a, b) = shape.coordinates(index);
        let mut neighbor_sum = vec![0.0f64; channel_count];
        let mut neighbor_count = 0u32;
        // Fixed axis/sign accumulation order is part of the prototype numerical
        // contract: L-, L+, a-, a+, b-, b+.
        for neighbor in [
            l.checked_sub(1).map(|value| shape.index(value, a, b)),
            (l + 1 < shape.l).then(|| shape.index(l + 1, a, b)),
            a.checked_sub(1).map(|value| shape.index(l, value, b)),
            (a + 1 < shape.a).then(|| shape.index(l, a + 1, b)),
            b.checked_sub(1).map(|value| shape.index(l, a, value)),
            (b + 1 < shape.b).then(|| shape.index(l, a, b + 1)),
        ]
        .into_iter()
        .flatten()
        {
            let adjacent = &nodes[neighbor];
            if !adjacent.valid {
                continue;
            }
            for (sum, value) in neighbor_sum.iter_mut().zip(&adjacent.coverages) {
                *sum += f64::from(*value);
            }
            neighbor_count += 1;
        }

        if neighbor_count == 0 {
            references.push(Some(node.coverages.clone()));
            continue;
        }

        let neighbor_weight = 1.0 - self_weight;
        let mut reference = Vec::with_capacity(channel_count);
        for channel in 0..channel_count {
            let mean = (neighbor_sum[channel] / f64::from(neighbor_count)) as f32;
            let value = self_weight * node.coverages[channel] + neighbor_weight * mean;
            // Convex combinations of normalized valid coverages remain in range;
            // clamp only absorbs representational epsilon at the endpoints.
            reference.push(value.clamp(0.0, 1.0));
        }
        references.push(Some(reference));
    }
    Ok(references)
}

fn max_l1_delta(
    left: &[ContinuityFieldNode],
    right: &[ContinuityFieldNode],
) -> Result<f32, String> {
    if left.len() != right.len() {
        return Err("Jacobi diagnostic state length mismatch.".to_owned());
    }
    let mut maximum = 0.0f32;
    for (index, (left, right)) in left.iter().zip(right).enumerate() {
        if left.valid != right.valid {
            return Err(format!(
                "Jacobi node {index} validity changed between iterations."
            ));
        }
        if !left.valid {
            continue;
        }
        if left.coverages.len() != right.coverages.len() {
            return Err(format!(
                "Jacobi node {index} topology changed between iterations."
            ));
        }
        let l1 = left
            .coverages
            .iter()
            .zip(&right.coverages)
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>();
        maximum = maximum.max(l1);
    }
    Ok(maximum)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(iterations: u16, self_weight: f32) -> JacobiSixNeighborPolicy {
        JacobiSixNeighborPolicy {
            schema_version: JACOBI_SIX_NEIGHBOR_POLICY_SCHEMA_VERSION,
            iterations,
            self_weight,
        }
    }

    fn fixture(shape: JacobiGridShape) -> Vec<ContinuityFieldNode> {
        let count = shape.node_count().unwrap();
        (0..count)
            .map(|index| {
                let phase = index as f32 / (count.saturating_sub(1).max(1) as f32);
                ContinuityFieldNode::valid(vec![phase, 1.0 - phase])
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

    #[test]
    fn synchronous_updates_are_independent_of_node_loop_order() {
        let shape = JacobiGridShape { l: 3, a: 3, b: 3 };
        let initial = fixture(shape);
        let forward = (0..initial.len()).collect::<Vec<_>>();
        let mut reverse = forward.clone();
        reverse.reverse();

        let first = build_with_order(
            &initial,
            shape,
            policy(5, 0.35),
            &forward,
            deterministic_solver,
        )
        .unwrap();
        let second = build_with_order(
            &initial,
            shape,
            policy(5, 0.35),
            &reverse,
            deterministic_solver,
        )
        .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn unsupported_nodes_remain_unsupported_and_are_ignored_by_neighbors() {
        let shape = JacobiGridShape { l: 3, a: 1, b: 1 };
        let mut initial = vec![
            ContinuityFieldNode::valid(vec![0.0, 1.0]),
            ContinuityFieldNode::unsupported(2),
            ContinuityFieldNode::valid(vec![1.0, 0.0]),
        ];
        let result =
            build_jacobi_six_neighbor_field(&initial, shape, policy(3, 0.0), |_, reference| {
                Ok(reference.to_vec())
            })
            .unwrap();
        assert!(!result.nodes[1].valid);
        assert_eq!(result.nodes[0].coverages, vec![0.0, 1.0]);
        assert_eq!(result.nodes[2].coverages, vec![1.0, 0.0]);
        initial[1] = ContinuityFieldNode::valid(vec![0.5, 0.5]);
        let connected =
            build_jacobi_six_neighbor_field(&initial, shape, policy(1, 0.0), |_, reference| {
                Ok(reference.to_vec())
            })
            .unwrap();
        assert_ne!(connected.nodes[0].coverages, result.nodes[0].coverages);
    }

    #[test]
    fn period_two_diagnostic_exposes_oscillation_without_nondeterministic_stopping() {
        let shape = JacobiGridShape { l: 1, a: 1, b: 1 };
        let initial = vec![ContinuityFieldNode::valid(vec![0.2])];
        let result =
            build_jacobi_six_neighbor_field(&initial, shape, policy(4, 1.0), |_, reference| {
                Ok(vec![1.0 - reference[0]])
            })
            .unwrap();
        assert!(
            result
                .diagnostics
                .iter()
                .all(|item| item.max_l1_from_previous > 0.5)
        );
        let period_two_epsilon = 1.0e-6f32;
        for diagnostic in &result.diagnostics[1..] {
            assert!(
                diagnostic
                    .max_l1_from_two_back
                    .is_some_and(|delta| delta <= period_two_epsilon),
                "expected period-two recurrence within {period_two_epsilon}, got {:?}",
                diagnostic.max_l1_from_two_back,
            );
        }
    }

    #[test]
    fn policy_and_input_validation_fail_closed() {
        assert!(policy(0, 0.5).validate().is_err());
        assert!(policy(1, f32::NAN).validate().is_err());
        assert!(policy(1, 1.1).validate().is_err());

        let shape = JacobiGridShape { l: 2, a: 1, b: 1 };
        let bad = vec![
            ContinuityFieldNode::valid(vec![0.0]),
            ContinuityFieldNode::valid(vec![f32::NAN]),
        ];
        assert!(
            build_jacobi_six_neighbor_field(&bad, shape, policy(1, 0.5), |_, reference| {
                Ok(reference.to_vec())
            })
            .is_err()
        );
    }

    #[test]
    fn repeated_runs_are_bitwise_deterministic_for_deterministic_solver() {
        let shape = JacobiGridShape { l: 4, a: 2, b: 2 };
        let initial = fixture(shape);
        let first =
            build_jacobi_six_neighbor_field(&initial, shape, policy(8, 0.4), deterministic_solver)
                .unwrap();
        let second =
            build_jacobi_six_neighbor_field(&initial, shape, policy(8, 0.4), deterministic_solver)
                .unwrap();
        assert_eq!(first, second);
    }
}
