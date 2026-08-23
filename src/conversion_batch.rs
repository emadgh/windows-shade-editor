use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::color_conversion::{
    ConversionEngineMode, ConversionRecipe, ConversionRenderingIntent, ConversionTargetDefinition,
    SeparationStrategy,
};
use crate::conversion_transaction::{CommittedConversionOutput, ConversionJobCapture};
use crate::custom_optimizer_config::CustomOptimizerSolverConfig;
use crate::production_project_disposition::ProductionProjectDisposition;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversionBatchScope {
    CurrentFace,
    SelectedFaces,
    AllFaces,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversionBatchFaceCapture {
    pub source_face_index: usize,
    pub capture: ConversionJobCapture,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversionBatchCapture {
    pub scope: ConversionBatchScope,
    pub source_face_count: usize,
    pub production_project_disposition: ProductionProjectDisposition,
    pub batch_recipe_policy_sha256: String,
    pub faces: Vec<ConversionBatchFaceCapture>,
}

impl ConversionBatchCapture {
    pub fn capture(
        scope: ConversionBatchScope,
        source_face_count: usize,
        production_project_disposition: ProductionProjectDisposition,
        mut faces: Vec<ConversionBatchFaceCapture>,
    ) -> Result<Self, String> {
        for face in &mut faces {
            face.capture.output_tiff_path =
                crate::tiff_output::canonical_destination(&face.capture.output_tiff_path);
        }
        faces.sort_by_key(|face| face.source_face_index);
        let first = faces
            .first()
            .ok_or_else(|| "Conversion batch requires at least one Face.".to_owned())?;
        let batch_recipe_policy_sha256 = batch_recipe_policy_sha256(&first.capture.conversion_recipe)?;
        let batch = Self {
            scope,
            source_face_count,
            production_project_disposition,
            batch_recipe_policy_sha256,
            faces,
        };
        batch.validate()?;
        Ok(batch)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.source_face_count == 0 {
            return Err("Conversion batch Source project must contain at least one Face.".to_owned());
        }
        if self.faces.is_empty() {
            return Err("Conversion batch requires at least one captured Face.".to_owned());
        }
        self.production_project_disposition.validate()?;
        if !is_sha256(&self.batch_recipe_policy_sha256) {
            return Err("Conversion batch requires a canonical recipe-policy SHA-256.".to_owned());
        }

        match self.scope {
            ConversionBatchScope::CurrentFace if self.faces.len() != 1 => {
                return Err("Current Face conversion scope must capture exactly one Face.".to_owned());
            }
            ConversionBatchScope::AllFaces if self.faces.len() != self.source_face_count => {
                return Err(format!(
                    "All Faces scope captured {} of {} Source Faces.",
                    self.faces.len(), self.source_face_count
                ));
            }
            ConversionBatchScope::CurrentFace
            | ConversionBatchScope::SelectedFaces
            | ConversionBatchScope::AllFaces => {}
        }

        let first = &self.faces[0].capture;
        first.validate()?;
        let canonical_source_project = &first.source_project_path;
        let canonical_source_project_sha = &first.source_project_file_sha256;
        let canonical_snapshot = first.source_snapshot_id;
        let canonical_production_project = &first.production_project_path;
        let canonical_project_name = first.production_project_name.trim();
        let canonical_output_policy = first.output_policy;

        let mut previous_index = None;
        let mut source_paths = BTreeSet::new();
        let mut output_paths = BTreeSet::new();

        for (ordinal, face) in self.faces.iter().enumerate() {
            let capture = &face.capture;
            capture.validate().map_err(|error| {
                format!("Batch Face {} failed capture validation: {error}", face.source_face_index + 1)
            })?;
            if face.source_face_index >= self.source_face_count {
                return Err(format!(
                    "Batch Face index {} is outside Source Face count {}.",
                    face.source_face_index, self.source_face_count
                ));
            }
            if previous_index.is_some_and(|previous| face.source_face_index <= previous) {
                return Err("Conversion batch Face indices must be unique and in Source-project order.".to_owned());
            }
            previous_index = Some(face.source_face_index);

            if !paths_match(&capture.source_project_path, canonical_source_project)
                || !capture
                    .source_project_file_sha256
                    .eq_ignore_ascii_case(canonical_source_project_sha)
            {
                return Err(format!(
                    "Batch Face {} was not captured from the same immutable Source project state.",
                    face.source_face_index + 1
                ));
            }
            if capture.source_snapshot_id != canonical_snapshot {
                return Err(format!(
                    "Batch Face {} references a different Source Snapshot; one batch must freeze one saved Source state.",
                    face.source_face_index + 1
                ));
            }
            if !paths_match(&capture.production_project_path, canonical_production_project)
                || capture.production_project_name.trim() != canonical_project_name
            {
                return Err("All Faces in a conversion batch must target one Production project.".to_owned());
            }
            if capture.output_policy != canonical_output_policy {
                return Err("All Faces in a conversion batch must use one output collision policy.".to_owned());
            }

            let policy_sha = batch_recipe_policy_sha256(&capture.conversion_recipe)?;
            if !policy_sha.eq_ignore_ascii_case(&self.batch_recipe_policy_sha256) {
                return Err(format!(
                    "Batch Face {} uses a different target/engine/separation policy. Per-Face target overrides are not supported.",
                    face.source_face_index + 1
                ));
            }

            if !source_paths.insert(path_key(&capture.source_face_path)) {
                return Err("Conversion batch contains the same Source Face path more than once.".to_owned());
            }
            if !output_paths.insert(path_key(&capture.output_tiff_path)) {
                return Err("Conversion batch output TIFF paths must be unique per Face.".to_owned());
            }

            if self.scope == ConversionBatchScope::AllFaces && face.source_face_index != ordinal {
                return Err(format!(
                    "All Faces scope must preserve complete Source order; expected Face index {ordinal}, found {}.",
                    face.source_face_index
                ));
            }
        }

        Ok(())
    }

    pub fn production_project_path(&self) -> &Path {
        &self.faces[0].capture.production_project_path
    }

    pub fn source_project_path(&self) -> &Path {
        &self.faces[0].capture.source_project_path
    }

    pub fn face_count(&self) -> usize {
        self.faces.len()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversionBatchCommittedFace {
    pub source_face_index: usize,
    pub output_path: PathBuf,
    pub output_sha256: String,
    pub converted_at_unix_ms: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversionBatchCheckpoint {
    #[serde(default)]
    pub committed_faces: Vec<ConversionBatchCommittedFace>,
}

impl ConversionBatchCheckpoint {
    pub fn validate_for(&self, batch: &ConversionBatchCapture) -> Result<(), String> {
        batch.validate()?;
        if self.committed_faces.len() > batch.faces.len() {
            return Err("Conversion batch checkpoint contains more committed Faces than the batch.".to_owned());
        }
        for (ordinal, committed) in self.committed_faces.iter().enumerate() {
            let expected = &batch.faces[ordinal];
            if committed.source_face_index != expected.source_face_index {
                return Err(format!(
                    "Batch checkpoint order diverged at ordinal {ordinal}; committed Source Face {} but expected {}.",
                    committed.source_face_index, expected.source_face_index
                ));
            }
            if !paths_match(&committed.output_path, &expected.capture.output_tiff_path) {
                return Err(format!(
                    "Batch checkpoint output path diverged for Source Face {}.",
                    committed.source_face_index + 1
                ));
            }
            if !is_sha256(&committed.output_sha256) {
                return Err(format!(
                    "Batch checkpoint Source Face {} has an invalid committed output SHA-256.",
                    committed.source_face_index + 1
                ));
            }
        }
        Ok(())
    }

    pub fn record_committed(
        &mut self,
        batch: &ConversionBatchCapture,
        output: &CommittedConversionOutput,
    ) -> Result<usize, String> {
        self.validate_for(batch)?;
        let ordinal = self.committed_faces.len();
        let face = batch
            .faces
            .get(ordinal)
            .ok_or_else(|| "Conversion batch is already fully committed.".to_owned())?;
        if !paths_match(&output.path, &face.capture.output_tiff_path) {
            return Err(format!(
                "Committed output path does not match pending batch Face {} destination.",
                face.source_face_index + 1
            ));
        }
        if !is_sha256(&output.sha256) {
            return Err("Committed batch output requires a full SHA-256 identity.".to_owned());
        }
        self.committed_faces.push(ConversionBatchCommittedFace {
            source_face_index: face.source_face_index,
            output_path: output.path.clone(),
            output_sha256: output.sha256.to_ascii_lowercase(),
            converted_at_unix_ms: output.converted_at_unix_ms,
        });
        Ok(face.source_face_index)
    }

    pub fn next_pending_face<'a>(
        &self,
        batch: &'a ConversionBatchCapture,
    ) -> Result<Option<&'a ConversionBatchFaceCapture>, String> {
        self.validate_for(batch)?;
        Ok(batch.faces.get(self.committed_faces.len()))
    }

    pub fn completed_count(&self) -> usize {
        self.committed_faces.len()
    }
}

#[derive(Serialize)]
struct BatchRecipePolicy<'a> {
    schema_version: u32,
    engine_mode: ConversionEngineMode,
    target: &'a ConversionTargetDefinition,
    rendering_intent: ConversionRenderingIntent,
    black_point_compensation: bool,
    strategy: &'a SeparationStrategy,
    custom_optimizer_solver: &'a Option<CustomOptimizerSolverConfig>,
}

pub fn batch_recipe_policy_sha256(recipe: &ConversionRecipe) -> Result<String, String> {
    recipe.validate().map_err(|errors| {
        format!("Cannot fingerprint invalid conversion recipe: {}", errors.join(" "))
    })?;
    let policy = BatchRecipePolicy {
        schema_version: recipe.schema_version,
        engine_mode: recipe.engine_mode,
        target: &recipe.target,
        rendering_intent: recipe.rendering_intent,
        black_point_compensation: recipe.black_point_compensation,
        strategy: &recipe.strategy,
        custom_optimizer_solver: &recipe.custom_optimizer_solver,
    };
    let bytes = serde_json::to_vec(&policy)
        .map_err(|error| format!("Cannot serialize conversion batch policy: {error}"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\").to_ascii_lowercase()
}

fn paths_match(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionTargetDefinition, TargetChannelDefinition,
    };
    use crate::conversion_transaction::{CapturedOutputPolicy, CapturedSourceProfile};
    use crate::model::{IccProfileIdentity, ShadeProject};

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn recipe(source_profile_hash: &str, target_name: &str) -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "Source RGB".to_owned(),
                sha256: source_profile_hash.to_owned(),
            },
            target: ConversionTargetDefinition {
                name: target_name.to_owned(),
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
                    description: "Press CMYK".to_owned(),
                    sha256: hash('d'),
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

    fn face_capture(index: usize, source_profile_hash: &str, target_name: &str) -> ConversionBatchFaceCapture {
        let project = ShadeProject::default();
        let capture = ConversionJobCapture::capture(
            &project,
            PathBuf::from(r"C:\Design\Source.shade"),
            hash('a'),
            PathBuf::from(format!(r"C:\Design\Face-{index}.tif")),
            Some(11),
            hash(char::from_digit(((index % 6) + 1) as u32, 10).unwrap()),
            CapturedSourceProfile::Embedded,
            recipe(source_profile_hash, target_name),
            CapturedOutputPolicy::MustNotExist,
            PathBuf::from(format!(r"C:\Production\Face-{index}-Press.tif")),
            PathBuf::from(r"C:\Production\Job.shade"),
            "Source - Press".to_owned(),
            format!("Face {index} - Press"),
        )
        .unwrap();
        ConversionBatchFaceCapture {
            source_face_index: index,
            capture,
        }
    }

    #[test]
    fn all_faces_are_sorted_into_source_order_and_allow_per_face_source_icc_identity() {
        let batch = ConversionBatchCapture::capture(
            ConversionBatchScope::AllFaces,
            3,
            ProductionProjectDisposition::CreateNew,
            vec![
                face_capture(2, &hash('c'), "Press"),
                face_capture(0, &hash('a'), "Press"),
                face_capture(1, &hash('b'), "Press"),
            ],
        )
        .unwrap();
        assert_eq!(
            batch
                .faces
                .iter()
                .map(|face| face.source_face_index)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert_eq!(batch.face_count(), 3);
    }

    #[test]
    fn target_policy_drift_is_rejected_even_when_each_face_capture_is_valid() {
        let error = ConversionBatchCapture::capture(
            ConversionBatchScope::SelectedFaces,
            3,
            ProductionProjectDisposition::CreateNew,
            vec![
                face_capture(0, &hash('a'), "Press A"),
                face_capture(1, &hash('a'), "Press B"),
            ],
        )
        .expect_err("one batch cannot mix target policies");
        assert!(error.contains("different target/engine/separation policy"));
    }

    #[test]
    fn all_faces_scope_rejects_missing_source_face() {
        let error = ConversionBatchCapture::capture(
            ConversionBatchScope::AllFaces,
            3,
            ProductionProjectDisposition::CreateNew,
            vec![
                face_capture(0, &hash('a'), "Press"),
                face_capture(2, &hash('a'), "Press"),
            ],
        )
        .expect_err("All Faces must be complete");
        assert!(error.contains("captured 2 of 3"));
    }

    #[test]
    fn duplicate_output_destination_is_rejected() {
        let first = face_capture(0, &hash('a'), "Press");
        let mut second = face_capture(1, &hash('a'), "Press");
        second.capture.output_tiff_path = first.capture.output_tiff_path.clone();
        let error = ConversionBatchCapture::capture(
            ConversionBatchScope::SelectedFaces,
            2,
            ProductionProjectDisposition::CreateNew,
            vec![first, second],
        )
        .expect_err("batch outputs must be unique");
        assert!(error.contains("output TIFF paths must be unique"));
    }

    #[test]
    fn checkpoint_commits_only_the_next_face_and_round_trips() {
        let batch = ConversionBatchCapture::capture(
            ConversionBatchScope::SelectedFaces,
            3,
            ProductionProjectDisposition::CreateNew,
            vec![
                face_capture(0, &hash('a'), "Press"),
                face_capture(2, &hash('b'), "Press"),
            ],
        )
        .unwrap();
        let mut checkpoint = ConversionBatchCheckpoint::default();
        let first_output = CommittedConversionOutput {
            path: batch.faces[0].capture.output_tiff_path.clone(),
            sha256: hash('e'),
            converted_at_unix_ms: 1234,
        };
        assert_eq!(checkpoint.record_committed(&batch, &first_output).unwrap(), 0);
        assert_eq!(
            checkpoint
                .next_pending_face(&batch)
                .unwrap()
                .map(|face| face.source_face_index),
            Some(2)
        );

        let json = serde_json::to_string(&checkpoint).unwrap();
        let restored: ConversionBatchCheckpoint = serde_json::from_str(&json).unwrap();
        restored.validate_for(&batch).unwrap();
        assert_eq!(restored, checkpoint);
    }

    #[test]
    fn checkpoint_rejects_output_for_a_later_face_before_its_turn() {
        let batch = ConversionBatchCapture::capture(
            ConversionBatchScope::SelectedFaces,
            2,
            ProductionProjectDisposition::CreateNew,
            vec![
                face_capture(0, &hash('a'), "Press"),
                face_capture(1, &hash('a'), "Press"),
            ],
        )
        .unwrap();
        let mut checkpoint = ConversionBatchCheckpoint::default();
        let wrong_output = CommittedConversionOutput {
            path: batch.faces[1].capture.output_tiff_path.clone(),
            sha256: hash('f'),
            converted_at_unix_ms: 1234,
        };
        let error = checkpoint
            .record_committed(&batch, &wrong_output)
            .expect_err("checkpoint cannot skip batch order");
        assert!(error.contains("pending batch Face"));
    }
}
