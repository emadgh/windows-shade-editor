use std::fs;
use std::path::{Path, PathBuf};

use crate::color_conversion::{ConversionEngineMode, ConversionRecipe};
use crate::conversion_candidate_preview::{CandidatePreviewInput, CandidatePreviewResult};
use crate::conversion_recipe::recipe_sha256;
use crate::conversion_transaction::ConversionCancellation;
use crate::inverse_lut_identity::{
    INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION, InverseLutBuildPolicy,
    InverseLutContinuityFieldMethod, InverseLutInterpolationMethod,
    InverseLutNumericalPrecision, InverseLutOutputQuantization, InverseLutValidityEncoding,
    LabGridSpec,
};
use crate::output_icc_forward_model::OutputIccForwardModel;
use crate::profile_backed_candidate_preview::render_profile_backed_candidate_preview;
use crate::profile_backed_inverse_lut_artifact::{
    ProfileBackedInverseLutArtifact, load_profile_backed_inverse_lut_artifact,
    save_profile_backed_inverse_lut_artifact,
};
use crate::profile_backed_inverse_lut_builder::{
    ProfileBackedInverseLutBuildStats, build_output_icc_inverse_lut_payload,
};
use crate::profile_backed_optimizer_authority::ProfileBackedOptimizerAuthority;
use crate::profile_backed_optimizer_execution_capture::CapturedProfileBackedOptimizerExecution;

const PROFILE_BACKED_UI_LUT_EXTENSION: &str = "profile-backed-lut.json";
const PROFILE_BACKED_UI_CACHE_DIRECTORY: &str = "profile-backed-optimizer-v1";
const PROFILE_BACKED_UI_AXIS_SAMPLES_V1: u16 = 17;

#[derive(Clone, Debug)]
pub struct PreparedProfileBackedOptimizerExecution {
    pub capture: CapturedProfileBackedOptimizerExecution,
    pub build_stats: ProfileBackedInverseLutBuildStats,
}

#[derive(Clone, Debug)]
pub struct PreparedProfileBackedCandidate {
    pub result: CandidatePreviewResult,
    pub capture: CapturedProfileBackedOptimizerExecution,
    pub build_stats: ProfileBackedInverseLutBuildStats,
}

/// Versioned persistent cache root used by both unified Candidate Preview and final
/// queue preparation. The queue/recovery worker may need to reopen the exact artifact
/// after the UI operation that created it, so this deliberately does not use a random
/// process-temporary path when LOCALAPPDATA is available.
pub fn default_profile_backed_optimizer_artifact_root() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("ShadeEditor").join(PROFILE_BACKED_UI_CACHE_DIRECTORY)
}

/// Frozen v1 policy for the unified profile-backed workflow. Preview and final queue
/// must call this same function so the inverse-LUT build identity cannot drift between
/// what the operator inspected and what production consumes.
///
/// V1 uses a bounded 17^3 full D50-Lab lattice (4,913 nodes) with the already-versioned
/// validity, interpolation, precision and quantization contracts. Unsupported nodes
/// remain explicitly invalid; runtime interpolation never bridges them.
pub fn profile_backed_optimizer_ui_build_policy_v1() -> InverseLutBuildPolicy {
    InverseLutBuildPolicy {
        schema_version: INVERSE_LUT_BUILD_POLICY_SCHEMA_VERSION,
        grid: LabGridSpec {
            l_min: 0.0,
            l_max: 100.0,
            l_samples: PROFILE_BACKED_UI_AXIS_SAMPLES_V1,
            a_min: -128.0,
            a_max: 127.0,
            a_samples: PROFILE_BACKED_UI_AXIS_SAMPLES_V1,
            b_min: -128.0,
            b_max: 127.0,
            b_samples: PROFILE_BACKED_UI_AXIS_SAMPLES_V1,
        },
        interpolation: InverseLutInterpolationMethod::TrilinearV1,
        validity_encoding: InverseLutValidityEncoding::ExplicitNodeValidityMaskV1,
        numerical_precision: InverseLutNumericalPrecision::NormalizedF32V1,
        output_quantization: InverseLutOutputQuantization::ClampScaleRoundV1,
        continuity_field: InverseLutContinuityFieldMethod::IndependentNodeSolvesV1,
    }
}

/// Content-addressed destination for a profile-backed inverse LUT belonging to one
/// exact immutable recipe. The caller owns the cache/root directory policy; this
/// helper owns only recipe identity and filename stability.
pub fn profile_backed_optimizer_artifact_path(
    root: &Path,
    recipe: &ConversionRecipe,
) -> Result<PathBuf, String> {
    validate_profile_backed_recipe(recipe)?;
    let recipe_sha = recipe_sha256(recipe)?;
    Ok(root.join(format!("{recipe_sha}.{PROFILE_BACKED_UI_LUT_EXTENSION}")))
}

