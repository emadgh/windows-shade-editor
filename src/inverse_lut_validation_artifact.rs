use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::inverse_lut_validation::InverseLutValidationReport;
use crate::safe_fs;

pub const INVERSE_LUT_VALIDATION_ARTIFACT_FORMAT_VERSION: u32 = 1;
pub const MAX_INVERSE_LUT_VALIDATION_ARTIFACT_BYTES: u64 = 1024 * 1024;
static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct InverseLutValidationArtifactEnvelope {
    format_version: u32,
    report_content_id: String,
    report: InverseLutValidationReport,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedInverseLutValidationArtifact {
    pub report_content_id: String,
    pub report: InverseLutValidationReport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InverseLutValidationPublishOutcome {
    Published,
    ReusedExisting,
}

/// Load and fully revalidate a persisted inverse-LUT validation report.
///
/// The stored report content ID is never trusted: it is recomputed from the
/// complete versioned report, including exact LUT/recipe/characterization
/// bindings and ordered path diagnostics.
pub fn load_inverse_lut_validation_artifact(
    path: &Path,
) -> Result<VerifiedInverseLutValidationArtifact, String> {
    let metadata = fs::metadata(path).map_err(|error| {
        format!(
            "Cannot inspect inverse-LUT validation artifact {}: {error}",
            path.display()
        )
    })?;
    if metadata.len() == 0 || metadata.len() > MAX_INVERSE_LUT_VALIDATION_ARTIFACT_BYTES {
        return Err(format!(
            "Inverse-LUT validation artifact {} has invalid bounded size {} bytes.",
            path.display(),
            metadata.len()
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        format!(
            "Cannot read inverse-LUT validation artifact {}: {error}",
            path.display()
        )
    })?;
    let envelope: InverseLutValidationArtifactEnvelope = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Cannot parse inverse-LUT validation artifact: {error}"))?;
    verify_envelope(envelope)
}

/// Atomically publish an immutable validation report. Existing bytes may only
/// be reused when they revalidate to the exact same report content identity.
pub fn publish_inverse_lut_validation_artifact_if_absent(
    destination: &Path,
    report: &InverseLutValidationReport,
) -> Result<InverseLutValidationPublishOutcome, String> {
    let (expected_content_id, bytes) = prepare(report)?;
    if destination.exists() {
        verify_existing(destination, &expected_content_id)?;
        return Ok(InverseLutValidationPublishOutcome::ReusedExisting);
    }

    let staged = unique_staged_path(destination)?;
    let result = (|| {
        fs::write(&staged, &bytes).map_err(|error| {
            format!(
                "Cannot stage inverse-LUT validation artifact {}: {error}",
                staged.display()
            )
        })?;
        let staged_verified = load_inverse_lut_validation_artifact(&staged)?;
        if staged_verified.report_content_id != expected_content_id {
            return Err("Staged inverse-LUT validation artifact identity changed after write.".to_owned());
        }

        match safe_fs::commit_staged_file_if_absent(&staged, destination) {
            Ok(()) => Ok(InverseLutValidationPublishOutcome::Published),
            Err(commit_error) if destination.exists() => {
                let _ = fs::remove_file(&staged);
                verify_existing(destination, &expected_content_id)?;
                Ok(InverseLutValidationPublishOutcome::ReusedExisting)
            }
            Err(commit_error) => Err(commit_error),
        }
    })();
    if result.is_err() && staged.exists() {
        let _ = fs::remove_file(&staged);
    }
    result
}

fn prepare(report: &InverseLutValidationReport) -> Result<(String, Vec<u8>), String> {
    report.validate().map_err(|errors| errors.join("\n"))?;
    let report_content_id = report.content_id()?;
    let envelope = InverseLutValidationArtifactEnvelope {
        format_version: INVERSE_LUT_VALIDATION_ARTIFACT_FORMAT_VERSION,
        report_content_id: report_content_id.clone(),
        report: report.clone(),
    };
    let bytes = serde_json::to_vec(&envelope)
        .map_err(|error| format!("Cannot serialize inverse-LUT validation artifact: {error}"))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_INVERSE_LUT_VALIDATION_ARTIFACT_BYTES {
        return Err(format!(
            "Inverse-LUT validation artifact exceeds bounded maximum of {MAX_INVERSE_LUT_VALIDATION_ARTIFACT_BYTES} bytes."
        ));
    }
    Ok((report_content_id, bytes))
}

fn verify_envelope(
    envelope: InverseLutValidationArtifactEnvelope,
) -> Result<VerifiedInverseLutValidationArtifact, String> {
    if envelope.format_version != INVERSE_LUT_VALIDATION_ARTIFACT_FORMAT_VERSION {
        return Err(format!(
            "Unsupported inverse-LUT validation artifact format {} (expected {}).",
            envelope.format_version, INVERSE_LUT_VALIDATION_ARTIFACT_FORMAT_VERSION
        ));
    }
    envelope
        .report
        .validate()
        .map_err(|errors| errors.join("\n"))?;
    let actual_content_id = envelope.report.content_id()?;
    if envelope.report_content_id != actual_content_id {
        return Err(format!(
            "Inverse-LUT validation report content-ID mismatch: stored {}, actual {}.",
            envelope.report_content_id, actual_content_id
        ));
    }
    Ok(VerifiedInverseLutValidationArtifact {
        report_content_id: actual_content_id,
        report: envelope.report,
    })
}

fn verify_existing(path: &Path, expected_content_id: &str) -> Result<(), String> {
    let existing = load_inverse_lut_validation_artifact(path)?;
    if existing.report_content_id != expected_content_id {
        return Err(format!(
            "Existing inverse-LUT validation artifact {} has report {}, expected {}. Refusing to replace immutable validation evidence.",
            path.display(),
            existing.report_content_id,
            expected_content_id
        ));
    }
    Ok(())
}

fn unique_staged_path(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "Cannot create inverse-LUT validation folder {}: {error}",
            parent.display()
        )
    })?;
    let file_name = destination
        .file_name()
        .ok_or_else(|| "Inverse-LUT validation destination has no file name.".to_owned())?
        .to_string_lossy();
    for _ in 0..1024 {
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{file_name}.stage-{}-{sequence}",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("Cannot allocate unique inverse-LUT validation staging path.".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inverse_lut_path_validation::{
        InverseLutPathDiagnostic, InverseLutValidationPathKind,
    };
    use crate::inverse_lut_validation::{
        InverseLutValidationPolicy, InverseLutValidationSample, summarize_validation_samples,
    };

    fn id(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    fn bare(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn paths() -> Vec<InverseLutPathDiagnostic> {
        [
            InverseLutValidationPathKind::NeutralAxis,
            InverseLutValidationPathKind::NearNeutralWarm,
            InverseLutValidationPathKind::NearNeutralCool,
            InverseLutValidationPathKind::AAxis,
            InverseLutValidationPathKind::BAxis,
            InverseLutValidationPathKind::AbDiagonal,
            InverseLutValidationPathKind::AbOpposedDiagonal,
        ]
        .into_iter()
        .map(|kind| InverseLutPathDiagnostic {
            kind,
            sample_count: 5,
            unsupported_samples: 0,
            max_channel_jump: Some(0.0),
            max_normalized_channel_jump: Some(0.0),
            max_vector_l1_jump: Some(0.0),
            max_vector_l2_jump: Some(0.0),
            max_total_ink_jump: Some(0.0),
            dominant_channel_switches: Some(0),
            max_channel_second_difference: Some(0.0),
            max_normalized_channel_second_difference: Some(0.0),
            max_vector_l1_second_difference: Some(0.0),
            max_vector_l2_second_difference: Some(0.0),
            max_total_ink_second_difference: Some(0.0),
            continuity_violation_count: Some(0),
            curvature_violation_count: Some(0),
        })
        .collect()
    }

    fn report() -> InverseLutValidationReport {
        let sample = InverseLutValidationSample {
            supported: true,
            lut_delta_e00: Some(0.1),
            reference_delta_e00: Some(0.1),
            lut_vs_reference_delta_e00: Some(0.0),
            ink_l1: Some(0.0),
            ink_l2: Some(0.0),
            max_channel_deviation: Some(0.0),
            u8_quantization_l1: Some(0.0),
            u16_quantization_l1: Some(0.0),
            constraints_preserved: true,
        };
        summarize_validation_samples(
            id('a'),
            bare('b'),
            bare('c'),
            id('d'),
            InverseLutValidationPolicy::default(),
            paths(),
            &[sample],
        )
        .unwrap()
    }

    fn temp_folder(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-lut-validation-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn immutable_publish_load_and_reuse_preserve_exact_report_identity() {
        let folder = temp_folder("publish");
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("validation.shade-lut-validation");
        let report = report();
        let expected_id = report.content_id().unwrap();

        assert_eq!(
            publish_inverse_lut_validation_artifact_if_absent(&path, &report).unwrap(),
            InverseLutValidationPublishOutcome::Published
        );
        let loaded = load_inverse_lut_validation_artifact(&path).unwrap();
        assert_eq!(loaded.report_content_id, expected_id);
        assert_eq!(loaded.report, report);
        assert_eq!(
            publish_inverse_lut_validation_artifact_if_absent(&path, &report).unwrap(),
            InverseLutValidationPublishOutcome::ReusedExisting
        );
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn immutable_destination_rejects_a_different_valid_report() {
        let folder = temp_folder("mismatch");
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("validation.shade-lut-validation");
        let first = report();
        publish_inverse_lut_validation_artifact_if_absent(&path, &first).unwrap();

        let mut second = report();
        second.path_diagnostics[0].max_channel_jump = Some(0.01);
        second.passed = second.path_diagnostics[0].passes(&second.policy.path_policy)
            && second.summary.supported_samples > 0;
        assert!(second.validate().is_ok());
        assert!(publish_inverse_lut_validation_artifact_if_absent(&path, &second).is_err());
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn tampered_report_content_id_is_rejected_on_load() {
        let folder = temp_folder("tamper");
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("validation.shade-lut-validation");
        let report = report();
        let (_, bytes) = prepare(&report).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["report_content_id"] = serde_json::Value::String(id('f'));
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(load_inverse_lut_validation_artifact(&path).is_err());
        let _ = fs::remove_dir_all(folder);
    }
}
