use crate::conversion_batch::{ConversionBatchCapture, ConversionBatchCheckpoint};
use crate::conversion_transaction::{
    CompletedConversionTransaction, ConversionCancellation, ConversionPhase, ConversionProgress,
    ConversionTransactionBackend, ConversionTransactionOutcome,
};
use crate::conversion_transaction_disposition::{
    ExistingProductionProjectStore, run_conversion_transaction_with_disposition,
};
use crate::production_project_compat::validate_existing_production_project_baseline_at_path;
use crate::production_project_disposition::ProductionProjectDisposition;

#[derive(Clone, Debug, PartialEq)]
pub struct ConversionBatchProgress {
    pub source_face_index: usize,
    pub ordinal: usize,
    pub face_count: usize,
    pub phase: ConversionPhase,
    pub face_fraction: f32,
    pub overall_fraction: f32,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub enum ConversionBatchStepOutcome {
    AlreadyComplete {
        checkpoint: ConversionBatchCheckpoint,
    },
    CompletedFace {
        checkpoint: ConversionBatchCheckpoint,
        completed: CompletedConversionTransaction,
        source_face_index: usize,
        ordinal: usize,
        batch_complete: bool,
    },
    Halted {
        checkpoint: ConversionBatchCheckpoint,
        source_face_index: usize,
        disposition: Option<ProductionProjectDisposition>,
        outcome: ConversionTransactionOutcome,
    },
}

impl ConversionBatchStepOutcome {
    pub fn checkpoint(&self) -> &ConversionBatchCheckpoint {
        match self {
            Self::AlreadyComplete { checkpoint }
            | Self::CompletedFace { checkpoint, .. }
            | Self::Halted { checkpoint, .. } => checkpoint,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ConversionBatchExecutionOutcome {
    Completed {
        checkpoint: ConversionBatchCheckpoint,
        completions: Vec<CompletedConversionTransaction>,
    },
    Halted {
        checkpoint: ConversionBatchCheckpoint,
        completions: Vec<CompletedConversionTransaction>,
        source_face_index: usize,
        disposition: Option<ProductionProjectDisposition>,
        outcome: ConversionTransactionOutcome,
    },
}

impl ConversionBatchExecutionOutcome {
    pub fn checkpoint(&self) -> &ConversionBatchCheckpoint {
        match self {
            Self::Completed { checkpoint, .. } | Self::Halted { checkpoint, .. } => checkpoint,
        }
    }

    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }
}

/// Execute exactly one pending Face from a deterministic conversion batch.
///
/// Queue callers intentionally use this one-Face step instead of running the
/// whole batch in one worker. After `CompletedFace` the updated checkpoint can
/// be persisted before another Face is allowed to start, making restart/crash
/// recovery deterministic at every committed TIFF/Production-project boundary.
pub fn run_next_conversion_batch_face<B, F>(
    batch: &ConversionBatchCapture,
    checkpoint: ConversionBatchCheckpoint,
    cancellation: &ConversionCancellation,
    backend: &mut B,
    mut report: F,
) -> ConversionBatchStepOutcome
where
    B: ConversionTransactionBackend + ExistingProductionProjectStore,
    F: FnMut(ConversionBatchProgress),
{
    if let Err(error) = batch.validate() {
        return halted_step_before_face(batch, checkpoint, 0, error);
    }
    if let Err(error) = checkpoint.validate_for(batch) {
        return halted_step_before_face(batch, checkpoint, 0, error);
    }

    let ordinal = checkpoint.completed_count();
    let Some(face) = batch.faces.get(ordinal) else {
        return ConversionBatchStepOutcome::AlreadyComplete { checkpoint };
    };

    if cancellation.is_requested() {
        return ConversionBatchStepOutcome::Halted {
            checkpoint,
            source_face_index: face.source_face_index,
            disposition: None,
            outcome: ConversionTransactionOutcome::CancelledBeforeCommit {
                phase: ConversionPhase::CaptureValidation,
                message: "Production conversion batch cancelled before the next Face commit."
                    .to_owned(),
            },
        };
    }

    let disposition = match disposition_for_face(batch, ordinal, backend) {
        Ok(disposition) => disposition,
        Err(error) => {
            return ConversionBatchStepOutcome::Halted {
                checkpoint,
                source_face_index: face.source_face_index,
                disposition: None,
                outcome: ConversionTransactionOutcome::FailedBeforeCommit {
                    phase: ConversionPhase::CaptureValidation,
                    error,
                },
            };
        }
    };

    let face_count = batch.faces.len();
    let mut batch_report = |progress: ConversionProgress| {
        let face_fraction = sanitize_fraction(progress.fraction);
        report(ConversionBatchProgress {
            source_face_index: face.source_face_index,
            ordinal,
            face_count,
            phase: progress.phase,
            face_fraction,
            overall_fraction: sanitize_fraction(
                (ordinal as f32 + face_fraction) / face_count as f32,
            ),
            detail: progress.detail,
        });
    };
    let outcome = run_conversion_transaction_with_disposition(
        &face.capture,
        &disposition,
        cancellation,
        backend,
        &mut batch_report,
    );

    match outcome {
        ConversionTransactionOutcome::Completed(completed) => {
            let mut checkpoint = checkpoint;
            if let Err(error) = checkpoint.record_committed(batch, &completed.committed_output) {
                return ConversionBatchStepOutcome::Halted {
                    checkpoint,
                    source_face_index: face.source_face_index,
                    disposition: Some(disposition),
                    outcome: ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
                        committed_output: completed.committed_output,
                        production_project_path: completed.production_project_path,
                        production_project: Some(completed.production_project),
                        error: format!(
                            "Converted Face committed but batch checkpoint could not advance: {error}"
                        ),
                    },
                };
            }
            let batch_complete = checkpoint.completed_count() == face_count;
            ConversionBatchStepOutcome::CompletedFace {
                checkpoint,
                completed,
                source_face_index: face.source_face_index,
                ordinal,
                batch_complete,
            }
        }
        outcome => ConversionBatchStepOutcome::Halted {
            checkpoint,
            source_face_index: face.source_face_index,
            disposition: Some(disposition),
            outcome,
        },
    }
}

/// Convenience runner for domain tests/callers that do not require a durable
/// checkpoint between Faces. Persistent queue execution should call
/// `run_next_conversion_batch_face` and save the returned checkpoint before
/// starting another Face.
pub fn run_conversion_batch<B, F>(
    batch: &ConversionBatchCapture,
    mut checkpoint: ConversionBatchCheckpoint,
    cancellation: &ConversionCancellation,
    backend: &mut B,
    mut report: F,
) -> ConversionBatchExecutionOutcome
where
    B: ConversionTransactionBackend + ExistingProductionProjectStore,
    F: FnMut(ConversionBatchProgress),
{
    let mut completions = Vec::new();
    loop {
        match run_next_conversion_batch_face(
            batch,
            checkpoint,
            cancellation,
            backend,
            &mut report,
        ) {
            ConversionBatchStepOutcome::AlreadyComplete {
                checkpoint: completed_checkpoint,
            } => {
                return ConversionBatchExecutionOutcome::Completed {
                    checkpoint: completed_checkpoint,
                    completions,
                };
            }
            ConversionBatchStepOutcome::CompletedFace {
                checkpoint: next_checkpoint,
                completed,
                batch_complete,
                ..
            } => {
                checkpoint = next_checkpoint;
                completions.push(completed);
                if batch_complete {
                    return ConversionBatchExecutionOutcome::Completed {
                        checkpoint,
                        completions,
                    };
                }
            }
            ConversionBatchStepOutcome::Halted {
                checkpoint: halted_checkpoint,
                source_face_index,
                disposition,
                outcome,
            } => {
                return ConversionBatchExecutionOutcome::Halted {
                    checkpoint: halted_checkpoint,
                    completions,
                    source_face_index,
                    disposition,
                    outcome,
                };
            }
        }
    }
}

fn disposition_for_face<B>(
    batch: &ConversionBatchCapture,
    ordinal: usize,
    backend: &mut B,
) -> Result<ProductionProjectDisposition, String>
where
    B: ExistingProductionProjectStore,
{
    if ordinal == 0 {
        return Ok(batch.production_project_disposition.clone());
    }

    let project_path = batch.production_project_path();
    let loaded = backend.load_existing_production_project(project_path)?;
    let compatibility = validate_existing_production_project_baseline_at_path(
        &loaded.project,
        project_path,
        batch.source_project_path(),
    )?;
    match &batch.production_project_disposition {
        ProductionProjectDisposition::UpdateExistingRoute {
            route_policy_sha256,
            allow_production_work_discard,
            ..
        } => ProductionProjectDisposition::update_existing_route(
            loaded.file_sha256,
            &compatibility,
            route_policy_sha256.clone(),
            *allow_production_work_discard,
        ),
        ProductionProjectDisposition::CreateNew
        | ProductionProjectDisposition::AppendExisting { .. } => {
            ProductionProjectDisposition::append_existing(loaded.file_sha256, &compatibility)
        }
    }
}

fn halted_step_before_face(
    batch: &ConversionBatchCapture,
    checkpoint: ConversionBatchCheckpoint,
    ordinal: usize,
    error: String,
) -> ConversionBatchStepOutcome {
    let source_face_index = batch
        .faces
        .get(ordinal)
        .map(|face| face.source_face_index)
        .unwrap_or(0);
    ConversionBatchStepOutcome::Halted {
        checkpoint,
        source_face_index,
        disposition: None,
        outcome: ConversionTransactionOutcome::FailedBeforeCommit {
            phase: ConversionPhase::CaptureValidation,
            error,
        },
    }
}

fn sanitize_fraction(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
        ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
        TargetChannelDefinition,
    };
    use crate::conversion_batch::{ConversionBatchFaceCapture, ConversionBatchScope};
    use crate::conversion_transaction::{
        CapturedOutputPolicy, CapturedSourceProfile, CommittedConversionOutput,
    };
    use crate::conversion_transaction_disposition::LoadedExistingProductionProject;
    use crate::model::{IccProfileIdentity, ShadeProject};

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn recipe(source_hash: &str) -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "Source RGB".to_owned(),
                sha256: source_hash.to_owned(),
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