/// Prepare exact execution using the same frozen policy/cache contract consumed by
/// unified Candidate Preview. This is also the final-queue preparation entry point for
/// Faces that do not already carry a reusable Candidate capture.
pub fn prepare_default_profile_backed_optimizer_execution(
    recipe: &ConversionRecipe,
) -> Result<PreparedProfileBackedOptimizerExecution, String> {
    validate_profile_backed_recipe(recipe)?;
    let output_path = recipe
        .target
        .output_profile_path
        .as_deref()
        .ok_or_else(|| "Profile-backed optimizer recipe has no Output ICC path.".to_owned())?;
    let output_profile_bytes = fs::read(output_path).map_err(|error| {
        format!(
            "Cannot reopen profile-backed Output ICC {output_path} for LUT preparation: {error}"
        )
    })?;
    let artifact_path = profile_backed_optimizer_artifact_path(
        &default_profile_backed_optimizer_artifact_root(),
        recipe,
    )?;
    prepare_profile_backed_optimizer_execution(
        recipe,
        &output_profile_bytes,
        profile_backed_optimizer_ui_build_policy_v1(),
        &artifact_path,
    )
}

/// Build, publish, reopen and capture the exact profile-backed optimizer execution
/// authority used by unified Candidate Preview and final queueing.
///
/// No measured-characterization evidence is accepted or minted here. Every call is
/// bound to the exact recipe, Output ICC bytes and versioned inverse-LUT policy. The
/// artifact is reopened after atomic publication before the execution capture is
/// returned, so callers never queue authority for bytes that were not persisted.
pub fn prepare_profile_backed_optimizer_execution(
    recipe: &ConversionRecipe,
    output_profile_bytes: &[u8],
    build_policy: InverseLutBuildPolicy,
    artifact_path: &Path,
) -> Result<PreparedProfileBackedOptimizerExecution, String> {
    validate_profile_backed_recipe(recipe)?;
    build_policy
        .validate()
        .map_err(|errors| errors.join(" "))?;

    let output_identity = recipe
        .target
        .output_profile_identity
        .as_ref()
        .ok_or_else(|| "Profile-backed optimizer recipe has no Output ICC identity.".to_owned())?;
    let channel_names = recipe
        .target
        .channels
        .iter()
        .map(|channel| channel.name.clone())
        .collect::<Vec<_>>();
    let model = OutputIccForwardModel::from_bytes(
        output_identity,
        channel_names,
        output_profile_bytes,
        recipe.rendering_intent,
    )?;
    let built = build_output_icc_inverse_lut_payload(recipe, &model, build_policy)
        .map_err(|error| format!("Cannot build profile-backed inverse LUT: {error:?}"))?;
    let artifact = ProfileBackedInverseLutArtifact::from_built(recipe, &built)
        .map_err(|errors| errors.join(" "))?;
    let authority = ProfileBackedOptimizerAuthority::capture(recipe, output_profile_bytes, &built)
        .map_err(|error| format!("Cannot capture profile-backed optimizer authority: {error:?}"))?;

    if let Some(parent) = artifact_path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Cannot create profile-backed optimizer artifact directory {}: {error}",
                parent.display()
            )
        })?;
    }
    save_profile_backed_inverse_lut_artifact(artifact_path, &artifact)?;
    let reopened = load_profile_backed_inverse_lut_artifact(artifact_path)?;
    let capture = CapturedProfileBackedOptimizerExecution::from_verified(
        artifact_path.to_path_buf(),
        &reopened,
        authority,
        recipe,
    )
    .map_err(|errors| errors.join(" "))?;

    Ok(PreparedProfileBackedOptimizerExecution {
        capture,
        build_stats: built.stats,
    })
}

/// Prepare one exact profile-backed execution and immediately render Candidate Preview
/// through that same persisted capture. The returned capture is the only authority the
/// unified UI may retain/promote/queue for this rendered candidate; no second authority
/// is minted after pixels are shown to the operator.
pub fn prepare_and_render_profile_backed_candidate(
    input: CandidatePreviewInput,
    output_profile_bytes: &[u8],
    build_policy: InverseLutBuildPolicy,
    artifact_path: &Path,
    cancellation: &ConversionCancellation,
) -> Result<PreparedProfileBackedCandidate, String> {
    let prepared = prepare_profile_backed_optimizer_execution(
        &input.recipe,
        output_profile_bytes,
        build_policy,
        artifact_path,
    )?;
    let result = render_profile_backed_candidate_preview(
        input,
        &prepared.capture.authority,
        &prepared.capture.lut_artifact_path,
        cancellation,
    )?;
    Ok(PreparedProfileBackedCandidate {
        result,
        capture: prepared.capture,
        build_stats: prepared.build_stats,
    })
}

/// Unified Candidate Preview entry point. It reopens the Output ICC, builds/publishes
/// the exact content-addressed artifact using the frozen UI policy, renders through the
/// persisted capture, and returns that same capture alongside the raster result.
pub fn prepare_and_render_default_profile_backed_candidate(
    input: CandidatePreviewInput,
    cancellation: &ConversionCancellation,
) -> Result<PreparedProfileBackedCandidate, String> {
    validate_profile_backed_recipe(&input.recipe)?;
    let output_path = input
        .recipe
        .target
        .output_profile_path
        .as_deref()
        .ok_or_else(|| "Profile-backed optimizer recipe has no Output ICC path.".to_owned())?;
    let output_profile_bytes = fs::read(output_path).map_err(|error| {
        format!(
            "Cannot reopen profile-backed Output ICC {output_path} for Candidate Preview preparation: {error}"
        )
    })?;
    let artifact_path = profile_backed_optimizer_artifact_path(
        &default_profile_backed_optimizer_artifact_root(),
        &input.recipe,
    )?;
    prepare_and_render_profile_backed_candidate(
        input,
        &output_profile_bytes,
        profile_backed_optimizer_ui_build_policy_v1(),
        &artifact_path,
        cancellation,
    )
}

