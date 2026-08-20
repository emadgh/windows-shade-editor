use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::color_conversion::ProjectRole;
use crate::conversion_transaction::{CommittedConversionOutput, CompletedConversionTransaction, ConversionJobCapture};
use crate::icc_conversion_worker::sha256_file;
use crate::model::ShadeProject;
use crate::production_project_compat::{
    AppendConvertedFaceSpec, append_converted_face_to_production_project_at_path,
    validate_existing_production_project_for_append_at_path,
};
use crate::production_project_disposition::ProductionProjectDisposition;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversionRecoveryStage {
    #[default]
    ProductionProjectSavePending,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversionRecoveryRecord {
    #[serde(default)]
    pub stage: ConversionRecoveryStage,
    pub committed_output: CommittedConversionOutput,
    pub production_project_path: PathBuf,
    pub production_project: Option<ShadeProject>,
    pub error: String,
}

pub fn recover_production_project(
    capture: &ConversionJobCapture,
    disposition: &ProductionProjectDisposition,
    recovery: &ConversionRecoveryRecord,
) -> Result<CompletedConversionTransaction, String> {
    if recovery.stage != ConversionRecoveryStage::ProductionProjectSavePending {
        return Err("Unsupported Color Conversion recovery stage.".to_owned());
    }
    capture.validate()?;
    disposition.validate()?;
    validate_recovery_paths(capture, recovery)?;
    verify_committed_output(recovery)?;

    let project = recovery.production_project.as_ref().ok_or_else(|| {
        "Production TIFF is committed, but exact Production project recovery state is unavailable."
            .to_owned()
    })?;
    validate_recovery_project(project, recovery)?;

    match disposition {
        ProductionProjectDisposition::CreateNew => {
            recover_create_new_project(project, &recovery.production_project_path)?;
        }
        ProductionProjectDisposition::AppendExisting {
            expected_project_sha256,
            expected_compatibility,
        } => {
            recover_append_existing_project(
                capture,
                project,
                &recovery.production_project_path,
                expected_project_sha256,
                expected_compatibility,
            )?;
        }
    }

    Ok(CompletedConversionTransaction {
        committed_output: recovery.committed_output.clone(),
        production_project_path: recovery.production_project_path.clone(),
        production_project: project.clone(),
    })
}

fn validate_recovery_paths(
    capture: &ConversionJobCapture,
    recovery: &ConversionRecoveryRecord,
) -> Result<(), String> {
    if !paths_match(&capture.output_tiff_path, &recovery.committed_output.path) {
        return Err(
            "Recovery TIFF path does not match the immutable conversion capture.".to_owned(),
        );
    }
    if !paths_match(
        &capture.production_project_path,
        &recovery.production_project_path,
    ) {
        return Err(
            "Recovery Production project path does not match the immutable conversion capture."
                .to_owned(),
        );
    }
    Ok(())
}

fn verify_committed_output(recovery: &ConversionRecoveryRecord) -> Result<(), String> {
    let actual = sha256_file(&recovery.committed_output.path).map_err(|error| {
        format!(
            "Cannot verify committed production TIFF {} before recovery: {error}",
            recovery.committed_output.path.display()
        )
    })?;
    if !actual.eq_ignore_ascii_case(recovery.committed_output.sha256.trim()) {
        return Err(
            "Committed production TIFF changed after conversion; Production project recovery is blocked."
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_recovery_project(
    project: &ShadeProject,
    recovery: &ConversionRecoveryRecord,
) -> Result<(), String> {
    if project.project_role != ProjectRole::Production {
        return Err("Recovery state is not a Production project.".to_owned());
    }
    if project.faces.is_empty()
        || project.production_provenance.is_empty()
        || project.faces.len() != project.production_provenance.len()
    {
        return Err(
            "Recovery Production project must contain a one-to-one Face/provenance set."
                .to_owned(),
        );
    }
    let provenance = project
        .production_provenance
        .last()
        .expect("non-empty provenance checked above");
    if !paths_match(
        Path::new(&provenance.output_path),
        &recovery.committed_output.path,
    ) {
        return Err(
            "Recovery Production project does not reference the committed TIFF as its newest Face."
                .to_owned(),
        );
    }
    if !provenance
        .output_sha256
        .eq_ignore_ascii_case(recovery.committed_output.sha256.trim())
    {
        return Err(
            "Recovery Production provenance does not match the committed TIFF SHA-256.".to_owned(),
        );
    }
    Ok(())
}

fn recover_create_new_project(project: &ShadeProject, path: &Path) -> Result<(), String> {
    if path.exists() {
        let existing = ShadeProject::load(path).map_err(|error| {
            format!(
                "Production project destination already exists and cannot be verified for recovery: {error}"
            )
        })?;
        if projects_equivalent_at_path(&existing, project, path)? {
            return Ok(());
        }
        return Err(
            "Production project destination is occupied by a different project; recovery will not overwrite it."
                .to_owned(),
        );
    }
    let resolved_faces = project.resolve_face_paths(path);
    project.save_new(path, &resolved_faces)
}

fn recover_append_existing_project(
    capture: &ConversionJobCapture,
    recovery_project: &ShadeProject,
    path: &Path,
    expected_project_sha256: &str,
    expected_compatibility: &crate::production_project_disposition::CapturedProductionCompatibilityKey,
) -> Result<(), String> {
    if !path.exists() {
        return Err(
            "Existing Production project is missing; append recovery cannot recreate its prior production state."
                .to_owned(),
        );
    }

    let current = ShadeProject::load(path).map_err(|error| {
        format!(
            "Cannot load existing Production project {} during recovery: {error}",
            path.display()
        )
    })?;
    if projects_equivalent_at_path(&current, recovery_project, path)? {
        return Ok(());
    }

    let current_sha256 = sha256_file(path)?;
    if !current_sha256.eq_ignore_ascii_case(expected_project_sha256.trim()) {
        return Err(
            "Existing Production project changed after conversion capture; append recovery is blocked."
                .to_owned(),
        );
    }

    let incoming = recovery_project
        .production_provenance
        .last()
        .cloned()
        .ok_or_else(|| "Append recovery has no incoming provenance.".to_owned())?;
    let compatibility = validate_existing_production_project_for_append_at_path(
        &current,
        path,
        &capture.source_project_path,
        &incoming,
    )?;
    if !expected_compatibility.matches_runtime(&compatibility) {
        return Err(
            "Existing Production target compatibility changed; append recovery is blocked."
                .to_owned(),
        );
    }

    let mut reconstructed = current;
    append_converted_face_to_production_project_at_path(
        &mut reconstructed,
        path,
        AppendConvertedFaceSpec {
            source_project_path: &capture.source_project_path,
            output_face_label: &capture.output_face_label,
            provenance: incoming,
        },
    )?;
    if !projects_equivalent_at_path(&reconstructed, recovery_project, path)? {
        return Err(
            "Persisted recovery state does not match a deterministic append of the captured conversion."
                .to_owned(),
        );
    }

    let before_save = sha256_file(path)?;
    if !before_save.eq_ignore_ascii_case(expected_project_sha256.trim()) {
        return Err(
            "Existing Production project changed immediately before recovery save; append was blocked."
                .to_owned(),
        );
    }
    let resolved_faces = recovery_project.resolve_face_paths(path);
    recovery_project.save(path, &resolved_faces)
}

fn projects_equivalent_at_path(
    left: &ShadeProject,
    right: &ShadeProject,
    shade_path: &Path,
) -> Result<bool, String> {
    let left = normalized_project_value(left, shade_path)?;
    let right = normalized_project_value(right, shade_path)?;
    Ok(left == right)
}

fn normalized_project_value(project: &ShadeProject, shade_path: &Path) -> Result<serde_json::Value, String> {
    let mut normalized = project.clone();
    let resolved = project.resolve_face_paths(shade_path);
    for (face, path) in normalized.faces.iter_mut().zip(resolved) {
        face.path = path.to_string_lossy().into_owned();
    }
    serde_json::to_value(normalized)
        .map_err(|error| format!("Cannot normalize Production project for recovery: {error}"))
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
        ConversionRenderingIntent, ConversionSourceRef, ConversionTargetDefinition,
        ProductionProvenance, SeparationStrategy, TargetChannelDefinition,
    };
    use crate::conversion_transaction::{CapturedOutputPolicy, CapturedSourceProfile};
    use crate::model::{ChannelAdjustment, IccProfileIdentity};
    use crate::production_project::{ProductionProjectSpec, build_production_project};
    use crate::production_project_compat::{
        AppendConvertedFaceSpec, ProductionCompatibilityKey,
        append_converted_face_to_production_project_at_path,
    };

    fn hash(byte: u8) -> String {
        format!("{:02x}", byte).repeat(32)
    }

    fn temp_path(label: &str, extension: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shade-conversion-recovery-{label}-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            extension
        ))
    }

    fn recipe() -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: hash(1),
            },
            target: ConversionTargetDefinition {
                name: "Press".to_owned(),
                channels: ["Cyan", "Magenta", "Yellow", "Black"]
                    .into_iter()
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: None,
                    })
                    .collect(),
                bit_depth: 16,
                output_profile_identity: Some(IccProfileIdentity {
                    description: "Press".to_owned(),
                    sha256: hash(2),
                }),
                output_profile_path: Some(r"C:\Color\Press.icc".to_owned()),
                device_link_identity: None,
                device_link_path: None,
                characterization_id: None,
                total_ink_limit: None,
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: true,
            strategy: SeparationStrategy::default(),
            custom_optimizer_solver: None,
        }
    }

    fn provenance(source_project: &Path, output: &Path, output_sha256: String) -> ProductionProvenance {
        ProductionProvenance {
            source: ConversionSourceRef {
                source_project_path: source_project.to_string_lossy().into_owned(),
                source_face_path: r"C:\Design\Face.tif".to_owned(),
                source_snapshot_id: None,
                source_file_sha256: hash(3),
            },
            recipe: recipe(),
            custom_optimizer: None,
            output_path: output.to_string_lossy().into_owned(),
            output_sha256,
            converted_at_unix_ms: 1,
        }
    }

    fn capture(source_project: &Path, output: &Path, project_path: &Path) -> ConversionJobCapture {
        ConversionJobCapture::capture(
            &ShadeProject {
                adjustments: BTreeMap::from([("Red".to_owned(), ChannelAdjustment::default())]),
                ..ShadeProject::default()
            },
            source_project.to_path_buf(),
            hash(4),
            PathBuf::from(r"C:\Design\Face.tif"),
            None,
            hash(3),
            CapturedSourceProfile::Embedded,
            recipe(),
            CapturedOutputPolicy::MustNotExist,
            output.to_path_buf(),
            project_path.to_path_buf(),
            "Production".to_owned(),
            "Converted Face".to_owned(),
        )
        .unwrap()
    }

    #[test]
    fn create_new_recovery_writes_only_missing_project() {
        let source_project = temp_path("source", "shade");
        let output = temp_path("output", "tif");
        let project_path = temp_path("production", "shade");
        fs::write(&output, b"committed tiff bytes").unwrap();
        let output_sha = sha256_file(&output).unwrap();
        let project = build_production_project(ProductionProjectSpec {
            project_name: "Production",
            source_project_path: &source_project,
            output_tiff_path: &output,
            output_face_label: "Converted Face",
            provenance: provenance(&source_project, &output, output_sha.clone()),
        })
        .unwrap();
        let record = ConversionRecoveryRecord {
            stage: ConversionRecoveryStage::ProductionProjectSavePending,
            committed_output: CommittedConversionOutput {
                path: output.clone(),
                sha256: output_sha,
                converted_at_unix_ms: 1,
            },
            production_project_path: project_path.clone(),
            production_project: Some(project),
            error: "simulated save failure".to_owned(),
        };
        let captured = capture(&source_project, &output, &project_path);
        recover_production_project(&captured, &ProductionProjectDisposition::CreateNew, &record)
            .unwrap();
        assert!(project_path.exists());
        assert_eq!(fs::read(&output).unwrap(), b"committed tiff bytes");
        let _ = fs::remove_file(output);
        let _ = fs::remove_file(project_path);
    }

    #[test]
    fn create_new_recovery_refuses_unrelated_existing_project() {
        let source_project = temp_path("source-conflict", "shade");
        let output = temp_path("output-conflict", "tif");
        let project_path = temp_path("production-conflict", "shade");
        fs::write(&output, b"committed").unwrap();
        let output_sha = sha256_file(&output).unwrap();
        let project = build_production_project(ProductionProjectSpec {
            project_name: "Production",
            source_project_path: &source_project,
            output_tiff_path: &output,
            output_face_label: "Converted Face",
            provenance: provenance(&source_project, &output, output_sha.clone()),
        })
        .unwrap();
        ShadeProject::default().save_new(&project_path, &[]).unwrap();
        let record = ConversionRecoveryRecord {
            stage: ConversionRecoveryStage::ProductionProjectSavePending,
            committed_output: CommittedConversionOutput {
                path: output.clone(),
                sha256: output_sha,
                converted_at_unix_ms: 1,
            },
            production_project_path: project_path.clone(),
            production_project: Some(project),
            error: "simulated save failure".to_owned(),
        };
        let captured = capture(&source_project, &output, &project_path);
        assert!(recover_production_project(
            &captured,
            &ProductionProjectDisposition::CreateNew,
            &record
        )
        .is_err());
        let _ = fs::remove_file(output);
        let _ = fs::remove_file(project_path);
    }

    #[test]
    fn append_recovery_refuses_changed_existing_project() {
        let source_project = temp_path("append-source", "shade");
        let first_output = temp_path("append-first", "tif");
        let second_output = temp_path("append-second", "tif");
        let project_path = temp_path("append-production", "shade");
        fs::write(&first_output, b"first").unwrap();
        fs::write(&second_output, b"second").unwrap();
        let first_sha = sha256_file(&first_output).unwrap();
        let second_sha = sha256_file(&second_output).unwrap();
        let mut existing = build_production_project(ProductionProjectSpec {
            project_name: "Production",
            source_project_path: &source_project,
            output_tiff_path: &first_output,
            output_face_label: "Face 1",
            provenance: provenance(&source_project, &first_output, first_sha),
        })
        .unwrap();
        existing.save_new(&project_path, &[first_output.clone()]).unwrap();
        let expected_sha = sha256_file(&project_path).unwrap();
        let key = ProductionCompatibilityKey::from_provenance(&existing.production_provenance[0]).unwrap();
        let disposition = ProductionProjectDisposition::append_existing(expected_sha, &key).unwrap();
        let incoming = provenance(&source_project, &second_output, second_sha.clone());
        append_converted_face_to_production_project_at_path(
            &mut existing,
            &project_path,
            AppendConvertedFaceSpec {
                source_project_path: &source_project,
                output_face_label: "Converted Face",
                provenance: incoming,
            },
        )
        .unwrap();
        fs::write(&project_path, b"changed after capture").unwrap();
        let record = ConversionRecoveryRecord {
            stage: ConversionRecoveryStage::ProductionProjectSavePending,
            committed_output: CommittedConversionOutput {
                path: second_output.clone(),
                sha256: second_sha,
                converted_at_unix_ms: 1,
            },
            production_project_path: project_path.clone(),
            production_project: Some(existing),
            error: "simulated append save failure".to_owned(),
        };
        let captured = capture(&source_project, &second_output, &project_path);
        assert!(recover_production_project(&captured, &disposition, &record).is_err());
        let _ = fs::remove_file(first_output);
        let _ = fs::remove_file(second_output);
        let _ = fs::remove_file(project_path);
    }
}