    fn face(index: usize, source_hash: &str) -> ConversionBatchFaceCapture {
        let capture = crate::conversion_transaction::ConversionJobCapture::capture(
            &ShadeProject::default(),
            PathBuf::from(r"C:\Design\Source.shade"),
            hash('a'),
            PathBuf::from(format!(r"C:\Design\Face-{index}.tif")),
            Some(5),
            hash(if index == 0 { 'b' } else { 'c' }),
            CapturedSourceProfile::Embedded,
            recipe(source_hash),
            CapturedOutputPolicy::MustNotExist,
            PathBuf::from(format!(r"C:\Production\Face-{index}.tif")),
            PathBuf::from(r"C:\Production\Job.shade"),
            "Production".to_owned(),
            format!("Face {index}"),
        )
        .unwrap();
        ConversionBatchFaceCapture {
            source_face_index: index,
            capture,
        }
    }

    fn batch() -> ConversionBatchCapture {
        ConversionBatchCapture::capture(
            ConversionBatchScope::AllFaces,
            2,
            ProductionProjectDisposition::CreateNew,
            vec![face(0, &hash('1')), face(1, &hash('2'))],
        )
        .unwrap()
    }

    struct MockBackend {
        project: Option<ShadeProject>,
        project_sha256: Option<String>,
        generation: u32,
        fail_source: Option<PathBuf>,
        fail_project_save: bool,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                project: None,
                project_sha256: None,
                generation: 0,
                fail_source: None,
                fail_project_save: false,
            }
        }