fn validate_profile_backed_recipe(recipe: &ConversionRecipe) -> Result<(), String> {
    recipe.validate().map_err(|errors| errors.join(" "))?;
    if recipe.engine_mode != ConversionEngineMode::CustomOptimizer {
        return Err("Profile-backed UI execution requires a Custom Optimizer recipe.".to_owned());
    }
    if recipe
        .target
        .characterization_id
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(
            "Measured-characterization recipes must use measured optimizer execution authority."
                .to_owned(),
        );
    }
    let identity = recipe
        .target
        .output_profile_identity
        .as_ref()
        .ok_or_else(|| "Profile-backed optimizer recipe has no Output ICC identity.".to_owned())?;
    if identity.sha256.len() != 64
        || !identity.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("Profile-backed optimizer Output ICC identity is not SHA-256.".to_owned());
    }
    if recipe
        .target
        .output_profile_path
        .as_deref()
        .is_none_or(|path| path.trim().is_empty())
    {
        return Err("Profile-backed optimizer recipe has no Output ICC path.".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionRenderingIntent, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
    use crate::model::IccProfileIdentity;

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn recipe() -> ConversionRecipe {
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: hash('a'),
            },
            source_transparency_policy: None,
            target: ConversionTargetDefinition {
                name: "Profile-backed 4C".to_owned(),
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
                    sha256: hash('b'),
                }),
                output_profile_path: Some(r"C:\Color\Ceramic.icc".to_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: Some(4.0),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: Some(CustomOptimizerSolverConfig::default()),
        }
    }

    #[test]
    fn artifact_locator_is_bound_to_exact_recipe_identity() {
        let root = Path::new(r"C:\Shade\Cache");
        let first = recipe();
        let first_path = profile_backed_optimizer_artifact_path(root, &first).unwrap();
        let first_sha = recipe_sha256(&first).unwrap();
        assert_eq!(
            first_path,
            root.join(format!("{first_sha}.{PROFILE_BACKED_UI_LUT_EXTENSION}"))
        );

        let mut changed = first;
        changed.strategy.black_channel = Some("Black".to_owned());
        changed.strategy.black_generation_strength = 0.7;
        let second_path = profile_backed_optimizer_artifact_path(root, &changed).unwrap();
        assert_ne!(first_path, second_path);
    }

    #[test]
    fn frozen_ui_policy_is_bounded_and_full_lab_domain() {
        let policy = profile_backed_optimizer_ui_build_policy_v1();
        assert!(policy.validate().is_ok());
        assert_eq!(policy.grid.l_min, 0.0);
        assert_eq!(policy.grid.l_max, 100.0);
        assert_eq!(policy.grid.a_min, -128.0);
        assert_eq!(policy.grid.a_max, 127.0);
        assert_eq!(policy.grid.b_min, -128.0);
        assert_eq!(policy.grid.b_max, 127.0);
        assert_eq!(policy.grid.node_count(), Some(4_913));
        assert_eq!(
            policy.continuity_field,
            InverseLutContinuityFieldMethod::IndependentNodeSolvesV1
        );
    }

    #[test]
    fn persistent_default_artifact_root_is_versioned() {
        let root = default_profile_backed_optimizer_artifact_root();
        assert!(root.ends_with(PROFILE_BACKED_UI_CACHE_DIRECTORY));
    }

    #[test]
    fn measured_recipe_cannot_use_profile_backed_ui_execution() {
        let mut measured = recipe();
        measured.target.characterization_id = Some("sha256:measured".to_owned());
        assert!(validate_profile_backed_recipe(&measured).is_err());
    }

    #[test]
    fn standard_engine_cannot_use_profile_backed_ui_execution() {
        let mut standard = recipe();
        standard.engine_mode = ConversionEngineMode::Icc;
        standard.custom_optimizer_solver = None;
        assert!(validate_profile_backed_recipe(&standard).is_err());
    }

    #[test]
    fn candidate_helper_retains_one_exact_execution_capture_contract() {
        let source = include_str!("profile_backed_optimizer_ui_execution.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("prepare_profile_backed_optimizer_execution("));
        assert!(runtime.contains("render_profile_backed_candidate_preview("));
        assert!(runtime.contains("&prepared.capture.authority"));
        assert!(runtime.contains("&prepared.capture.lut_artifact_path"));
        assert!(runtime.contains("capture: prepared.capture"));
        assert!(runtime.contains("profile_backed_optimizer_ui_build_policy_v1()"));
        assert!(runtime.contains("default_profile_backed_optimizer_artifact_root()"));
    }
}
