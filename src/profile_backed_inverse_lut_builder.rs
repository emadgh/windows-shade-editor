use crate::color_conversion::ConversionRecipe;
use crate::custom_optimizer_config::{CustomOptimizerObjectiveWeights, CustomOptimizerSolverConfig};
use crate::conversion_transaction::ConversionCancellation;
use crate::device_characterization::DeviceForwardModel;
use crate::inverse_lut_continuity_builder::{
    BuiltJacobiContinuityField, JacobiContinuityBuildError, build_positive_v2_jacobi_field,
    lab_grid_points,
};
use crate::inverse_lut_identity::{InverseLutBuildPolicy, InverseLutContinuityFieldMethod};
use crate::inverse_separation_solver::{InverseSolveError, InverseSolverStats, solve_inverse_separation};
use crate::output_icc_forward_model::OutputIccForwardModel;
use crate::separation_optimizer::CandidateScoringWeights;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileBackedForwardModelMethod {
    OutputIccDeviceToPcsV1,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BuiltProfileBackedInverseLutPayload {
    pub forward_model_method: ProfileBackedForwardModelMethod,
    pub forward_model_id: String,
    pub channel_names: Vec<String>,
    pub target_bit_depth: u8,
    pub build_policy: InverseLutBuildPolicy,
    pub validity: Vec<bool>,
    pub coverages: Vec<f32>,
    pub stats: ProfileBackedInverseLutBuildStats,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProfileBackedInverseLutBuildStats {
    pub node_count: u64,
    pub supported_nodes: u64,
    pub unsupported_nodes: u64,
    pub attempted_candidates: u64,
    pub characterized_candidates: u64,
    pub feasible_candidates: u64,
    pub forward_rejected_candidates: u64,
    pub constraint_rejected_candidates: u64,
    pub continuity_seed_attempts: u64,
    pub continuity_solves: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProfileBackedInverseLutBuildError {
    Cancelled,
    InvalidRecipe(Vec<String>),
    MissingSolverConfig,
    MissingObjectiveWeightProvenance(Vec<String>),
    InvalidBuildPolicy(Vec<String>),
    ForwardModelIdentityMismatch {
        expected: String,
        actual: String,
    },
    ChannelTopologyMismatch {
        expected: Vec<String>,
        actual: Vec<String>,
    },
    Grid(JacobiContinuityBuildError),
    SolveFailed {
        index: usize,
        error: InverseSolveError,
    },
    ContinuityField(JacobiContinuityBuildError),
    CounterOverflow,
    PayloadTopology(String),
}

#[derive(Clone, Debug)]
struct BuiltPayloadParts {
    validity: Vec<bool>,
    coverages: Vec<f32>,
    stats: ProfileBackedInverseLutBuildStats,
}

/// Build an inverse-LUT payload from the exact Output ICC forward model introduced in #484.
///
/// This constructor deliberately has no measured-characterization parameter. The inverse
/// solver still enforces the exact Output ICC SHA-derived model identity, channel order,
/// per-channel limits, total-ink limit, color-error policy and separation preferences.
/// Artifact identity/publication and execution authority are separate #486 layers.
pub fn build_output_icc_inverse_lut_payload(
    recipe: &ConversionRecipe,
    model: &OutputIccForwardModel,
    build_policy: InverseLutBuildPolicy,
) -> Result<BuiltProfileBackedInverseLutPayload, ProfileBackedInverseLutBuildError> {
    build_device_forward_model_payload(
        recipe,
        model,
        ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1,
        build_policy,
        None,
    )
}

/// Candidate-facing variant with cooperative cancellation. Engine preparation must
/// periodically honor the shared Candidate cancellation token so stale generations
/// cannot keep consuming CPU after the operator changes target or settings.
pub fn build_output_icc_inverse_lut_payload_with_cancellation(
    recipe: &ConversionRecipe,
    model: &OutputIccForwardModel,
    build_policy: InverseLutBuildPolicy,
    cancellation: &ConversionCancellation,
) -> Result<BuiltProfileBackedInverseLutPayload, ProfileBackedInverseLutBuildError> {
    build_device_forward_model_payload(
        recipe,
        model,
        ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1,
        build_policy,
        Some(cancellation),
    )
}

fn build_device_forward_model_payload(
    recipe: &ConversionRecipe,
    model: &dyn DeviceForwardModel,
    method: ProfileBackedForwardModelMethod,
    build_policy: InverseLutBuildPolicy,
    cancellation: Option<&ConversionCancellation>,
) -> Result<BuiltProfileBackedInverseLutPayload, ProfileBackedInverseLutBuildError> {
    check_cancellation(cancellation)?;
    recipe
        .validate()
        .map_err(ProfileBackedInverseLutBuildError::InvalidRecipe)?;
    build_policy
        .validate()
        .map_err(ProfileBackedInverseLutBuildError::InvalidBuildPolicy)?;

    let solver = recipe
        .custom_optimizer_solver
        .ok_or(ProfileBackedInverseLutBuildError::MissingSolverConfig)?;
    solver
        .validate(recipe.target.channels.len())
        .map_err(ProfileBackedInverseLutBuildError::MissingObjectiveWeightProvenance)?;
    let objective = solver.objective_weights.ok_or_else(|| {
        ProfileBackedInverseLutBuildError::MissingObjectiveWeightProvenance(vec![
            "Custom Optimizer objective-weight provenance is missing; recapture the recipe before profile-backed inverse-LUT construction."
                .to_owned(),
        ])
    })?;
    objective
        .validate()
        .map_err(ProfileBackedInverseLutBuildError::MissingObjectiveWeightProvenance)?;

    let expected_model_id = crate::optimizer_forward_model_authority::optimizer_forward_model_identity(
        &recipe.target,
    )
    .map_err(|error| {
        ProfileBackedInverseLutBuildError::InvalidRecipe(vec![format!(
            "Cannot resolve Custom Optimizer forward-model authority: {error:?}"
        )])
    })?;
    if expected_model_id != model.identity().id {
        return Err(ProfileBackedInverseLutBuildError::ForwardModelIdentityMismatch {
            expected: expected_model_id,
            actual: model.identity().id.clone(),
        });
    }
    let expected_channels = recipe
        .target
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect::<Vec<_>>();
    if expected_channels != model.identity().channel_names {
        return Err(ProfileBackedInverseLutBuildError::ChannelTopologyMismatch {
            expected: expected_channels,
            actual: model.identity().channel_names.clone(),
        });
    }

    let weights = runtime_weights(objective);
    let parts = match build_policy.continuity_field {
        InverseLutContinuityFieldMethod::IndependentNodeSolvesV1 => build_independent_payload(
            recipe,
            model,
            build_policy,
            solver,
            weights,
            cancellation,
        )?,
        InverseLutContinuityFieldMethod::JacobiSixNeighborV1 { .. } => build_continuity_payload(
            recipe,
            model,
            build_policy,
            solver,
            weights,
            cancellation,
        )?,
    };
    check_cancellation(cancellation)?;
    validate_payload(
        recipe.target.channels.len(),
        build_policy,
        &parts.validity,
        &parts.coverages,
    )?;

    Ok(BuiltProfileBackedInverseLutPayload {
        forward_model_method: method,
        forward_model_id: model.identity().id.clone(),
        channel_names: model.identity().channel_names.clone(),
        target_bit_depth: recipe.target.bit_depth,
        build_policy,
        validity: parts.validity,
        coverages: parts.coverages,
        stats: parts.stats,
    })
}

fn build_independent_payload(
    recipe: &ConversionRecipe,
    model: &dyn DeviceForwardModel,
    build_policy: InverseLutBuildPolicy,
    solver: CustomOptimizerSolverConfig,
    weights: CandidateScoringWeights,
    cancellation: Option<&ConversionCancellation>,
) -> Result<BuiltPayloadParts, ProfileBackedInverseLutBuildError> {
    let (_shape, labs) = lab_grid_points(build_policy.grid)
        .map_err(ProfileBackedInverseLutBuildError::Grid)?;
    let channel_count = recipe.target.channels.len();
    let node_count = u64::try_from(labs.len())
        .map_err(|_| ProfileBackedInverseLutBuildError::CounterOverflow)?;
    let coverage_values = labs
        .len()
        .checked_mul(channel_count)
        .ok_or(ProfileBackedInverseLutBuildError::CounterOverflow)?;
    let mut validity = Vec::with_capacity(labs.len());
    let mut coverages = Vec::with_capacity(coverage_values);
    let mut stats = ProfileBackedInverseLutBuildStats {
        node_count,
        ..ProfileBackedInverseLutBuildStats::default()
    };

    for (index, lab) in labs.iter().copied().enumerate() {
        check_cancellation(cancellation)?;
        match solve_inverse_separation(
            &recipe.target,
            &recipe.strategy,
            weights,
            model,
            lab,
            solver,
        ) {
            Ok(result) => {
                if result.candidate.coverages.len() != channel_count {
                    return Err(ProfileBackedInverseLutBuildError::PayloadTopology(format!(
                        "Profile-backed inverse LUT node {index} solver topology mismatch: expected {channel_count}, got {}.",
                        result.candidate.coverages.len()
                    )));
                }
                validity.push(true);
                coverages.extend_from_slice(&result.candidate.coverages);
                stats.supported_nodes = checked_add(stats.supported_nodes, 1)?;
                accumulate_solver_stats(&mut stats, result.stats)?;
            }
            Err(InverseSolveError::NoFeasibleCandidate) => {
                validity.push(false);
                coverages.extend(std::iter::repeat_n(0.0, channel_count));
                stats.unsupported_nodes = checked_add(stats.unsupported_nodes, 1)?;
            }
            Err(error) => {
                return Err(ProfileBackedInverseLutBuildError::SolveFailed { index, error });
            }
        }
    }

    Ok(BuiltPayloadParts {
        validity,
        coverages,
        stats,
    })
}

fn build_continuity_payload(
    recipe: &ConversionRecipe,
    model: &dyn DeviceForwardModel,
    build_policy: InverseLutBuildPolicy,
    solver: CustomOptimizerSolverConfig,
    weights: CandidateScoringWeights,
    cancellation: Option<&ConversionCancellation>,
) -> Result<BuiltPayloadParts, ProfileBackedInverseLutBuildError> {
    check_cancellation(cancellation)?;
    let field = build_positive_v2_jacobi_field(
        &recipe.target,
        &recipe.strategy,
        weights,
        model,
        build_policy.grid,
        solver,
        build_policy.continuity_field,
    )
    .map_err(ProfileBackedInverseLutBuildError::ContinuityField)?;
    check_cancellation(cancellation)?;
    flatten_continuity_field(recipe.target.channels.len(), field)
}

fn check_cancellation(
    cancellation: Option<&ConversionCancellation>,
) -> Result<(), ProfileBackedInverseLutBuildError> {
    if cancellation.is_some_and(ConversionCancellation::is_requested) {
        Err(ProfileBackedInverseLutBuildError::Cancelled)
    } else {
        Ok(())
    }
}

fn flatten_continuity_field(
    channel_count: usize,
    built: BuiltJacobiContinuityField,
) -> Result<BuiltPayloadParts, ProfileBackedInverseLutBuildError> {
    let node_count = built.field.nodes.len();
    let mut validity = Vec::with_capacity(node_count);
    let mut coverages = Vec::with_capacity(
        node_count
            .checked_mul(channel_count)
            .ok_or(ProfileBackedInverseLutBuildError::CounterOverflow)?,
    );
    for (index, node) in built.field.nodes.into_iter().enumerate() {
        if node.coverages.len() != channel_count {
            return Err(ProfileBackedInverseLutBuildError::PayloadTopology(format!(
                "Profile-backed continuity-field node {index} topology mismatch: expected {channel_count}, got {}.",
                node.coverages.len()
            )));
        }
        validity.push(node.valid);
        if node.valid {
            coverages.extend(node.coverages);
        } else {
            coverages.extend(std::iter::repeat_n(0.0, channel_count));
        }
    }
    let supported_nodes = u64::try_from(validity.iter().filter(|value| **value).count())
        .map_err(|_| ProfileBackedInverseLutBuildError::CounterOverflow)?;
    let node_count_u64 = u64::try_from(node_count)
        .map_err(|_| ProfileBackedInverseLutBuildError::CounterOverflow)?;
    let unsupported_nodes = node_count_u64
        .checked_sub(supported_nodes)
        .ok_or(ProfileBackedInverseLutBuildError::CounterOverflow)?;
    Ok(BuiltPayloadParts {
        validity,
        coverages,
        stats: ProfileBackedInverseLutBuildStats {
            node_count: node_count_u64,
            supported_nodes,
            unsupported_nodes,
            continuity_seed_attempts: built.stats.seed_attempts,
            continuity_solves: built.stats.continuity_solves,
            ..ProfileBackedInverseLutBuildStats::default()
        },
    })
}

fn runtime_weights(objective: CustomOptimizerObjectiveWeights) -> CandidateScoringWeights {
    CandidateScoringWeights {
        color_error: objective.color_error,
        ink_preference: objective.ink_preference,
        neutral_black: objective.neutral_black,
        total_ink: objective.total_ink,
    }
}

fn accumulate_solver_stats(
    destination: &mut ProfileBackedInverseLutBuildStats,
    source: InverseSolverStats,
) -> Result<(), ProfileBackedInverseLutBuildError> {
    destination.attempted_candidates =
        checked_add(destination.attempted_candidates, source.attempted as u64)?;
    destination.characterized_candidates = checked_add(
        destination.characterized_candidates,
        source.characterized as u64,
    )?;
    destination.feasible_candidates =
        checked_add(destination.feasible_candidates, source.feasible as u64)?;
    destination.forward_rejected_candidates = checked_add(
        destination.forward_rejected_candidates,
        source.forward_rejected as u64,
    )?;
    destination.constraint_rejected_candidates = checked_add(
        destination.constraint_rejected_candidates,
        source.constraint_rejected as u64,
    )?;
    Ok(())
}

fn checked_add(
    left: u64,
    right: u64,
) -> Result<u64, ProfileBackedInverseLutBuildError> {
    left.checked_add(right)
        .ok_or(ProfileBackedInverseLutBuildError::CounterOverflow)
}

fn validate_payload(
    channel_count: usize,
    build_policy: InverseLutBuildPolicy,
    validity: &[bool],
    coverages: &[f32],
) -> Result<(), ProfileBackedInverseLutBuildError> {
    let expected_nodes = build_policy
        .grid
        .node_count()
        .ok_or(ProfileBackedInverseLutBuildError::CounterOverflow)?;
    if validity.len() as u64 != expected_nodes {
        return Err(ProfileBackedInverseLutBuildError::PayloadTopology(format!(
            "Profile-backed inverse LUT validity count mismatch: expected {expected_nodes}, got {}.",
            validity.len()
        )));
    }
    let expected_values = usize::try_from(expected_nodes)
        .ok()
        .and_then(|nodes| nodes.checked_mul(channel_count))
        .ok_or(ProfileBackedInverseLutBuildError::CounterOverflow)?;
    if coverages.len() != expected_values {
        return Err(ProfileBackedInverseLutBuildError::PayloadTopology(format!(
            "Profile-backed inverse LUT coverage count mismatch: expected {expected_values}, got {}.",
            coverages.len()
        )));
    }
    for (node_index, valid) in validity.iter().copied().enumerate() {
        let start = node_index * channel_count;
        for (channel_index, value) in coverages[start..start + channel_count]
            .iter()
            .copied()
            .enumerate()
        {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(ProfileBackedInverseLutBuildError::PayloadTopology(format!(
                    "Profile-backed inverse LUT node {node_index} channel {channel_index} is outside normalized finite coverage."
                )));
            }
            if !valid && value.to_bits() != 0 {
                return Err(ProfileBackedInverseLutBuildError::PayloadTopology(format!(
                    "Profile-backed inverse LUT invalid node {node_index} channel {channel_index} is not canonical positive zero."
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRenderingIntent,
        ConversionTargetDefinition, SeparationStrategy, TargetChannelDefinition,
    };
    use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
    use crate::device_characterization::{CharacterizationIdentity, LabColor};
    use crate::inverse_lut_identity::{
        INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION, InverseLutInterpolationMethod,
        InverseLutNumericalPrecision, InverseLutOutputQuantization, InverseLutValidityEncoding,
        LabGridSpec,
    };
    use crate::model::IccProfileIdentity;

    struct ProfileIdentityFixture {
        identity: CharacterizationIdentity,
    }

    impl DeviceForwardModel for ProfileIdentityFixture {
        fn identity(&self) -> &CharacterizationIdentity {
            &self.identity
        }

        fn predict_lab(&self, coverages: &[f32]) -> Result<LabColor, String> {
            if coverages.len() != 4 {
                return Err("fixture topology mismatch".to_owned());
            }
            let blue = f64::from(coverages[0]);
            let brown = f64::from(coverages[1]);
            let beige = f64::from(coverages[2]);
            let black = f64::from(coverages[3]);
            Ok(LabColor {
                l: 95.0 - 18.0 * blue - 16.0 * brown - 10.0 * beige - 44.0 * black,
                a: -2.0 * blue + 5.0 * brown + beige,
                b: -8.0 * blue + 6.0 * brown + 2.0 * beige,
            })
        }
    }

    fn recipe() -> ConversionRecipe {
        let hash = "a".repeat(64);
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: "source-hash".to_owned(),
            },
            source_transparency_policy: None,
            target: ConversionTargetDefinition {
                name: "Existing Output ICC 4C".to_owned(),
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
                output_profile_identity: Some(IccProfileIdentity {
                    description: "Existing ceramic Output ICC".to_owned(),
                    sha256: hash,
                }),
                output_profile_path: Some("Ceramic.icc".to_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: Some(1.8),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy {
                black_channel: Some("Black".to_owned()),
                black_generation_strength: 1.0,
                black_max: 0.8,
                neutral_chroma_threshold: 8.0,
                max_delta_e00: Some(3.0),
                ..SeparationStrategy::default()
            },
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig {
                initial_samples: 48,
                beam_width: 6,
                refinement_rounds: 1,
                initial_step_fraction: 0.15,
                step_decay: 0.5,
                preference_delta_e00: 0.2,
                ..CustomOptimizerSolverConfig::default()
            }),
        }
    }

    fn model() -> ProfileIdentityFixture {
        ProfileIdentityFixture {
            identity: CharacterizationIdentity {
                id: format!("output-icc-sha256:{}", "a".repeat(64)),
                channel_names: ["Blue", "Brown", "Beige", "Black"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            },
        }
    }

    fn policy() -> InverseLutBuildPolicy {
        InverseLutBuildPolicy {
            schema_version: INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION,
            grid: LabGridSpec {
                l_min: 70.0,
                l_max: 90.0,
                l_samples: 2,
                a_min: -2.0,
                a_max: 2.0,
                a_samples: 2,
                b_min: -2.0,
                b_max: 2.0,
                b_samples: 2,
            },
            interpolation: InverseLutInterpolationMethod::TrilinearV1,
            validity_encoding: InverseLutValidityEncoding::ExplicitNodeValidityMaskV1,
            numerical_precision: InverseLutNumericalPrecision::NormalizedF32V1,
            output_quantization: InverseLutOutputQuantization::ClampScaleRoundV1,
            continuity_field: InverseLutContinuityFieldMethod::IndependentNodeSolvesV1,
        }
    }

    #[test]
    fn generic_builder_is_deterministic_for_output_icc_identity() {
        let first = build_device_forward_model_payload(
            &recipe(),
            &model(),
            ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1,
            policy(),
            None,
        )
        .unwrap();
        let second = build_device_forward_model_payload(
            &recipe(),
            &model(),
            ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1,
            policy(),
            None,
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.forward_model_method, ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1);
        assert_eq!(first.forward_model_id, format!("output-icc-sha256:{}", "a".repeat(64)));
        assert_eq!(first.stats.node_count, 8);
        assert_eq!(first.stats.supported_nodes + first.stats.unsupported_nodes, 8);
    }

    #[test]
    fn generic_builder_rejects_wrong_profile_identity_before_grid_search() {
        let mut wrong = model();
        wrong.identity.id = format!("output-icc-sha256:{}", "b".repeat(64));
        let error = build_device_forward_model_payload(
            &recipe(),
            &wrong,
            ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1,
            policy(),
            None,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ProfileBackedInverseLutBuildError::ForwardModelIdentityMismatch { .. }
        ));
    }

    #[test]
    fn stale_candidate_lut_build_honors_cancellation_before_solver_work() {
        let cancellation = ConversionCancellation::default();
        cancellation.request();
        let error = build_device_forward_model_payload(
            &recipe(),
            &model(),
            ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1,
            policy(),
            Some(&cancellation),
        )
        .unwrap_err();
        assert!(matches!(error, ProfileBackedInverseLutBuildError::Cancelled));

        let source = include_str!("profile_backed_inverse_lut_builder.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        let loop_start = runtime.find("for (index, lab) in labs.iter().copied().enumerate()").unwrap();
        let solve = runtime[loop_start..].find("solve_inverse_separation(").unwrap();
        assert!(runtime[loop_start..loop_start + solve].contains("check_cancellation"));
    }

    #[test]
    fn profile_builder_contract_is_typed_to_output_icc_model() {
        let source = include_str!("profile_backed_inverse_lut_builder.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("model: &OutputIccForwardModel"));
        assert!(runtime.contains("OutputIccDeviceToPcsV1"));
        assert!(runtime.contains("solve_inverse_separation"));
        assert!(!runtime.contains("ValidatedLocalForwardModel"));
    }
}
