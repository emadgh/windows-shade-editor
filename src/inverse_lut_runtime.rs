use std::path::Path;

use sha2::{Digest, Sha256};

use crate::color_conversion::ConversionRecipe;
use crate::custom_optimizer_config::CustomOptimizerObjectiveWeights;
use crate::device_characterization::LabColor;
use crate::device_characterization_model::ValidatedLocalForwardModel;
use crate::inverse_lut_artifact::{
    InverseLutPublishOutcome, VerifiedInverseLutArtifact, publish_inverse_lut_artifact_if_absent,
};
use crate::inverse_lut_continuity_builder::{
    BuiltJacobiContinuityField, JacobiContinuityBuildError, build_positive_v2_jacobi_field,
    lab_grid_points,
};
use crate::inverse_lut_identity::{
    InverseLutBuildPolicy, InverseLutContinuityFieldMethod, InverseLutIdentityError,
    InverseLutIdentityRecord, InverseLutInterpolationMethod, quantize_normalized_coverage,
};
use crate::inverse_separation_solver::{
    InverseSolveError, InverseSolverStats, solve_inverse_separation,
};
use crate::separation_optimizer::CandidateScoringWeights;

const GRID_NODE_SNAP_EPSILON: f64 = 1.0e-10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InverseLutBuildStats {
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
pub struct BuiltInverseLutPayload {
    pub identity: InverseLutIdentityRecord,
    pub validity: Vec<bool>,
    /// Node-major normalized coverage values in authoritative channel order.
    pub coverages: Vec<f32>,
    pub stats: InverseLutBuildStats,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InverseLutBuildError {
    Identity(InverseLutIdentityError),
    MissingSolverConfig,
    MissingObjectiveWeightProvenance(Vec<String>),
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
    stats: InverseLutBuildStats,
}

pub fn build_inverse_lut_payload(
    recipe: &ConversionRecipe,
    model: &ValidatedLocalForwardModel,
    build_policy: InverseLutBuildPolicy,
) -> Result<BuiltInverseLutPayload, InverseLutBuildError> {
    let identity = InverseLutIdentityRecord::from_local_model(recipe, model, build_policy)
        .map_err(InverseLutBuildError::Identity)?;
    let solver = recipe
        .custom_optimizer_solver
        .ok_or(InverseLutBuildError::MissingSolverConfig)?;
    solver
        .validate(recipe.target.channels.len())
        .map_err(InverseLutBuildError::MissingObjectiveWeightProvenance)?;
    let objective = solver.objective_weights.ok_or_else(|| {
        InverseLutBuildError::MissingObjectiveWeightProvenance(vec![
            "Custom Optimizer objective-weight provenance is missing; recapture the recipe before inverse-LUT construction."
                .to_owned(),
        ])
    })?;
    objective
        .validate()
        .map_err(InverseLutBuildError::MissingObjectiveWeightProvenance)?;
    let weights = runtime_weights(objective);

    let parts = match build_policy.continuity_field {
        InverseLutContinuityFieldMethod::IndependentNodeSolvesV1 => {
            build_independent_payload(recipe, model, build_policy, solver, weights)?
        }
        InverseLutContinuityFieldMethod::JacobiSixNeighborV1 { .. } => {
            build_continuity_payload(recipe, model, build_policy, solver, weights)?
        }
    };
    let built = BuiltInverseLutPayload {
        identity,
        validity: parts.validity,
        coverages: parts.coverages,
        stats: parts.stats,
    };
    validate_built_payload(&built)?;
    Ok(built)
}

pub fn publish_built_inverse_lut_if_absent(
    destination: &Path,
    built: &BuiltInverseLutPayload,
) -> Result<InverseLutPublishOutcome, String> {
    validate_built_payload(built).map_err(|error| format!("{error:?}"))?;
    publish_inverse_lut_artifact_if_absent(
        destination,
        &built.identity,
        &built.validity,
        &built.coverages,
    )
}

fn build_independent_payload(
    recipe: &ConversionRecipe,
    model: &ValidatedLocalForwardModel,
    build_policy: InverseLutBuildPolicy,
    solver: crate::custom_optimizer_config::CustomOptimizerSolverConfig,
    weights: CandidateScoringWeights,
) -> Result<BuiltPayloadParts, InverseLutBuildError> {
    let (_shape, labs) = lab_grid_points(build_policy.grid).map_err(InverseLutBuildError::Grid)?;
    let channel_count = recipe.target.channels.len();
    let node_count =
        u64::try_from(labs.len()).map_err(|_| InverseLutBuildError::CounterOverflow)?;
    let coverage_values = labs
        .len()
        .checked_mul(channel_count)
        .ok_or(InverseLutBuildError::CounterOverflow)?;
    let mut validity = Vec::with_capacity(labs.len());
    let mut coverages = Vec::with_capacity(coverage_values);
    let mut stats = InverseLutBuildStats {
        node_count,
        ..InverseLutBuildStats::default()
    };

    for (index, lab) in labs.iter().copied().enumerate() {
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
                    return Err(InverseLutBuildError::PayloadTopology(format!(
                        "Inverse LUT node {index} solver topology mismatch: expected {channel_count}, got {}.",
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
            Err(error) => return Err(InverseLutBuildError::SolveFailed { index, error }),
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
    model: &ValidatedLocalForwardModel,
    build_policy: InverseLutBuildPolicy,
    solver: crate::custom_optimizer_config::CustomOptimizerSolverConfig,
    weights: CandidateScoringWeights,
) -> Result<BuiltPayloadParts, InverseLutBuildError> {
    let field = build_positive_v2_jacobi_field(
        &recipe.target,
        &recipe.strategy,
        weights,
        model,
        build_policy.grid,
        solver,
        build_policy.continuity_field,
    )
    .map_err(InverseLutBuildError::ContinuityField)?;
    flatten_continuity_field(recipe.target.channels.len(), field)
}

fn flatten_continuity_field(
    channel_count: usize,
    built: BuiltJacobiContinuityField,
) -> Result<BuiltPayloadParts, InverseLutBuildError> {
    let node_count = built.field.nodes.len();
    let mut validity = Vec::with_capacity(node_count);
    let mut coverages = Vec::with_capacity(
        node_count
            .checked_mul(channel_count)
            .ok_or(InverseLutBuildError::CounterOverflow)?,
    );
    for (index, node) in built.field.nodes.into_iter().enumerate() {
        if node.coverages.len() != channel_count {
            return Err(InverseLutBuildError::PayloadTopology(format!(
                "Continuity-field node {index} topology mismatch: expected {channel_count}, got {}.",
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
        .map_err(|_| InverseLutBuildError::CounterOverflow)?;
    let node_count_u64 =
        u64::try_from(node_count).map_err(|_| InverseLutBuildError::CounterOverflow)?;
    let unsupported_nodes = node_count_u64
        .checked_sub(supported_nodes)
        .ok_or(InverseLutBuildError::CounterOverflow)?;
    Ok(BuiltPayloadParts {
        validity,
        coverages,
        stats: InverseLutBuildStats {
            node_count: node_count_u64,
            supported_nodes,
            unsupported_nodes,
            continuity_seed_attempts: built.stats.seed_attempts,
            continuity_solves: built.stats.continuity_solves,
            ..InverseLutBuildStats::default()
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
    destination: &mut InverseLutBuildStats,
    source: InverseSolverStats,
) -> Result<(), InverseLutBuildError> {
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

fn checked_add(left: u64, right: u64) -> Result<u64, InverseLutBuildError> {
    left.checked_add(right)
        .ok_or(InverseLutBuildError::CounterOverflow)
}

fn validate_built_payload(built: &BuiltInverseLutPayload) -> Result<(), InverseLutBuildError> {
    built
        .identity
        .validate()
        .map_err(|errors| InverseLutBuildError::PayloadTopology(errors.join("\n")))?;
    let expected_nodes = built
        .identity
        .build_policy
        .grid
        .node_count()
        .ok_or(InverseLutBuildError::CounterOverflow)?;
    if built.validity.len() as u64 != expected_nodes {
        return Err(InverseLutBuildError::PayloadTopology(format!(
            "Inverse LUT validity count mismatch: expected {expected_nodes}, got {}.",
            built.validity.len()
        )));
    }
    let channels = built.identity.channel_names.len();
    let expected_values = usize::try_from(expected_nodes)
        .ok()
        .and_then(|nodes| nodes.checked_mul(channels))
        .ok_or(InverseLutBuildError::CounterOverflow)?;
    if built.coverages.len() != expected_values {
        return Err(InverseLutBuildError::PayloadTopology(format!(
            "Inverse LUT coverage count mismatch: expected {expected_values}, got {}.",
            built.coverages.len()
        )));
    }
    for (node_index, valid) in built.validity.iter().copied().enumerate() {
        let start = node_index * channels;
        for (channel_index, value) in built.coverages[start..start + channels]
            .iter()
            .copied()
            .enumerate()
        {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(InverseLutBuildError::PayloadTopology(format!(
                    "Inverse LUT node {node_index} channel {channel_index} is outside normalized finite coverage."
                )));
            }
            if !valid && value.to_bits() != 0 {
                return Err(InverseLutBuildError::PayloadTopology(format!(
                    "Inverse LUT invalid node {node_index} channel {channel_index} is not canonical positive zero."
                )));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct InverseLutRuntime {
    artifact: VerifiedInverseLutArtifact,
    l_samples: usize,
    a_samples: usize,
    b_samples: usize,
    channel_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InverseLutLookupError {
    InvalidArtifact(String),
    NonFiniteLab,
    OutOfDomain {
        axis: &'static str,
        value: f64,
        minimum: f64,
        maximum: f64,
    },
    UnsupportedCorner {
        node_index: usize,
    },
    InvalidInterpolatedCoverage {
        channel_index: usize,
        value: f32,
    },
    Quantization(String),
}

impl InverseLutRuntime {
    pub fn from_verified(
        artifact: VerifiedInverseLutArtifact,
    ) -> Result<Self, InverseLutLookupError> {
        artifact
            .identity
            .validate()
            .map_err(|errors| InverseLutLookupError::InvalidArtifact(errors.join("\n")))?;
        let expected_content_id = artifact
            .identity
            .content_id()
            .map_err(InverseLutLookupError::InvalidArtifact)?;
        if artifact.identity_content_id != expected_content_id {
            return Err(InverseLutLookupError::InvalidArtifact(format!(
                "Inverse LUT identity content-id mismatch: expected {expected_content_id}, got {}.",
                artifact.identity_content_id
            )));
        }
        if artifact.identity.build_policy.interpolation
            != InverseLutInterpolationMethod::TrilinearV1
        {
            return Err(InverseLutLookupError::InvalidArtifact(
                "Unsupported inverse LUT runtime interpolation method.".to_owned(),
            ));
        }

        let grid = artifact.identity.build_policy.grid;
        let l_samples = usize::from(grid.l_samples);
        let a_samples = usize::from(grid.a_samples);
        let b_samples = usize::from(grid.b_samples);
        let node_count = l_samples
            .checked_mul(a_samples)
            .and_then(|value| value.checked_mul(b_samples))
            .ok_or_else(|| {
                InverseLutLookupError::InvalidArtifact(
                    "Inverse LUT runtime node count overflowed usize.".to_owned(),
                )
            })?;
        let channel_count = artifact.identity.channel_names.len();
        let coverage_count = node_count.checked_mul(channel_count).ok_or_else(|| {
            InverseLutLookupError::InvalidArtifact(
                "Inverse LUT runtime coverage count overflowed usize.".to_owned(),
            )
        })?;
        if artifact.validity.len() != node_count || artifact.coverages.len() != coverage_count {
            return Err(InverseLutLookupError::InvalidArtifact(
                "Inverse LUT runtime payload dimensions do not match identity.".to_owned(),
            ));
        }

        let actual_payload_sha256 =
            validate_and_hash_payload(&artifact.validity, &artifact.coverages, channel_count)?;
        if artifact.payload_sha256 != actual_payload_sha256 {
            return Err(InverseLutLookupError::InvalidArtifact(format!(
                "Inverse LUT payload SHA-256 mismatch: expected {}, got {actual_payload_sha256}.",
                artifact.payload_sha256
            )));
        }

        Ok(Self {
            artifact,
            l_samples,
            a_samples,
            b_samples,
            channel_count,
        })
    }

    pub fn identity(&self) -> &InverseLutIdentityRecord {
        &self.artifact.identity
    }

    pub fn identity_content_id(&self) -> &str {
        &self.artifact.identity_content_id
    }

    pub fn lookup(&self, lab: LabColor) -> Result<Vec<f32>, InverseLutLookupError> {
        let mut output = vec![0.0f32; self.channel_count];
        self.lookup_into(lab, &mut output)?;
        Ok(output)
    }

    /// Allocation-free hot-path lookup for the production raster worker.
    pub fn lookup_into(
        &self,
        lab: LabColor,
        output: &mut [f32],
    ) -> Result<(), InverseLutLookupError> {
        if output.len() != self.channel_count {
            return Err(InverseLutLookupError::InvalidArtifact(format!(
                "Inverse LUT lookup output topology mismatch: expected {}, got {}.",
                self.channel_count,
                output.len()
            )));
        }
        if !lab.l.is_finite() || !lab.a.is_finite() || !lab.b.is_finite() {
            return Err(InverseLutLookupError::NonFiniteLab);
        }
        let grid = self.artifact.identity.build_policy.grid;
        let l = axis_bracket(lab.l, grid.l_min, grid.l_max, self.l_samples, "L*")?;
        let a = axis_bracket(lab.a, grid.a_min, grid.a_max, self.a_samples, "a*")?;
        let b = axis_bracket(lab.b, grid.b_min, grid.b_max, self.b_samples, "b*")?;

        let l_terms = l.terms();
        let a_terms = a.terms();
        let b_terms = b.terms();
        let mut accum = [0.0f64; crate::custom_optimizer_config::CUSTOM_OPTIMIZER_MAX_CHANNELS];
        for &(li, lw) in &l_terms {
            if lw == 0.0 {
                continue;
            }
            for &(ai, aw) in &a_terms {
                if aw == 0.0 {
                    continue;
                }
                for &(bi, bw) in &b_terms {
                    if bw == 0.0 {
                        continue;
                    }
                    let weight = lw * aw * bw;
                    if weight == 0.0 {
                        continue;
                    }
                    let node_index = (li * self.a_samples + ai) * self.b_samples + bi;
                    if !self.artifact.validity[node_index] {
                        return Err(InverseLutLookupError::UnsupportedCorner { node_index });
                    }
                    let start = node_index * self.channel_count;
                    for (channel_index, value) in self.artifact.coverages
                        [start..start + self.channel_count]
                        .iter()
                        .copied()
                        .enumerate()
                    {
                        accum[channel_index] += f64::from(value) * weight;
                    }
                }
            }
        }

        for (channel_index, destination) in output.iter_mut().enumerate() {
            let value = accum[channel_index] as f32;
            if !value.is_finite() || value < -1.0e-6 || value > 1.0 + 1.0e-6 {
                return Err(InverseLutLookupError::InvalidInterpolatedCoverage {
                    channel_index,
                    value,
                });
            }
            *destination = value.clamp(0.0, 1.0);
        }
        Ok(())
    }

    pub fn lookup_quantized(&self, lab: LabColor) -> Result<Vec<u16>, InverseLutLookupError> {
        let normalized = self.lookup(lab)?;
        normalized
            .into_iter()
            .map(|value| {
                quantize_normalized_coverage(
                    value,
                    self.artifact.identity.target_bit_depth,
                    self.artifact.identity.build_policy.output_quantization,
                )
                .map_err(InverseLutLookupError::Quantization)
            })
            .collect()
    }
}

fn validate_and_hash_payload(
    validity: &[bool],
    coverages: &[f32],
    channel_count: usize,
) -> Result<String, InverseLutLookupError> {
    let mut hasher = Sha256::new();
    for valid in validity {
        hasher.update([u8::from(*valid)]);
    }
    for (index, value) in coverages.iter().copied().enumerate() {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(InverseLutLookupError::InvalidArtifact(format!(
                "Inverse LUT coverage {index} is not finite normalized data."
            )));
        }
        if value == 0.0 && value.to_bits() != 0 {
            return Err(InverseLutLookupError::InvalidArtifact(format!(
                "Inverse LUT coverage {index} stores non-canonical negative zero."
            )));
        }
        let node_index = index / channel_count;
        if !validity[node_index] && value.to_bits() != 0 {
            return Err(InverseLutLookupError::InvalidArtifact(format!(
                "Inverse LUT invalid node {node_index} stores non-zero coverage."
            )));
        }
        hasher.update(value.to_bits().to_le_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Clone, Copy, Debug)]
struct AxisBracket {
    low: usize,
    high: usize,
    fraction: f64,
}

impl AxisBracket {
    fn terms(self) -> [(usize, f64); 2] {
        if self.low == self.high {
            [(self.low, 1.0), (self.low, 0.0)]
        } else {
            [(self.low, 1.0 - self.fraction), (self.high, self.fraction)]
        }
    }
}

fn axis_bracket(
    value: f64,
    minimum: f64,
    maximum: f64,
    samples: usize,
    axis: &'static str,
) -> Result<AxisBracket, InverseLutLookupError> {
    if value < minimum || value > maximum {
        return Err(InverseLutLookupError::OutOfDomain {
            axis,
            value,
            minimum,
            maximum,
        });
    }
    if value == minimum {
        return Ok(AxisBracket {
            low: 0,
            high: 0,
            fraction: 0.0,
        });
    }
    if value == maximum {
        let last = samples - 1;
        return Ok(AxisBracket {
            low: last,
            high: last,
            fraction: 0.0,
        });
    }

    let position = (value - minimum) / (maximum - minimum) * (samples - 1) as f64;
    let nearest = position.round();
    if (position - nearest).abs() <= GRID_NODE_SNAP_EPSILON {
        let index = nearest.clamp(0.0, (samples - 1) as f64) as usize;
        return Ok(AxisBracket {
            low: index,
            high: index,
            fraction: 0.0,
        });
    }
    let low = position.floor() as usize;
    let high = low + 1;
    Ok(AxisBracket {
        low,
        high,
        fraction: position - low as f64,
    })
}