        fn advance_project_identity(&mut self) {
            self.generation += 1;
            self.project_sha256 = Some(format!("{:064x}", self.generation));
        }
    }

    impl ConversionTransactionBackend for MockBackend {
        fn render_convert_and_commit(
            &mut self,
            capture: &crate::conversion_transaction::ConversionJobCapture,
            _cancellation: &ConversionCancellation,
            report: &mut dyn FnMut(ConversionProgress),
        ) -> Result<CommittedConversionOutput, String> {
            report(ConversionProgress::new(
                ConversionPhase::ColorConversion,
                0.5,
                "mock conversion",
            ));
            if self
                .fail_source
                .as_deref()
                .is_some_and(|path| path == capture.source_face_path.as_path())
            {
                return Err("mock Face conversion failure".to_owned());
            }
            Ok(CommittedConversionOutput {
                path: capture.output_tiff_path.clone(),
                sha256: hash('e'),
                converted_at_unix_ms: 1000 + self.generation as i64,
            })
        }

        fn save_production_project(
            &mut self,
            _path: &Path,
            project: &ShadeProject,
        ) -> Result<(), String> {
            if self.fail_project_save {
                return Err("mock Production project save failure".to_owned());
            }
            self.project = Some(project.clone());
            self.advance_project_identity();
            Ok(())
        }
    }

    impl ExistingProductionProjectStore for MockBackend {
        fn load_existing_production_project(
            &mut self,
            _path: &Path,
        ) -> Result<LoadedExistingProductionProject, String> {
            Ok(LoadedExistingProductionProject {
                project: self
                    .project
                    .clone()
                    .ok_or_else(|| "mock Production project does not exist".to_owned())?,
                file_sha256: self
                    .project_sha256
                    .clone()
                    .ok_or_else(|| "mock Production project has no identity".to_owned())?,
            })
        }

        fn save_existing_production_project(
            &mut self,
            _path: &Path,
            expected_sha256: &str,
            project: &ShadeProject,
        ) -> Result<(), String> {
            if self.project_sha256.as_deref() != Some(expected_sha256) {
                return Err("mock optimistic Production SHA mismatch".to_owned());
            }
            if self.fail_project_save {
                return Err("mock Production project save failure".to_owned());
            }
            self.project = Some(project.clone());
            self.advance_project_identity();
            Ok(())
        }
    }

    #[test]
    fn one_step_commits_exactly_one_face_before_returning_checkpoint() {
        let batch = batch();
        let mut backend = MockBackend::new();
        let step = run_next_conversion_batch_face(
            &batch,
            ConversionBatchCheckpoint::default(),
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        let ConversionBatchStepOutcome::CompletedFace {
            checkpoint,
            source_face_index,
            batch_complete,
            ..
        } = step
        else {
            panic!("first batch step should commit one Face");
        };
        assert_eq!(source_face_index, 0);
        assert_eq!(checkpoint.completed_count(), 1);
        assert!(!batch_complete);
        assert_eq!(backend.project.as_ref().unwrap().faces.len(), 1);
    }

    #[test]
    fn two_faces_commit_into_one_production_project_in_source_order() {
        let batch = batch();
        let mut backend = MockBackend::new();
        let outcome = run_conversion_batch(
            &batch,
            ConversionBatchCheckpoint::default(),
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        let ConversionBatchExecutionOutcome::Completed {
            checkpoint,
            completions,
        } = outcome
        else {
            panic!("batch should complete");
        };
        assert_eq!(checkpoint.completed_count(), 2);
        assert_eq!(completions.len(), 2);
        let project = backend.project.unwrap();
        assert_eq!(project.faces.len(), 2);
        assert_eq!(project.production_provenance.len(), 2);
        assert!(project.faces[0].path.contains("Face-0"));
        assert!(project.faces[1].path.contains("Face-1"));
    }

    #[test]
    fn failed_second_face_keeps_first_commit_and_retry_resumes_second_only() {
        let batch = batch();
        let mut backend = MockBackend::new();
        backend.fail_source = Some(batch.faces[1].capture.source_face_path.clone());
        let first = run_conversion_batch(
            &batch,
            ConversionBatchCheckpoint::default(),
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        let ConversionBatchExecutionOutcome::Halted {
            checkpoint,
            source_face_index,
            ..
        } = first
        else {
            panic!("second Face should halt batch");
        };
        assert_eq!(source_face_index, 1);
        assert_eq!(checkpoint.completed_count(), 1);
        assert_eq!(backend.project.as_ref().unwrap().faces.len(), 1);

        backend.fail_source = None;
        let resumed = run_conversion_batch(
            &batch,
            checkpoint,
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        assert!(resumed.is_complete());
        assert_eq!(resumed.checkpoint().completed_count(), 2);
        assert_eq!(backend.project.as_ref().unwrap().faces.len(), 2);
    }

    #[test]
    fn committed_second_face_halt_carries_exact_append_disposition() {
        let batch = batch();
        let mut backend = MockBackend::new();
        let first = run_next_conversion_batch_face(
            &batch,
            ConversionBatchCheckpoint::default(),
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        let ConversionBatchStepOutcome::CompletedFace { checkpoint, .. } = first else {
            panic!("first Face should commit");
        };
        let expected_sha256 = backend.project_sha256.clone().unwrap();
        backend.fail_project_save = true;

        let second = run_next_conversion_batch_face(
            &batch,
            checkpoint,
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        let ConversionBatchStepOutcome::Halted {
            source_face_index,
            disposition: Some(ProductionProjectDisposition::AppendExisting {
                expected_project_sha256,
                ..
            }),
            outcome: ConversionTransactionOutcome::OutputCommittedNeedsRecovery { .. },
            ..
        } = second
        else {
            panic!("second Face project-save failure should preserve exact append disposition");
        };
        assert_eq!(source_face_index, 1);
        assert_eq!(expected_project_sha256, expected_sha256);
    }

    #[test]
    fn overall_progress_is_monotonic_across_faces() {
        let batch = batch();
        let mut backend = MockBackend::new();
        let mut fractions = Vec::new();
        let outcome = run_conversion_batch(
            &batch,
            ConversionBatchCheckpoint::default(),
            &ConversionCancellation::default(),
            &mut backend,
            |progress| fractions.push(progress.overall_fraction),
        );
        assert!(outcome.is_complete());
        assert!(!fractions.is_empty());
        assert!(fractions.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(fractions.iter().all(|value| (0.0..=1.0).contains(value)));
    }
}
