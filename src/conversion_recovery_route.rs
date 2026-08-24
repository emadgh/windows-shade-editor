use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::color_conversion::ProjectRole;
use crate::conversion_batch::batch_recipe_policy_sha256;
use crate::conversion_transaction::{
    CommittedConversionOutput, CompletedConversionTransaction, ConversionJobCapture,
};
use crate::icc_conversion_worker::sha256_file;
use crate::model::ShadeProject;
use crate::production_project_compat::{
    AppendConvertedFaceSpec, append_converted_face_to_production_project_at_path,
    validate_existing_production_project_baseline_at_path,
    validate_existing_production_project_for_append_at_path,
};
use crate::production_project_disposition::{
    CapturedProductionCompatibilityKey, ProductionProjectDisposition,
};

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
        } => recover_append_existing_project(
            capture,
            project,
            recovery,
            expected_project_sha256,
            expected_compatibility,
        )?,
        ProductionProjectDisposition::UpdateExistingRoute {
            expected_project_sha256,
            expected_compatibility,
            route_policy_sha256,
            ..
        } => recover_update_existing_route_project(
            capture,
            project,
            recovery,
            expected_project_sha256,
            expected_compatibility,
            route_policy_sha256,
        )?,
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
    recovery_committed_index(project, recovery)?;
    Ok(())
}

fn recovery_committed_index(
    project: &ShadeProject,
    recovery: &ConversionRecoveryRecord,
) -> Result<usize, String> {
    let matches = project
        .production_provenance
        .iter()
        .enumerate()
        .filter(|(_, provenance)| {
            paths_match(
                Path::new(&provenance.output_path),
                &recovery.committed_output.path,
            ) && provenance
                .output_sha256
                .eq_ignore_ascii_case(recovery.committed_output.sha256.trim())
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => Err(
            "Recovery Production project does not contain provenance for the committed TIFF."
                .to_owned(),
        ),
        _ => Err(
            "Recovery Production project contains ambiguous duplicate provenance for the committed TIFF."
                .to_owned(),
        ),
    }
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
    recovery: &ConversionRecoveryRecord,
    expected_project_sha256: &str,
    expected_compatibility: &CapturedProductionCompatibilityKey,
) -> Result<(), String> {
    let path = &recovery.production_project_path;
    if !path.exists() {
        return Err(
            "Existing Production project is missing; append recovery cannot recreate its prior production state."
                .to_owned(),
        );
    }

    let current = load_current_project(path, "append")?;
    if projects_equivalent_at_path(&current, recovery_project, path)? {
        return Ok(());
    }
    ensure_project_sha(path, expected_project_sha256, "append recovery")?;

    let incoming_index = recovery_committed_index(recovery_project, recovery)?;
    let incoming = recovery_project.production_provenance[incoming_index].clone();
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
            output_face_label: &recovery_project.faces[incoming_index].label,
            provenance: incoming,
        },
    )?;
    if !projects_equivalent_at_path(&reconstructed, recovery_project, path)? {
        return Err(
            "Persisted recovery state does not match a deterministic append of the captured conversion."
                .to_owned(),
        );
    }

    save_recovery_project_if_unchanged(
        recovery_project,
        path,
        expected_project_sha256,
        "append recovery",
    )
}

