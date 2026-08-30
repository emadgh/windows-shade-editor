use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_recipe::recipe_sha256;
use crate::device_characterization::LabColor;
use crate::inverse_lut_identity::{
    InverseLutBuildPolicy, InverseLutInterpolationMethod, quantize_normalized_coverage,
};
use crate::profile_backed_inverse_lut_builder::{
    BuiltProfileBackedInverseLutPayload, ProfileBackedForwardModelMethod,
};
use crate::safe_fs;

pub const PROFILE_BACKED_INVERSE_LUT_ARTIFACT_SCHEMA_VERSION: u32 = 1;
pub const MAX_PROFILE_BACKED_INVERSE_LUT_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
const OUTPUT_ICC_MODEL_PREFIX: &str = "output-icc-sha256:";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileBackedInverseLutForwardModelMethod {
    OutputIccDeviceToPcsV1,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProfileBackedInverseLutIdentity {
    pub schema_version: u32,
    pub forward_model_method: ProfileBackedInverseLutForwardModelMethod,
    pub output_profile_sha256: String,
    pub forward_model_id: String,
    pub recipe_sha256: String,
    pub channel_names: Vec<String>,
    pub target_bit_depth: u8,
    pub build_policy: InverseLutBuildPolicy,
}

impl ProfileBackedInverseLutIdentity {
    pub fn from_built(
        recipe: &ConversionRecipe,
        built: &BuiltProfileBackedInverseLutPayload,
    ) -> Result<Self, Vec<String>> {
        let mut errors = Vec::new();
        if let Err(mut recipe_errors) = recipe.validate() {
            errors.append(&mut recipe_errors);
        }
        if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
            errors.push("Profile-backed inverse LUT requires a Custom Optimizer recipe.".to_owned());
        }
        if recipe
            .target
            .characterization_id
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        {
            errors.push(
                "Profile-backed inverse LUT cannot replace a measured-characterization authority."
                    .to_owned(),
            );
        }
        let output_identity = match recipe.target.output_profile_identity.as_ref() {
            Some(identity) if is_bare_sha256(identity.sha256.trim()) => Some(identity),
            _ => {
                errors.push(
                    "Profile-backed inverse LUT requires a canonical Output ICC SHA-256 identity."
                        .to_owned(),
                );
                None
            }
        };
        if recipe
            .target
            .output_profile_path
            .as_deref()
            .is_none_or(|path| path.trim().is_empty())
        {
            errors.push("Profile-backed inverse LUT requires an exact Output ICC path.".to_owned());
        }
        if built.forward_model_method != ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1 {
            errors.push("Unsupported profile-backed forward-model method.".to_owned());
        }
        let recipe_channels = recipe
            .target
            .channels
            .iter()
            .map(|channel| channel.name.clone())
            .collect::<Vec<_>>();
        if recipe_channels != built.channel_names {
            errors.push("Profile-backed inverse LUT channel order does not match the recipe.".to_owned());
        }
        if recipe.target.bit_depth != built.target_bit_depth {
            errors.push("Profile-backed inverse LUT bit depth does not match the recipe.".to_owned());
        }
        if let Err(mut policy_errors) = built.build_policy.validate() {
            errors.append(&mut policy_errors);
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        let output_profile_sha256 = output_identity.unwrap().sha256.trim().to_owned();
        let expected_model_id = format!("{OUTPUT_ICC_MODEL_PREFIX}{output_profile_sha256}");
        if built.forward_model_id != expected_model_id {
            return Err(vec![format!(
                "Profile-backed inverse LUT model identity mismatch: expected {expected_model_id}, got {}.",
                built.forward_model_id
            )]);
        }
        let recipe_sha256 = recipe_sha256(recipe).map_err(|error| vec![error])?;
        let identity = Self {
            schema_version: PROFILE_BACKED_INVERSE_LUT_ARTIFACT_SCHEMA_VERSION,
            forward_model_method: ProfileBackedInverseLutForwardModelMethod::OutputIccDeviceToPcsV1,
            output_profile_sha256,
            forward_model_id: built.forward_model_id.clone(),
            recipe_sha256,
            channel_names: built.channel_names.clone(),
            target_bit_depth: built.target_bit_depth,
            build_policy: built.build_policy,
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if self.schema_version != PROFILE_BACKED_INVERSE_LUT_ARTIFACT_SCHEMA_VERSION {
            errors.push(format!(
                "Unsupported profile-backed inverse LUT identity schema {}.",
                self.schema_version
            ));
        }
        if !is_bare_sha256(&self.output_profile_sha256) {
            errors.push(
                "Profile-backed inverse LUT Output ICC identity must be canonical lowercase SHA-256."
                    .to_owned(),
            );
        }
        if !is_bare_sha256(&self.recipe_sha256) {
            errors.push(
                "Profile-backed inverse LUT recipe identity must be canonical lowercase SHA-256."
                    .to_owned(),
            );
        }
        if self.forward_model_id
            != format!("{OUTPUT_ICC_MODEL_PREFIX}{}", self.output_profile_sha256)
        {
            errors.push(
                "Profile-backed inverse LUT forward-model ID is not derived from its exact Output ICC SHA-256."
                    .to_owned(),
            );
        }
        validate_channel_names(&self.channel_names, &mut errors);
        if !matches!(self.target_bit_depth, 8 | 16) {
            errors.push(format!(
                "Profile-backed inverse LUT target bit depth must be 8 or 16, got {}.",
                self.target_bit_depth
            ));
        }
        if let Err(mut policy_errors) = self.build_policy.validate() {
            errors.append(&mut policy_errors);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn content_id(&self) -> Result<String, String> {
        self.validate().map_err(|errors| errors.join("\n"))?;
        let bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProfileBackedInverseLutArtifact {
    pub identity: ProfileBackedInverseLutIdentity,
    pub identity_content_id: String,
    pub payload_sha256: String,
    pub validity: Vec<bool>,
    pub coverages: Vec<f32>,
}

impl ProfileBackedInverseLutArtifact {
    pub fn from_built(
        recipe: &ConversionRecipe,
        built: &BuiltProfileBackedInverseLutPayload,
    ) -> Result<Self, Vec<String>> {
        let identity = ProfileBackedInverseLutIdentity::from_built(recipe, built)?;
        let identity_content_id = identity.content_id().map_err(|error| vec![error])?;
        validate_payload(&identity, &built.validity, &built.coverages)?;
        let payload_sha256 = payload_sha256(&built.validity, &built.coverages)
            .map_err(|error| vec![error])?;
        let artifact = Self {
            identity,
            identity_content_id,
            payload_sha256,
            validity: built.validity.clone(),
            coverages: built.coverages.clone(),
        };
        artifact.validate()?;
        Ok(artifact)
    }

    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if let Err(mut identity_errors) = self.identity.validate() {
            errors.append(&mut identity_errors);
        }
        match self.identity.content_id() {
            Ok(actual) if actual != self.identity_content_id => errors.push(format!(
                "Profile-backed inverse LUT identity content ID mismatch: recorded {}, actual {actual}.",
                self.identity_content_id
            )),
            Err(error) => errors.push(error),
            _ => {}
        }
        if let Err(mut payload_errors) =
            validate_payload(&self.identity, &self.validity, &self.coverages)
        {
            errors.append(&mut payload_errors);
        }
        match payload_sha256(&self.validity, &self.coverages) {
            Ok(actual) if actual != self.payload_sha256 => errors.push(format!(
                "Profile-backed inverse LUT payload SHA-256 mismatch: recorded {}, actual {actual}.",
                self.payload_sha256
            )),
            Err(error) => errors.push(error),
            _ => {}
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

pub fn save_profile_backed_inverse_lut_artifact(
    path: &Path,
    artifact: &ProfileBackedInverseLutArtifact,
) -> Result<(), String> {
    artifact.validate().map_err(|errors| errors.join("\n"))?;
    let bytes = serde_json::to_vec(artifact).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_PROFILE_BACKED_INVERSE_LUT_ARTIFACT_BYTES {
        return Err(format!(
            "Profile-backed inverse LUT artifact is {} bytes; maximum is {} bytes.",
            bytes.len(),
            MAX_PROFILE_BACKED_INVERSE_LUT_ARTIFACT_BYTES
        ));
    }
    safe_fs::atomic_write(path, &bytes, None)
}

pub fn load_profile_backed_inverse_lut_artifact(
    path: &Path,
) -> Result<ProfileBackedInverseLutArtifact, String> {
    let metadata = fs::metadata(path).map_err(|error| {
        format!(
            "Cannot inspect profile-backed inverse LUT artifact {}: {error}",
            path.display()
        )
    })?;
    if metadata.len() == 0 || metadata.len() > MAX_PROFILE_BACKED_INVERSE_LUT_ARTIFACT_BYTES {
        return Err(format!(
            "Profile-backed inverse LUT artifact {} has invalid bounded size {} bytes.",
            path.display(),
            metadata.len()
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        format!(
            "Cannot read profile-backed inverse LUT artifact {}: {error}",
            path.display()
        )
    })?;
    let artifact: ProfileBackedInverseLutArtifact =
        serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "Cannot parse profile-backed inverse LUT artifact {}: {error}",
                path.display()
            )
        })?;
    artifact.validate().map_err(|errors| errors.join("\n"))?;
    Ok(artifact)
}

#[derive(Clone, Debug)]
pub struct ProfileBackedInverseLutRuntime {
    artifact: ProfileBackedInverseLutArtifact,
    l_samples: usize,
    a_samples: usize,
    b_samples: usize,
    channel_count: usize,
}

impl ProfileBackedInverseLutRuntime {
    pub fn new(artifact: ProfileBackedInverseLutArtifact) -> Result<Self, String> {
        artifact.validate().map_err(|errors| errors.join("\n"))?;
        if artifact.identity.build_policy.interpolation != InverseLutInterpolationMethod::TrilinearV1 {
            return Err("Unsupported profile-backed inverse LUT interpolation method.".to_owned());
        }
        let grid = artifact.identity.build_policy.grid;
        Ok(Self {
            l_samples: usize::from(grid.l_samples),
            a_samples: usize::from(grid.a_samples),
            b_samples: usize::from(grid.b_samples),
            channel_count: artifact.identity.channel_names.len(),
            artifact,
        })
    }

    pub fn identity(&self) -> &ProfileBackedInverseLutIdentity {
        &self.artifact.identity
    }

    pub fn identity_content_id(&self) -> &str {
        &self.artifact.identity_content_id
    }

    pub fn payload_sha256(&self) -> &str {
        &self.artifact.payload_sha256
    }

    pub fn output_channels(&self) -> usize {
        self.channel_count
    }

    pub fn target_bit_depth(&self) -> u8 {
        self.artifact.identity.target_bit_depth
    }

    pub fn lookup_into(&self, lab: LabColor, output: &mut [f32]) -> Result<(), String> {
        if output.len() != self.channel_count {
            return Err(format!(
                "Profile-backed inverse LUT lookup requires {} output channels; got {}.",
                self.channel_count,
                output.len()
            ));
        }
        if !lab.l.is_finite() || !lab.a.is_finite() || !lab.b.is_finite() {
            return Err("Profile-backed inverse LUT lookup requires finite Lab.".to_owned());
        }
        let grid = self.artifact.identity.build_policy.grid;
        let l = axis_position(lab.l, grid.l_min, grid.l_max, self.l_samples, "L*")?;
        let a = axis_position(lab.a, grid.a_min, grid.a_max, self.a_samples, "a*")?;
        let b = axis_position(lab.b, grid.b_min, grid.b_max, self.b_samples, "b*")?;
        output.fill(0.0);

        let axes = [l, a, b];
        for l_side in 0..2 {
            let (li, lw) = side(axes[0], l_side);
            if lw == 0.0 {
                continue;
            }
            for a_side in 0..2 {
                let (ai, aw) = side(axes[1], a_side);
                if aw == 0.0 {
                    continue;
                }
                for b_side in 0..2 {
                    let (bi, bw) = side(axes[2], b_side);
                    let weight = lw * aw * bw;
                    if weight == 0.0 {
                        continue;
                    }
                    let node = self.node_index(li, ai, bi)?;
                    if !self.artifact.validity[node] {
                        return Err(format!(
                            "Profile-backed inverse LUT lookup requires unsupported node {node}."
                        ));
                    }
                    let start = node
                        .checked_mul(self.channel_count)
                        .ok_or_else(|| "Profile-backed LUT coverage offset overflowed.".to_owned())?;
                    for channel in 0..self.channel_count {
                        output[channel] += self.artifact.coverages[start + channel] * weight as f32;
                    }
                }
            }
        }
        if output
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err("Profile-backed inverse LUT interpolation produced invalid coverage.".to_owned());
        }
        Ok(())
    }

    pub fn lookup_quantized_into(
        &self,
        lab: LabColor,
        output: &mut [u16],
    ) -> Result<(), String> {
        if output.len() != self.channel_count {
            return Err(format!(
                "Profile-backed inverse LUT quantized lookup requires {} output channels; got {}.",
                self.channel_count,
                output.len()
            ));
        }
        let mut normalized = vec![0.0f32; self.channel_count];
        self.lookup_into(lab, &mut normalized)?;
        for (destination, value) in output.iter_mut().zip(normalized) {
            *destination = quantize_normalized_coverage(
                value,
                self.artifact.identity.target_bit_depth,
                self.artifact.identity.build_policy.output_quantization,
            )?;
        }
        Ok(())
    }

    fn node_index(&self, l: usize, a: usize, b: usize) -> Result<usize, String> {
        l.checked_mul(self.a_samples)
            .and_then(|value| value.checked_add(a))
            .and_then(|value| value.checked_mul(self.b_samples))
            .and_then(|value| value.checked_add(b))
            .ok_or_else(|| "Profile-backed inverse LUT node index overflowed.".to_owned())
    }
}

#[derive(Clone, Copy, Debug)]
struct AxisPosition {
    lower: usize,
    upper: usize,
    fraction: f64,
}

fn axis_position(
    value: f64,
    minimum: f64,
    maximum: f64,
    samples: usize,
    axis: &'static str,
) -> Result<AxisPosition, String> {
    if value < minimum || value > maximum {
        return Err(format!(
            "Profile-backed inverse LUT {axis} value {value} is outside [{minimum}, {maximum}]."
        ));
    }
    let scaled = (value - minimum) / (maximum - minimum) * (samples - 1) as f64;
    let lower = scaled.floor() as usize;
    let upper = lower.saturating_add(1).min(samples - 1);
    Ok(AxisPosition {
        lower,
        upper,
        fraction: if upper == lower {
            0.0
        } else {
            scaled - lower as f64
        },
    })
}

fn side(position: AxisPosition, upper: usize) -> (usize, f64) {
    if upper == 0 {
        (position.lower, 1.0 - position.fraction)
    } else {
        (position.upper, position.fraction)
    }
}

fn validate_payload(
    identity: &ProfileBackedInverseLutIdentity,
    validity: &[bool],
    coverages: &[f32],
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let Some(nodes) = identity.build_policy.grid.node_count() else {
        return Err(vec!["Profile-backed inverse LUT node count overflowed.".to_owned()]);
    };
    if validity.len() as u64 != nodes {
        errors.push(format!(
            "Profile-backed inverse LUT validity count mismatch: expected {nodes}, got {}.",
            validity.len()
        ));
    }
    let expected_coverages = usize::try_from(nodes)
        .ok()
        .and_then(|count| count.checked_mul(identity.channel_names.len()));
    if expected_coverages != Some(coverages.len()) {
        errors.push(format!(
            "Profile-backed inverse LUT coverage count mismatch: expected {:?}, got {}.",
            expected_coverages,
            coverages.len()
        ));
    }
    if errors.is_empty() {
        for (index, value) in coverages.iter().copied().enumerate() {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) || value.to_bits() == (-0.0f32).to_bits() {
                errors.push(format!(
                    "Profile-backed inverse LUT coverage {index} is not canonical finite normalized data."
                ));
                break;
            }
            let node = index / identity.channel_names.len();
            if !validity[node] && value.to_bits() != 0 {
                errors.push(format!(
                    "Profile-backed inverse LUT invalid node {node} contains non-zero coverage."
                ));
                break;
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn payload_sha256(validity: &[bool], coverages: &[f32]) -> Result<String, String> {
    let mut hasher = Sha256::new();
    for valid in validity {
        hasher.update([u8::from(*valid)]);
    }
    for (index, value) in coverages.iter().copied().enumerate() {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) || value.to_bits() == (-0.0f32).to_bits() {
            return Err(format!(
                "Cannot hash non-canonical profile-backed LUT coverage {index}."
            ));
        }
        hasher.update(value.to_bits().to_le_bytes());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_channel_names(names: &[String], errors: &mut Vec<String>) {
    if !(4..=12).contains(&names.len()) {
        errors.push(format!(
            "Profile-backed inverse LUT topology must contain 4..=12 channels, got {}.",
            names.len()
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            errors.push("Profile-backed inverse LUT channel names cannot be empty.".to_owned());
        } else if !seen.insert(trimmed.to_ascii_lowercase()) {
            errors.push(format!(
                "Duplicate profile-backed inverse LUT channel '{trimmed}'."
            ));
        }
    }
}

fn is_bare_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
    use crate::inverse_lut_identity::{
        INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION, InverseLutContinuityFieldMethod,
        InverseLutNumericalPrecision, InverseLutOutputQuantization, InverseLutValidityEncoding,
        LabGridSpec,
    };
    use crate::model::IccProfileIdentity;
    use crate::profile_backed_inverse_lut_builder::{
        ProfileBackedInverseLutBuildStats, ProfileBackedForwardModelMethod,
    };

    fn recipe() -> ConversionRecipe {
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: "0".repeat(64),
            },
            source_transparency_policy: None,
            target: ConversionTargetDefinition {
                name: "Profile target".to_owned(),
                channels: ["Blue", "Brown", "Beige", "Black"]
                    .into_iter()
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: Some(1.0),
                    })
                    .collect(),
                bit_depth: 16,
                output_profile_identity: Some(IccProfileIdentity {
                    description: "Output".to_owned(),
                    sha256: "a".repeat(64),
                }),
                output_profile_path: Some("C:\\Color\\Output.icc".to_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: Some(2.0),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
        }
    }

    fn policy() -> InverseLutBuildPolicy {
        InverseLutBuildPolicy {
            schema_version: INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION,
            grid: LabGridSpec {
                l_min: 0.0,
                l_max: 100.0,
                l_samples: 2,
                a_min: -10.0,
                a_max: 10.0,
                a_samples: 2,
                b_min: -10.0,
                b_max: 10.0,
                b_samples: 2,
            },
            interpolation: InverseLutInterpolationMethod::TrilinearV1,
            validity_encoding: InverseLutValidityEncoding::ExplicitNodeValidityMaskV1,
            numerical_precision: InverseLutNumericalPrecision::NormalizedF32V1,
            output_quantization: InverseLutOutputQuantization::ClampScaleRoundV1,
            continuity_field: InverseLutContinuityFieldMethod::IndependentNodeSolvesV1,
        }
    }

    fn built() -> BuiltProfileBackedInverseLutPayload {
        let pattern = [0.1f32, 0.2, 0.3, 0.4];
        let mut coverages = Vec::new();
        for _ in 0..8 {
            coverages.extend_from_slice(&pattern);
        }
        BuiltProfileBackedInverseLutPayload {
            forward_model_method: ProfileBackedForwardModelMethod::OutputIccDeviceToPcsV1,
            forward_model_id: format!("{OUTPUT_ICC_MODEL_PREFIX}{}", "a".repeat(64)),
            channel_names: ["Blue", "Brown", "Beige", "Black"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            target_bit_depth: 16,
            build_policy: policy(),
            validity: vec![true; 8],
            coverages,
            stats: ProfileBackedInverseLutBuildStats {
                node_count: 8,
                supported_nodes: 8,
                ..ProfileBackedInverseLutBuildStats::default()
            },
        }
    }

    #[test]
    fn identity_is_explicitly_output_icc_and_content_addressed() {
        let identity = ProfileBackedInverseLutIdentity::from_built(&recipe(), &built()).unwrap();
        assert_eq!(
            identity.forward_model_method,
            ProfileBackedInverseLutForwardModelMethod::OutputIccDeviceToPcsV1
        );
        assert_eq!(identity.output_profile_sha256, "a".repeat(64));
        assert_eq!(identity.content_id().unwrap(), identity.clone().content_id().unwrap());
    }

    #[test]
    fn artifact_round_trips_and_tampering_fails_closed() {
        let artifact = ProfileBackedInverseLutArtifact::from_built(&recipe(), &built()).unwrap();
        let path = std::env::temp_dir().join(format!(
            "shade-profile-lut-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        save_profile_backed_inverse_lut_artifact(&path, &artifact).unwrap();
        let loaded = load_profile_backed_inverse_lut_artifact(&path).unwrap();
        assert_eq!(loaded, artifact);
        let mut tampered = loaded;
        tampered.coverages[0] = 0.9;
        assert!(tampered.validate().is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn exact_grid_nodes_and_center_use_existing_trilinear_semantics() {
        let artifact = ProfileBackedInverseLutArtifact::from_built(&recipe(), &built()).unwrap();
        let runtime = ProfileBackedInverseLutRuntime::new(artifact).unwrap();
        let mut exact = [0.0f32; 4];
        runtime
            .lookup_into(
                LabColor {
                    l: 0.0,
                    a: -10.0,
                    b: -10.0,
                },
                &mut exact,
            )
            .unwrap();
        assert_eq!(exact, [0.1, 0.2, 0.3, 0.4]);
        let mut center = [0.0f32; 4];
        runtime
            .lookup_into(
                LabColor {
                    l: 50.0,
                    a: 0.0,
                    b: 0.0,
                },
                &mut center,
            )
            .unwrap();
        for (actual, expected) in center.into_iter().zip([0.1, 0.2, 0.3, 0.4]) {
            assert!((actual - expected).abs() < 1.0e-6);
        }
    }

    #[test]
    fn unsupported_corner_and_wrong_profile_identity_fail_closed() {
        let mut artifact = ProfileBackedInverseLutArtifact::from_built(&recipe(), &built()).unwrap();
        artifact.validity[7] = false;
        let start = 7 * 4;
        artifact.coverages[start..start + 4].fill(0.0);
        artifact.payload_sha256 = payload_sha256(&artifact.validity, &artifact.coverages).unwrap();
        let runtime = ProfileBackedInverseLutRuntime::new(artifact).unwrap();
        let mut output = [0.0f32; 4];
        assert!(runtime
            .lookup_into(
                LabColor {
                    l: 50.0,
                    a: 0.0,
                    b: 0.0,
                },
                &mut output,
            )
            .is_err());

        let mut wrong = built();
        wrong.forward_model_id = format!("{OUTPUT_ICC_MODEL_PREFIX}{}", "b".repeat(64));
        assert!(ProfileBackedInverseLutArtifact::from_built(&recipe(), &wrong).is_err());
    }

    #[test]
    fn measured_authority_cannot_be_reinterpreted_as_profile_artifact() {
        let mut measured = recipe();
        measured.target.characterization_id = Some(format!("sha256:{}", "c".repeat(64)));
        assert!(ProfileBackedInverseLutArtifact::from_built(&measured, &built()).is_err());
    }
}
