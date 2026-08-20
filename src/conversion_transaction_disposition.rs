use std::path::Path;

use crate::conversion_transaction::{
    CommittedConversionOutput, CompletedConversionTransaction, ConversionCancellation,
    ConversionJobCapture, ConversionPhase, ConversionProgress, ConversionTransactionBackend,
    ConversionTransactionOutcome, run_conversion_transaction,
};
use crate::icc_conversion_worker::{FilesystemIccConversionBackend, sha256_file};
use crate::model::ShadeProject;
use crate::production_project_compat::{
    AppendConvertedFaceSpec, append_converted_face_to_production_project_at_path,
    validate_existing_production_project_for_append_at_path,
};
use crate::production_project_disposition::ProductionProjectDisposition;

#[derive(Clone, Debug)]
pub struct LoadedExistingProductionProject {
    pub project: ShadeProject,
    pub file_sha256: String,
}

/// Storage operations specific to append-existing Production transactions.
/// Keeping this separate from `ConversionTransactionBackend` preserves every
/// existing create-new backend/mock while allowing append transactions to use
/// optimistic concurrency around the existing `.shade` file.
pub trait ExistingProductionProjectStore {
    fn load_existing_production_project(
        &mut self,
        path: &Path,
    ) -> Result<LoadedExistingProductionProject, String>;

    fn save_existing_production_project(
        &mut self,
        path: &Path,
        expected_sha256: &str,
        project: &ShadeProject,
    ) -> Result<(), String>;
}

impl ExistingProductionProjectStore for FilesystemIccConversionBackend {
    fn load_existing_production_project(
        &mut self,
        path: &Path,
    ) -> Result<LoadedExistingProductionProject, String> {
        let before = sha256_file(path).map_err(|error| {
            format!(
                "Cannot fingerprint existing Production project {}: {error}",
                path.display()
            )
        })?;
        let project = ShadeProject::load(path).map_err(|error| {
            format!(
                "Cannot load existing Production project {}: {error}",
                path.display()
            )
        })?;
        let after = sha256_file(path).map_err(|error| {
            format!(
                "Cannot re-fingerprint existing Production project {}: {error}",
                path.display()
            )
        })?;
        if before != after {
            return Err(
                "Existing Production project changed while it was being opened; append was not attempted."
                    .to_owned(),
            );
        }
        Ok(LoadedExistingProductionProject {
            project,
            file_sha256: after,
        })
    }

    fn save_existing_production_project(
        &mut self,
        path: &Path,
        expected_sha256: &str,
        project: &ShadeProject,
    ) -> Result<(), String> {
        let current = sha256_file(path).map_err(|error| {
            format!(
                "Cannot verify existing Production project {} before append save: {error}",
                path.display()
            )
        })?;
        if !current.eq_ignore_ascii_case(expected_sha256.trim()) {
            return Err(
                "Existing Production project changed after this conversion job was captured; append save was blocked."
                    .to_owned(),
            );
        }
        let resolved_faces = project.resolve_face_paths(path);
        project.save(path, &resolved_faces)
    }
}

struct ProductionDispositionBackend<'a, B> {
    inner: &'a mut B,
    disposition: &'a ProductionProjectDisposition,
    source_project_path: &'a Path,
    appended_project: Option<ShadeProject>,
}