fn recover_update_existing_route_project(
    capture: &ConversionJobCapture,
    recovery_project: &ShadeProject,
    recovery: &ConversionRecoveryRecord,
    expected_project_sha256: &str,
    expected_compatibility: &CapturedProductionCompatibilityKey,
    route_policy_sha256: &str,
) -> Result<(), String> {
    let path = &recovery.production_project_path;
    if !path.exists() {
        return Err(
            "Existing Production route project is missing; same-route recovery cannot recreate prior route history."
                .to_owned(),
        );
    }

    let current = load_current_project(path, "route-update")?;
    if projects_equivalent_at_path(&current, recovery_project, path)? {
        return Ok(());
    }
    ensure_project_sha(path, expected_project_sha256, "route-update recovery")?;

    let compatibility = validate_existing_production_project_baseline_at_path(
        &current,
        path,
        &capture.source_project_path,
    )?;
    if !expected_compatibility.matches_runtime(&compatibility) {
        return Err(
            "Existing Production route target compatibility changed; recovery is blocked."
                .to_owned(),
        );
    }

    let incoming_index = recovery_committed_index(recovery_project, recovery)?;
    let incoming = recovery_project.production_provenance[incoming_index].clone();
    let incoming_policy = batch_recipe_policy_sha256(&incoming.recipe)?;
    if !incoming_policy.eq_ignore_ascii_case(route_policy_sha256.trim()) {
        return Err(
            "Recovery provenance no longer matches the captured conversion route policy."
                .to_owned(),
        );
    }

    let matching_current = current
        .production_provenance
        .iter()
        .enumerate()
        .filter(|(_, provenance)| {
            paths_match_str(
                &provenance.source.source_project_path,
                &capture.source_project_path.to_string_lossy(),
            ) && paths_match_str(
                &provenance.source.source_face_path,
                &capture.source_face_path.to_string_lossy(),
            )
        })
        .map(|(index, provenance)| (index, provenance))
        .collect::<Vec<_>>();
    if matching_current.len() > 1 {
        return Err(
            "Existing Production route contains duplicate Source Face provenance; recovery is ambiguous."
                .to_owned(),
        );
    }

    let mut reconstructed = current;
    if let Some((index, previous)) = matching_current.first().copied() {
        let previous_policy = batch_recipe_policy_sha256(&previous.recipe)?;
        if !previous_policy.eq_ignore_ascii_case(route_policy_sha256.trim()) {
            return Err(
                "Existing output belongs to a different conversion route; recovery overwrite is blocked."
                    .to_owned(),
            );
        }
        if !paths_match(Path::new(&previous.output_path), Path::new(&incoming.output_path)) {
            return Err(
                "Saved same-route output mapping changed; recovery overwrite is blocked."
                    .to_owned(),
            );
        }
        reconstructed.faces[index] = recovery_project.faces[incoming_index].clone();
        reconstructed.production_provenance[index] = incoming;
    } else {
        append_converted_face_to_production_project_at_path(
            &mut reconstructed,
            path,
            AppendConvertedFaceSpec {
                source_project_path: &capture.source_project_path,
                output_face_label: &recovery_project.faces[incoming_index].label,
                provenance: incoming,
            },
        )?;
    }

    if !projects_equivalent_at_path(&reconstructed, recovery_project, path)? {
        return Err(
            "Persisted recovery state does not match deterministic same-route append/replacement."
                .to_owned(),
        );
    }

    save_recovery_project_if_unchanged(
        recovery_project,
        path,
        expected_project_sha256,
        "route-update recovery",
    )
}

fn load_current_project(path: &Path, operation: &str) -> Result<ShadeProject, String> {
    ShadeProject::load(path).map_err(|error| {
        format!(
            "Cannot load existing Production project {} during {operation}: {error}",
            path.display()
        )
    })
}

fn ensure_project_sha(path: &Path, expected: &str, operation: &str) -> Result<(), String> {
    let current = sha256_file(path)?;
    if !current.eq_ignore_ascii_case(expected.trim()) {
        return Err(format!(
            "Existing Production project changed after conversion capture; {operation} is blocked."
        ));
    }
    Ok(())
}

fn save_recovery_project_if_unchanged(
    project: &ShadeProject,
    path: &Path,
    expected_project_sha256: &str,
    operation: &str,
) -> Result<(), String> {
    ensure_project_sha(path, expected_project_sha256, operation)?;
    let resolved_faces = project.resolve_face_paths(path);
    project.save(path, &resolved_faces)
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

fn normalized_project_value(
    project: &ShadeProject,
    shade_path: &Path,
) -> Result<serde_json::Value, String> {
    let mut normalized = project.clone();
    let resolved = project.resolve_face_paths(shade_path);
    for (face, path) in normalized.faces.iter_mut().zip(resolved) {
        face.path = path.to_string_lossy().into_owned();
    }
    serde_json::to_value(normalized)
        .map_err(|error| format!("Cannot normalize Production project for recovery: {error}"))
}

fn paths_match(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}

fn paths_match_str(left: &str, right: &str) -> bool {
    left.trim()
        .replace('/', "\\")
        .eq_ignore_ascii_case(&right.trim().replace('/', "\\"))
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase()
}
