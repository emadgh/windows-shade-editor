use crate::color_conversion::{ConversionTargetDefinition, SeparationStrategy};
use crate::custom_optimizer_config::{CustomOptimizerSolverConfig, CustomOptimizerSolverMethod};
use crate::device_characterization::{DeviceForwardModel, LabColor};
use crate::inverse_lut_continuity_field::{
    ContinuityFieldNode, JacobiFieldResult, JacobiGridShape, JacobiSixNeighborPolicy,
    JACOBI_SIX_NEIGHBOR_POLICY_SCHEMA_VERSION, build_jacobi_six_neighbor_field,
};
use crate::inverse_lut_identity::{
    InverseLutContinuityFieldMethod, InverseLutContinuitySeedMethod, LabGridSpec,
};
use crate::inverse_separation_solver::{
    InverseSolveError, solve_inverse_separation, solve_inverse_separation_with_reference,
};
use crate::separation_optimizer::CandidateScoringWeights;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JacobiContinuityBuildStats {
    pub seed_attempts: u64,
    pub seed_supported_nodes: u64,
    pub seed_unsupported_nodes: u64,
    pub continuity_solves: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuiltJacobiContinuityField {
    pub shape: JacobiGridShape,
    pub field: JacobiFieldResult,
    pub stats: JacobiContinuityBuildStats,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JacobiContinuityBuildError {
    InvalidGrid(Vec<String>),
    InvalidFieldPolicy(Vec<String>),
    InvalidSolver(Vec<String>),
    RequiresPositiveContinuityV2,
    SeedSolveFailed {
        index: usize,
        error: InverseSolveError,
    },
    ContinuityIterationFailed(String),
    WorkCounterOverflow,
}

/// Construct the exact deterministic CIE Lab sample grid used by the offline
/// inverse-LUT field builder.
///
/// Node order is part of the V1 construction contract: L* is the outer axis,
/// a* is the middle axis and b* is the innermost/fastest axis. Endpoints are
/// included exactly and interior samples use a fixed linear interpolation.
pub fn lab_grid_points(
    grid: LabGridSpec,
) -> Result<(JacobiGridShape, Vec<LabColor>), JacobiContinuityBuildError> {
    grid.validate().map_err(JacobiContinuityBuildError::InvalidGrid)?;
    let shape = JacobiGridShape {
        l: usize::from(grid.l_samples),
        a: usize::from(grid.a_samples),
        b: usize::from(grid.b_samples),
    };
    let node_count = shape
        .node_count()
        .ok_or_else(|| JacobiContinuityBuildError::InvalidGrid(vec![
            "Jacobi Lab grid node count overflowed usize.".to_owned(),
        ]))?;
    let mut points = Vec::with_capacity(node_count);
    for l_index in 0..shape.l {
        let l = sample_axis(grid.l_min, grid.l_max, l_index, shape.l);
        for a_index in 0..shape.a {
            let a = sample_axis(grid.a_min, grid.a_max, a_index, shape.a);
            for b_index in 0..shape.b {
                let b = sample_axis(grid.b_min, grid.b_max, b_index, shape.b);
                points.push(LabColor { l, a, b });
            }
        }
    }
    Ok((shape, points))
}

/// Build the positive-weight V2 continuity field using the frozen offline rule:
///
/// 1. Every grid node is independently solved with BoundedHaltonBeamV1 using
///    the exact V2 solver's numerical search knobs but no continuity preference.
///    This is the deterministic seed for every connected supported region.
/// 2. No-feasible independent seed nodes remain explicitly unsupported.
/// 3. Every Jacobi iteration builds six-neighbor references from one immutable
///    previous snapshot and solves supported nodes with the real positive-weight
///    BoundedHaltonBeamContinuityV2 solver.
/// 4. A solve failure after seeding fails the complete field; validity never
///    changes merely because a continuity reference was difficult.
///
/// The function has no image/raster state and no traversal-dependent feedback.
pub fn build_positive_v2_jacobi_field(
    target: &ConversionTargetDefinition,
    strategy: &SeparationStrategy,
    weights: CandidateScoringWeights,
    model: &dyn DeviceForwardModel,
    grid: LabGridSpec,
    solver_config: CustomOptimizerSolverConfig,
    field_method: InverseLutContinuityFieldMethod,
) -> Result<BuiltJacobiContinuityField, JacobiContinuityBuildError> {
    grid.validate().map_err(JacobiContinuityBuildError::InvalidGrid)?;
    field_method
        .validate_for_grid(&grid)
        .map_err(JacobiContinuityBuildError::InvalidFieldPolicy)?;
    solver_config
        .validate(target.channels.len())
        .map_err(JacobiContinuityBuildError::InvalidSolver)?;

    let (seed_method, iterations, self_weight) = match field_method {
        InverseLutContinuityFieldMethod::JacobiSixNeighborV1 {
            seed_method,
            iterations,
            self_weight,
        } => (seed_method, iterations, self_weight),
        InverseLutContinuityFieldMethod::IndependentNodeSolvesV1 => {
            return Err(JacobiContinuityBuildError::RequiresPositiveContinuityV2);
        }
    };
    if seed_method != InverseLutContinuitySeedMethod::IndependentV1NodeSolveV1 {
        return Err(JacobiContinuityBuildError::InvalidFieldPolicy(vec![
            "Unsupported Jacobi continuity-field seed method.".to_owned(),
        ]));
    }
    let continuity = match (solver_config.method, solver_config.continuity_preference) {
        (CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2, Some(policy))
            if policy.weight > 0.0 => policy,
        _ => return Err(JacobiContinuityBuildError::RequiresPositiveContinuityV2),
    };
    debug_assert!(continuity.weight > 0.0);

    let seed_config = independent_seed_config(solver_config);
    seed_config
        .validate(target.channels.len())
        .map_err(JacobiContinuityBuildError::InvalidSolver)?;

    let (shape, labs) = lab_grid_points(grid)?;
    let channel_count = target.channels.len();
    let mut initial = Vec::with_capacity(labs.len());
    let mut stats = JacobiContinuityBuildStats {
        seed_attempts: u64::try_from(labs.len())
            .map_err(|_| JacobiContinuityBuildError::WorkCounterOverflow)?,
        ..JacobiContinuityBuildStats::default()
    };

    for (index, lab) in labs.iter().copied().enumerate() {
        match solve_inverse_separation(
            target,
            strategy,
            weights,
            model,
            lab,
            seed_config,
        ) {
            Ok(result) => {
                initial.push(ContinuityFieldNode::valid(result.candidate.coverages));
                stats.seed_supported_nodes = stats
                    .seed_supported_nodes
                    .checked_add(1)
                    .ok_or(JacobiContinuityBuildError::WorkCounterOverflow)?;
            }
            Err(InverseSolveError::NoFeasibleCandidate) => {
                initial.push(ContinuityFieldNode::unsupported(channel_count));
                stats.seed_unsupported_nodes = stats
                    .seed_unsupported_nodes
                    .checked_add(1)
                    .ok_or(JacobiContinuityBuildError::WorkCounterOverflow)?;
            }
            Err(error) => {
                return Err(JacobiContinuityBuildError::SeedSolveFailed { index, error });
            }
        }
    }

    stats.continuity_solves = stats
        .seed_supported_nodes
        .checked_mul(u64::from(iterations))
        .ok_or(JacobiContinuityBuildError::WorkCounterOverflow)?;

    let policy = JacobiSixNeighborPolicy {
        schema_version: JACOBI_SIX_NEIGHBOR_POLICY_SCHEMA_VERSION,
        iterations,
        self_weight,
    };
    let field = build_jacobi_six_neighbor_field(&initial, shape, policy, |index, reference| {
        solve_inverse_separation_with_reference(
            target,
            strategy,
            weights,
            model,
            labs[index],
            solver_config,
            Some(reference),
        )
        .map(|result| result.candidate.coverages)
        .map_err(|error| format!("{error:?}"))
    })
    .map_err(JacobiContinuityBuildError::ContinuityIterationFailed)?;

    Ok(BuiltJacobiContinuityField {
        shape,
        field,
        stats,
    })
}

fn independent_seed_config(config: CustomOptimizerSolverConfig) -> CustomOptimizerSolverConfig {
    CustomOptimizerSolverConfig {
        method: CustomOptimizerSolverMethod::BoundedHaltonBeamV1,
        continuity_preference: None,
        ..config
    }
}

fn sample_axis(minimum: f64, maximum: f64, index: usize, samples: usize) -> f64 {
    debug_assert!(samples >= 2);
    if index == 0 {
        return minimum;
    }
    if index + 1 == samples {
        return maximum;
    }
    let t = index as f64 / (samples - 1) as f64;
    minimum + (maximum - minimum) * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_optimizer_config::{
        ContinuityDistanceMetric, ContinuityPreferenceConfig,
    };
    use crate::inverse_lut_identity::{
        INVERSE_LUT_JACOBI_FIELD_METHOD_MAX_ITERATIONS, InverseLutContinuityFieldMethod,
        InverseLutContinuitySeedMethod,
    };

    fn grid() -> LabGridSpec {
        LabGridSpec {
            l_min: 0.0,
            l_max: 100.0,
            l_samples: 3,
            a_min: -1.0,
            a_max: 1.0,
            a_samples: 2,
            b_min: -2.0,
            b_max: 2.0,
            b_samples: 2,
        }
    }

    #[test]
    fn lab_grid_order_is_l_then_a_then_b_with_b_fastest() {
        let (shape, points) = lab_grid_points(grid()).unwrap();
        assert_eq!(shape, JacobiGridShape { l: 3, a: 2, b: 2 });
        assert_eq!(points.len(), 12);
        assert_eq!(points[0], LabColor { l: 0.0, a: -1.0, b: -2.0 });
        assert_eq!(points[1], LabColor { l: 0.0, a: -1.0, b: 2.0 });
        assert_eq!(points[2], LabColor { l: 0.0, a: 1.0, b: -2.0 });
        assert_eq!(points[4], LabColor { l: 50.0, a: -1.0, b: -2.0 });
        assert_eq!(points[11], LabColor { l: 100.0, a: 1.0, b: 2.0 });
    }

    #[test]
    fn independent_seed_preserves_search_knobs_and_removes_continuity_state() {
        let source = CustomOptimizerSolverConfig {
            method: CustomOptimizerSolverMethod::BoundedHaltonBeamContinuityV2,
            initial_samples: 777,
            beam_width: 31,
            refinement_rounds: 5,
            initial_step_fraction: 0.17,
            step_decay: 0.42,
            preference_delta_e00: 0.33,
            continuity_preference: Some(ContinuityPreferenceConfig {
                weight: 4.0,
                distance_metric: ContinuityDistanceMetric::NormalizedL1,
                max_normalized_channel_jump: 0.2,
                dominant_channel_switch_penalty: 0.5,
            }),
            ..CustomOptimizerSolverConfig::default()
        };
        let seed = independent_seed_config(source);
        assert_eq!(seed.method, CustomOptimizerSolverMethod::BoundedHaltonBeamV1);
        assert_eq!(seed.continuity_preference, None);
        assert_eq!(seed.initial_samples, source.initial_samples);
        assert_eq!(seed.beam_width, source.beam_width);
        assert_eq!(seed.refinement_rounds, source.refinement_rounds);
        assert_eq!(seed.initial_step_fraction, source.initial_step_fraction);
        assert_eq!(seed.step_decay, source.step_decay);
        assert_eq!(seed.preference_delta_e00, source.preference_delta_e00);
    }

    #[test]
    fn builder_contract_uses_only_the_frozen_jacobi_seed_method() {
        let method = InverseLutContinuityFieldMethod::JacobiSixNeighborV1 {
            seed_method: InverseLutContinuitySeedMethod::IndependentV1NodeSolveV1,
            iterations: INVERSE_LUT_JACOBI_FIELD_METHOD_MAX_ITERATIONS.min(8),
            self_weight: 0.35,
        };
        assert!(method.validate_for_grid(&grid()).is_ok());
    }
}