impl<B> ConversionTransactionBackend for ProductionDispositionBackend<'_, B>
where
    B: ConversionTransactionBackend + ExistingProductionProjectStore,
{
    fn render_convert_and_commit(
        &mut self,
        capture: &ConversionJobCapture,
        cancellation: &ConversionCancellation,
        report: &mut dyn FnMut(ConversionProgress),
    ) -> Result<CommittedConversionOutput, String> {
        self.inner
            .render_convert_and_commit(capture, cancellation, report)
    }

    fn save_production_project(
        &mut self,
        path: &Path,
        generated_project: &ShadeProject,
    ) -> Result<(), String> {
        match self.disposition {
            ProductionProjectDisposition::CreateNew => {
                self.inner.save_production_project(path, generated_project)
            }
            ProductionProjectDisposition::AppendExisting {
                expected_project_sha256,
                expected_compatibility,
            } => {
                if generated_project.faces.len() != 1
                    || generated_project.production_provenance.len() != 1
                {
                    return Err(
                        "Append transaction expected exactly one newly converted Face/provenance pair."
                            .to_owned(),
                    );
                }
                let loaded = self.inner.load_existing_production_project(path)?;
                if !loaded
                    .file_sha256
                    .eq_ignore_ascii_case(expected_project_sha256.trim())
                {
                    return Err(
                        "Existing Production project SHA-256 changed after the conversion job was captured."
                            .to_owned(),
                    );
                }

                let incoming = generated_project.production_provenance[0].clone();
                let compatibility = validate_existing_production_project_for_append_at_path(
                    &loaded.project,
                    path,
                    self.source_project_path,
                    &incoming,
                )?;
                if !expected_compatibility.matches_runtime(&compatibility) {
                    return Err(
                        "Existing Production project target compatibility changed after the conversion job was captured."
                            .to_owned(),
                    );
                }

                let mut appended = loaded.project;
                append_converted_face_to_production_project_at_path(
                    &mut appended,
                    path,
                    AppendConvertedFaceSpec {
                        source_project_path: self.source_project_path,
                        output_face_label: &generated_project.faces[0].label,
                        provenance: incoming,
                    },
                )?;

                // Capture the fully mutated in-memory project before the save so
                // a post-TIFF-commit save failure can surface exact recovery state.
                self.appended_project = Some(appended.clone());
                self.inner.save_existing_production_project(
                    path,
                    expected_project_sha256,
                    &appended,
                )
            }
        }
    }
}

/// Execute one conversion with explicit Production-project destination intent.
///
/// `CreateNew` is byte-for-byte the existing transaction behavior. Append mode
/// reuses that same TIFF/provenance transaction and intercepts only the small
/// Production-project save boundary after the output commit.
pub fn run_conversion_transaction_with_disposition<B, F>(
    capture: &ConversionJobCapture,
    disposition: &ProductionProjectDisposition,
    cancellation: &ConversionCancellation,
    backend: &mut B,
    report: F,
) -> ConversionTransactionOutcome
where
    B: ConversionTransactionBackend + ExistingProductionProjectStore,
    F: FnMut(ConversionProgress),
{
    if let Err(error) = disposition.validate() {
        return ConversionTransactionOutcome::FailedBeforeCommit {
            phase: ConversionPhase::CaptureValidation,
            error,
        };
    }

    let is_append = matches!(
        disposition,
        ProductionProjectDisposition::AppendExisting { .. }
    );
    let mut adapter = ProductionDispositionBackend {
        inner: backend,
        disposition,
        source_project_path: &capture.source_project_path,
        appended_project: None,
    };
    let outcome = run_conversion_transaction(capture, cancellation, &mut adapter, report);

    if !is_append {
        return outcome;
    }

    match outcome {
        ConversionTransactionOutcome::Completed(mut completed) => {
            let Some(project) = adapter.appended_project else {
                return ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
                    committed_output: completed.committed_output,
                    production_project_path: completed.production_project_path,
                    production_project: None,
                    error: "Append transaction completed its save boundary without exposing the appended Production project state."
                        .to_owned(),
                };
            };
            completed.production_project = project;
            ConversionTransactionOutcome::Completed(completed)
        }
        ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
            committed_output,
            production_project_path,
            production_project: _,
            error,
        } => ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
            committed_output,
            production_project_path,
            production_project: adapter.appended_project,
            error,
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
        ConversionRenderingIntent, ConversionSourceRef, ConversionTargetDefinition,
        ProductionProvenance, SeparationStrategy, TargetChannelDefinition,
    };
    use crate::conversion_transaction::{CapturedOutputPolicy, CapturedSourceProfile};
    use crate::export_recipe::ExportRecipe;
    use crate::model::{ChannelAdjustment, FaceRef, FaceStatus, IccProfileIdentity};
    use crate::production_project::{ProductionProjectSpec, build_production_project};
    use crate::production_project_compat::ProductionCompatibilityKey;

    const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

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
                name: "Press CMYK".to_owned(),
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
                    sha256: HASH_B.to_owned(),
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

    fn provenance(output: &Path, source_face: &str, source_hash: &str) -> ProductionProvenance {
        ProductionProvenance {
            source: ConversionSourceRef {
                source_project_path: r"C:\Design\Source.shade".to_owned(),
                source_face_path: source_face.to_owned(),
                source_snapshot_id: None,
                source_file_sha256: HASH_A.to_owned(),
            },
            recipe: recipe(source_hash),
            custom_optimizer: None,
            output_path: output.display().to_string(),
            output_sha256: HASH_C.to_owned(),
            converted_at_unix_ms: 1000,
        }
    }

    fn existing_project() -> ShadeProject {
        let output = Path::new(r"C:\Production\Face-1.tif");
        build_production_project(ProductionProjectSpec {
            project_name: "Production",
            source_project_path: Path::new(r"C:\Design\Source.shade"),
            output_tiff_path: output,
            output_face_label: "Face 1",
            provenance: provenance(output, r"C:\Design\Face-1.png", HASH_A),
        })
        .unwrap()
    }

    fn capture() -> ConversionJobCapture {
        let source = ShadeProject {
            adjustments: BTreeMap::from([("Red".to_owned(), ChannelAdjustment::default())]),
            ..ShadeProject::default()
        };
        ConversionJobCapture::capture(
            &source,
            PathBuf::from(r"C:\Design\Source.shade"),
            HASH_A.to_owned(),
            PathBuf::from(r"C:\Design\Face-2.jpg"),
            None,
            HASH_A.to_owned(),
            CapturedSourceProfile::Embedded,
            recipe(HASH_C),
            CapturedOutputPolicy::MustNotExist,
            PathBuf::from(r"C:\Production\Face-2.tif"),
            PathBuf::from(r"C:\Production\Job.shade"),
            "Production".to_owned(),
            "Face 2".to_owned(),
        )
        .unwrap()
    }

    struct MockBackend {
        existing: ShadeProject,
        existing_sha256: String,
        fail_existing_save: bool,
        create_save_calls: usize,
        existing_save_calls: usize,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                existing: existing_project(),
                existing_sha256: "d".repeat(64),
                fail_existing_save: false,
                create_save_calls: 0,
                existing_save_calls: 0,
            }
        }
    }

    impl ConversionTransactionBackend for MockBackend {
        fn render_convert_and_commit(
            &mut self,
            capture: &ConversionJobCapture,
            _cancellation: &ConversionCancellation,
            _report: &mut dyn FnMut(ConversionProgress),
        ) -> Result<CommittedConversionOutput, String> {
            Ok(CommittedConversionOutput {
                path: capture.output_tiff_path.clone(),
                sha256: HASH_C.to_owned(),
                converted_at_unix_ms: 2000,
            })
        }

        fn save_production_project(
            &mut self,
            _path: &Path,
            _project: &ShadeProject,
        ) -> Result<(), String> {
            self.create_save_calls += 1;
            Ok(())
        }
    }

    impl ExistingProductionProjectStore for MockBackend {
        fn load_existing_production_project(
            &mut self,
            _path: &Path,
        ) -> Result<LoadedExistingProductionProject, String> {
            Ok(LoadedExistingProductionProject {
                project: self.existing.clone(),
                file_sha256: self.existing_sha256.clone(),
            })
        }

        fn save_existing_production_project(
            &mut self,
            _path: &Path,
            expected_sha256: &str,
            project: &ShadeProject,
        ) -> Result<(), String> {
            self.existing_save_calls += 1;
            if expected_sha256 != self.existing_sha256 {
                return Err("mock optimistic-concurrency mismatch".to_owned());
            }
            if self.fail_existing_save {
                return Err("simulated existing project save failure".to_owned());
            }
            self.existing = project.clone();
            Ok(())
        }
    }

    fn append_disposition(backend: &MockBackend) -> ProductionProjectDisposition {
        let key = ProductionCompatibilityKey::from_provenance(
            &backend.existing.production_provenance[0],
        )
        .unwrap();
        ProductionProjectDisposition::append_existing(backend.existing_sha256.clone(), &key)
            .unwrap()
    }

    #[test]
    fn create_new_delegates_to_existing_transaction_behavior() {
        let mut backend = MockBackend::new();
        let outcome = run_conversion_transaction_with_disposition(
            &capture(),
            &ProductionProjectDisposition::CreateNew,
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        let ConversionTransactionOutcome::Completed(completed) = outcome else {
            panic!("create-new transaction should complete");
        };
        assert_eq!(completed.production_project.faces.len(), 1);
        assert_eq!(backend.create_save_calls, 1);
        assert_eq!(backend.existing_save_calls, 0);
    }

    #[test]
    fn compatible_append_returns_and_saves_multi_face_project() {
        let mut backend = MockBackend::new();
        let disposition = append_disposition(&backend);
        let outcome = run_conversion_transaction_with_disposition(
            &capture(),
            &disposition,
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        let ConversionTransactionOutcome::Completed(completed) = outcome else {
            panic!("append transaction should complete");
        };
        assert_eq!(backend.create_save_calls, 0);
        assert_eq!(backend.existing_save_calls, 1);
        assert_eq!(backend.existing.faces.len(), 2);
        assert_eq!(completed.production_project.faces.len(), 2);
        assert!(completed.production_project.faces[1].path.ends_with("Face-2.tif"));
    }

    #[test]
    fn stale_existing_project_hash_fails_after_tiff_commit_without_mutation() {
        let mut backend = MockBackend::new();
        let key = ProductionCompatibilityKey::from_provenance(
            &backend.existing.production_provenance[0],
        )
        .unwrap();
        let disposition = ProductionProjectDisposition::append_existing("e".repeat(64), &key)
            .unwrap();
        let outcome = run_conversion_transaction_with_disposition(
            &capture(),
            &disposition,
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        let ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
            production_project,
            error,
            ..
        } = outcome
        else {
            panic!("stale append must require recovery after committed TIFF");
        };
        assert!(error.contains("SHA-256 changed"));
        assert!(production_project.is_none());
        assert_eq!(backend.existing.faces.len(), 1);
        assert_eq!(backend.existing_save_calls, 0);
    }

    #[test]
    fn existing_project_save_failure_returns_exact_appended_recovery_state() {
        let mut backend = MockBackend::new();
        backend.fail_existing_save = true;
        let disposition = append_disposition(&backend);
        let outcome = run_conversion_transaction_with_disposition(
            &capture(),
            &disposition,
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        let ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
            production_project: Some(recovery_project),
            error,
            ..
        } = outcome
        else {
            panic!("save failure must keep appended project recovery state");
        };
        assert!(error.contains("simulated existing project save failure"));
        assert_eq!(recovery_project.faces.len(), 2);
        assert_eq!(backend.existing.faces.len(), 1);
        assert_eq!(backend.existing_save_calls, 1);
    }

    #[test]
    fn incompatible_expected_target_fails_closed_after_commit() {
        let mut backend = MockBackend::new();
        let mut key = ProductionCompatibilityKey::from_provenance(
            &backend.existing.production_provenance[0],
        )
        .unwrap();
        key.channel_names.swap(0, 1);
        let disposition = ProductionProjectDisposition::append_existing(
            backend.existing_sha256.clone(),
            &key,
        )
        .unwrap();
        let outcome = run_conversion_transaction_with_disposition(
            &capture(),
            &disposition,
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        let ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
            production_project,
            error,
            ..
        } = outcome
        else {
            panic!("compatibility drift must require recovery after committed TIFF");
        };
        assert!(error.contains("target compatibility changed"));
        assert!(production_project.is_none());
        assert_eq!(backend.existing.faces.len(), 1);
    }
}
