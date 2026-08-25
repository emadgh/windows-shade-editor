use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::conversion_route_migration::RouteMigrationPlan;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteMigrationExecutionStage {
    #[default]
    Staging,
    CommitPending,
    ProductionProjectSavePending,
    Complete,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StagedRouteMigrationOutput {
    pub ordinal: usize,
    pub staged_path: PathBuf,
    pub sha256: String,
    pub converted_at_unix_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CommittedRouteMigrationOutput {
    pub ordinal: usize,
    pub final_path: PathBuf,
    pub backup_path: PathBuf,
    pub previous_sha256: String,
    pub new_sha256: String,
    pub converted_at_unix_ms: i64,
}

/// Durable project-wide migration progress.
///
/// Staging is an ordered prefix and never mutates final outputs. Commit starts only after every
/// replacement has been staged. During commit each old final is moved to a unique backup before
/// the corresponding staged output is moved into the canonical final path. A recovery journal can
/// therefore distinguish old/new/half-swapped states from hashes alone.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RouteMigrationCheckpoint {
    #[serde(default)]
    pub stage: RouteMigrationExecutionStage,
    #[serde(default)]
    pub staged_outputs: Vec<StagedRouteMigrationOutput>,
    #[serde(default)]
    pub committed_outputs: Vec<CommittedRouteMigrationOutput>,
}

impl RouteMigrationCheckpoint {
    pub fn validate_for(&self, plan: &RouteMigrationPlan) -> Result<(), String> {
        plan.validate()?;
        if self.staged_outputs.len() > plan.faces.len()
            || self.committed_outputs.len() > self.staged_outputs.len()
        {
            return Err("Route migration checkpoint contains impossible output counts.".to_owned());
        }

        let mut staged_paths = BTreeSet::new();
        for (ordinal, staged) in self.staged_outputs.iter().enumerate() {
            if staged.ordinal != ordinal {
                return Err("Route migration staged outputs must form an ordered prefix.".to_owned());
            }
            if !is_sha256(&staged.sha256) {
                return Err("Route migration staged output has an invalid SHA-256.".to_owned());
            }
            let final_path = &plan.faces[ordinal].replacement.output_tiff_path;
            if paths_match(&staged.staged_path, final_path)
                || plan
                    .faces
                    .iter()
                    .any(|face| paths_match(&staged.staged_path, &face.replacement.output_tiff_path))
            {
                return Err(
                    "Route migration staging path must be distinct from every canonical output path."
                        .to_owned(),
                );
            }
            if !staged_paths.insert(path_key(&staged.staged_path)) {
                return Err("Route migration staging paths must be unique.".to_owned());
            }
        }

        let mut backup_paths = BTreeSet::new();
        for (ordinal, committed) in self.committed_outputs.iter().enumerate() {
            if committed.ordinal != ordinal {
                return Err("Route migration committed outputs must form an ordered prefix.".to_owned());
            }
            let planned = &plan.faces[ordinal];
            let staged = &self.staged_outputs[ordinal];
            if !paths_match(&committed.final_path, &planned.replacement.output_tiff_path)
                || !committed
                    .previous_sha256
                    .eq_ignore_ascii_case(&planned.previous_output_sha256)
                || !committed.new_sha256.eq_ignore_ascii_case(&staged.sha256)
                || committed.converted_at_unix_ms != staged.converted_at_unix_ms
            {
                return Err(
                    "Route migration committed output diverges from its immutable plan/staging record."
                        .to_owned(),
                );
            }
            if !is_sha256(&committed.previous_sha256) || !is_sha256(&committed.new_sha256) {
                return Err("Route migration committed output has an invalid SHA-256.".to_owned());
            }
            if paths_match(&committed.backup_path, &committed.final_path)
                || staged_paths.contains(&path_key(&committed.backup_path))
                || plan.faces.iter().any(|face| {
                    paths_match(&committed.backup_path, &face.replacement.output_tiff_path)
                })
                || !backup_paths.insert(path_key(&committed.backup_path))
            {
                return Err(
                    "Route migration backup path collides with staged/final migration ownership."
                        .to_owned(),
                );
            }
        }

        match self.stage {
            RouteMigrationExecutionStage::Staging => {
                if !self.committed_outputs.is_empty() {
                    return Err("Route migration cannot commit outputs while still staging.".to_owned());
                }
            }
            RouteMigrationExecutionStage::CommitPending => {
                if self.staged_outputs.len() != plan.faces.len() {
                    return Err(
                        "Route migration commit cannot begin until every Face is staged.".to_owned(),
                    );
                }
            }
            RouteMigrationExecutionStage::ProductionProjectSavePending
            | RouteMigrationExecutionStage::Complete => {
                if self.staged_outputs.len() != plan.faces.len()
                    || self.committed_outputs.len() != plan.faces.len()
                {
                    return Err(
                        "Route migration project save/complete stage requires every TIFF commit."
                            .to_owned(),
                    );
                }
            }
        }
        Ok(())
    }

    pub fn next_staging_ordinal(&self, plan: &RouteMigrationPlan) -> Result<Option<usize>, String> {
        self.validate_for(plan)?;
        if self.stage != RouteMigrationExecutionStage::Staging {
            return Ok(None);
        }
        Ok((self.staged_outputs.len() < plan.faces.len()).then_some(self.staged_outputs.len()))
    }

    pub fn record_staged(
        &mut self,
        plan: &RouteMigrationPlan,
        staged_path: PathBuf,
        sha256: impl Into<String>,
        converted_at_unix_ms: i64,
    ) -> Result<usize, String> {
        self.validate_for(plan)?;
        if self.stage != RouteMigrationExecutionStage::Staging {
            return Err("Route migration is no longer accepting staged outputs.".to_owned());
        }
        let ordinal = self.staged_outputs.len();
        if ordinal >= plan.faces.len() {
            return Err("All route migration Faces are already staged.".to_owned());
        }
        let staged = StagedRouteMigrationOutput {
            ordinal,
            staged_path,
            sha256: sha256.into().to_ascii_lowercase(),
            converted_at_unix_ms,
        };
        self.staged_outputs.push(staged);
        if let Err(error) = self.validate_for(plan) {
            self.staged_outputs.pop();
            return Err(error);
        }
        Ok(ordinal)
    }

    pub fn begin_commit(&mut self, plan: &RouteMigrationPlan) -> Result<(), String> {
        self.validate_for(plan)?;
        if self.stage != RouteMigrationExecutionStage::Staging
            || self.staged_outputs.len() != plan.faces.len()
        {
            return Err("Route migration cannot enter commit before complete staging.".to_owned());
        }
        self.stage = RouteMigrationExecutionStage::CommitPending;
        self.validate_for(plan)
    }

    pub fn next_commit_ordinal(&self, plan: &RouteMigrationPlan) -> Result<Option<usize>, String> {
        self.validate_for(plan)?;
        if self.stage != RouteMigrationExecutionStage::CommitPending {
            return Ok(None);
        }
        Ok((self.committed_outputs.len() < plan.faces.len())
            .then_some(self.committed_outputs.len()))
    }

    pub fn record_committed(
        &mut self,
        plan: &RouteMigrationPlan,
        backup_path: PathBuf,
    ) -> Result<usize, String> {
        self.validate_for(plan)?;
        if self.stage != RouteMigrationExecutionStage::CommitPending {
            return Err("Route migration is not at its output commit stage.".to_owned());
        }
        let ordinal = self.committed_outputs.len();
        let planned = plan
            .faces
            .get(ordinal)
            .ok_or_else(|| "All route migration outputs are already committed.".to_owned())?;
        let staged = self
            .staged_outputs
            .get(ordinal)
            .ok_or_else(|| "Route migration commit has no staged output for this Face.".to_owned())?;
        self.committed_outputs.push(CommittedRouteMigrationOutput {
            ordinal,
            final_path: planned.replacement.output_tiff_path.clone(),
            backup_path,
            previous_sha256: planned.previous_output_sha256.clone(),
            new_sha256: staged.sha256.clone(),
            converted_at_unix_ms: staged.converted_at_unix_ms,
        });
        if let Err(error) = self.validate_for(plan) {
            self.committed_outputs.pop();
            return Err(error);
        }
        Ok(ordinal)
    }

    pub fn mark_project_save_pending(
        &mut self,
        plan: &RouteMigrationPlan,
    ) -> Result<(), String> {
        self.validate_for(plan)?;
        if self.stage != RouteMigrationExecutionStage::CommitPending
            || self.committed_outputs.len() != plan.faces.len()
        {
            return Err(
                "Production project migration cannot be saved before every TIFF is committed."
                    .to_owned(),
            );
        }
        self.stage = RouteMigrationExecutionStage::ProductionProjectSavePending;
        self.validate_for(plan)
    }

    pub fn mark_complete(&mut self, plan: &RouteMigrationPlan) -> Result<(), String> {
        self.validate_for(plan)?;
        if self.stage != RouteMigrationExecutionStage::ProductionProjectSavePending {
            return Err(
                "Route migration cannot complete before the Production project save boundary."
                    .to_owned(),
            );
        }
        self.stage = RouteMigrationExecutionStage::Complete;
        self.validate_for(plan)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteMigrationFileObservation {
    pub final_sha256: Option<String>,
    pub staged_sha256: Option<String>,
    pub backup_sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteMigrationRecoveryAction {
    Restage,
    ReadyToCommit,
    FinishInterruptedSwap,
    RecordAlreadyCommitted,
    AlreadyCheckpointed,
}

/// Classify one Face after restart/crash without guessing from filenames.
///
/// The caller supplies hashes observed at the exact final/staged/backup paths from the persisted
/// plan/checkpoint. Any unrecognized combination fails closed.
pub fn classify_route_migration_recovery(
    plan: &RouteMigrationPlan,
    checkpoint: &RouteMigrationCheckpoint,
    ordinal: usize,
    observation: &RouteMigrationFileObservation,
) -> Result<RouteMigrationRecoveryAction, String> {
    checkpoint.validate_for(plan)?;
    let face = plan
        .faces
        .get(ordinal)
        .ok_or_else(|| "Route migration recovery ordinal is outside the immutable plan.".to_owned())?;
    let old = face.previous_output_sha256.as_str();
    let staged = checkpoint.staged_outputs.get(ordinal);
    let new = staged.map(|value| value.sha256.as_str());
    let committed = checkpoint.committed_outputs.get(ordinal);

    let final_is_old = hash_matches(observation.final_sha256.as_deref(), old);
    let final_is_new = new.is_some_and(|hash| hash_matches(observation.final_sha256.as_deref(), hash));
    let stage_is_new = new.is_some_and(|hash| hash_matches(observation.staged_sha256.as_deref(), hash));
    let backup_is_old = hash_matches(observation.backup_sha256.as_deref(), old);

    if committed.is_some() {
        if final_is_new && backup_is_old && observation.staged_sha256.is_none() {
            return Ok(RouteMigrationRecoveryAction::AlreadyCheckpointed);
        }
        return Err(
            "Committed route migration checkpoint does not match final/backup bytes; automatic recovery is blocked."
                .to_owned(),
        );
    }

    let Some(_) = staged else {
        if final_is_old && observation.staged_sha256.is_none() && observation.backup_sha256.is_none() {
            return Ok(RouteMigrationRecoveryAction::Restage);
        }
        return Err(
            "Unstaged route migration Face no longer has its exact previous output state."
                .to_owned(),
        );
    };

    if final_is_old && stage_is_new && observation.backup_sha256.is_none() {
        return Ok(RouteMigrationRecoveryAction::ReadyToCommit);
    }
    if observation.final_sha256.is_none() && stage_is_new && backup_is_old {
        return Ok(RouteMigrationRecoveryAction::FinishInterruptedSwap);
    }
    if final_is_new && observation.staged_sha256.is_none() && backup_is_old {
        return Ok(RouteMigrationRecoveryAction::RecordAlreadyCommitted);
    }
    if final_is_old && observation.staged_sha256.is_none() && observation.backup_sha256.is_none() {
        return Ok(RouteMigrationRecoveryAction::Restage);
    }

    Err(
        "Route migration output bytes do not match any safe recoverable old/staged/committed state."
            .to_owned(),
    )
}

fn hash_matches(observed: Option<&str>, expected: &str) -> bool {
    observed.is_some_and(|value| value.trim().eq_ignore_ascii_case(expected.trim()))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn paths_match(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .trim()
        .replace('/', "\\")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
        ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
        TargetChannelDefinition,
    };
    use crate::conversion_recipe::recipe_sha256;
    use crate::conversion_route_migration::{RouteMigrationFacePlan, RouteMigrationPlan};
    use crate::conversion_transaction::{
        CapturedOutputPolicy, CapturedSourceProfile, ConversionJobCapture,
    };
    use crate::model::{IccProfileIdentity, ShadeProject};
    use crate::production_project_compat::ProductionCompatibilityKey;
    use crate::production_project_disposition::{
        CapturedRouteFaceOwnership, RouteMigrationCapture,
    };

    fn hash(character: char) -> String {
        assert!(character.is_ascii());
        format!("{:02x}", character as u8).repeat(32)
    }

    fn recipe(source_hash: char) -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: hash(source_hash),
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
                    sha256: hash('p'),
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

    fn compatibility() -> ProductionCompatibilityKey {
        ProductionCompatibilityKey {
            engine_mode: ConversionEngineMode::Icc,
            output_profile_sha256: Some(hash('p')),
            device_link_sha256: None,
            characterization_id: None,
            channel_names: vec![
                "Cyan".to_owned(),
                "Magenta".to_owned(),
                "Yellow".to_owned(),
                "Black".to_owned(),
            ],
            bit_depth: 16,
        }
    }

    fn plan() -> RouteMigrationPlan {
        let old_recipe = recipe('a');
        let new_recipe = recipe('b');
        let old_recipe_sha = recipe_sha256(&old_recipe).unwrap();
        let new_recipe_sha = recipe_sha256(&new_recipe).unwrap();
        let replacement = ConversionJobCapture::capture(
            &ShadeProject::default(),
            PathBuf::from(r"C:\Design\Source.shade"),
            hash('s'),
            PathBuf::from(r"C:\Design\Face.tif"),
            Some(1),
            hash('f'),
            CapturedSourceProfile::Embedded,
            new_recipe,
            CapturedOutputPolicy::TransactionalReplace,
            PathBuf::from(r"C:\Production\Face.tif"),
            PathBuf::from(r"C:\Production\Job.shade"),
            "Production".to_owned(),
            "Face".to_owned(),
        )
        .unwrap();
        let policy = crate::conversion_batch::batch_recipe_policy_sha256(&old_recipe).unwrap();
        let intent = RouteMigrationCapture::capture(
            hash('j'),
            &compatibility(),
            &compatibility(),
            policy.clone(),
            policy,
            vec![CapturedRouteFaceOwnership {
                source_face_path: r"C:\Design\Face.tif".to_owned(),
                output_path: r"C:\Production\Face.tif".to_owned(),
                previous_recipe_sha256: old_recipe_sha.clone(),
            }],
            0,
            1,
            true,
            false,
        )
        .unwrap();
        RouteMigrationPlan {
            intent,
            source_project_path: PathBuf::from(r"C:\Design\Source.shade"),
            production_project_path: PathBuf::from(r"C:\Production\Job.shade"),
            faces: vec![RouteMigrationFacePlan {
                production_face_index: 0,
                replacement,
                previous_output_sha256: hash('o'),
                previous_recipe_sha256: old_recipe_sha,
                new_recipe_sha256: new_recipe_sha,
            }],
            requires_production_work_discard: false,
        }
    }

    #[test]
    fn checkpoint_requires_complete_staging_before_commit() {
        let plan = plan();
        let mut checkpoint = RouteMigrationCheckpoint::default();
        assert!(checkpoint.begin_commit(&plan).is_err());
        checkpoint
            .record_staged(
                &plan,
                PathBuf::from(r"C:\Production\.shade-migration\Face.tif"),
                hash('n'),
                123,
            )
            .unwrap();
        checkpoint.begin_commit(&plan).unwrap();
        assert_eq!(checkpoint.stage, RouteMigrationExecutionStage::CommitPending);
    }

    #[test]
    fn committed_prefix_cannot_advance_without_unique_backup() {
        let plan = plan();
        let mut checkpoint = RouteMigrationCheckpoint::default();
        checkpoint
            .record_staged(
                &plan,
                PathBuf::from(r"C:\Production\.shade-migration\Face.tif"),
                hash('n'),
                123,
            )
            .unwrap();
        checkpoint.begin_commit(&plan).unwrap();
        checkpoint
            .record_committed(
                &plan,
                PathBuf::from(r"C:\Production\.shade-migration\Face.old.tif"),
            )
            .unwrap();
        checkpoint.mark_project_save_pending(&plan).unwrap();
        assert_eq!(
            checkpoint.stage,
            RouteMigrationExecutionStage::ProductionProjectSavePending
        );
    }

    #[test]
    fn recovery_recognizes_crash_between_backup_and_final_swap() {
        let plan = plan();
        let mut checkpoint = RouteMigrationCheckpoint::default();
        checkpoint
            .record_staged(
                &plan,
                PathBuf::from(r"C:\Production\.shade-migration\Face.tif"),
                hash('n'),
                123,
            )
            .unwrap();
        checkpoint.begin_commit(&plan).unwrap();
        let action = classify_route_migration_recovery(
            &plan,
            &checkpoint,
            0,
            &RouteMigrationFileObservation {
                final_sha256: None,
                staged_sha256: Some(hash('n')),
                backup_sha256: Some(hash('o')),
            },
        )
        .unwrap();
        assert_eq!(action, RouteMigrationRecoveryAction::FinishInterruptedSwap);
    }

    #[test]
    fn recovery_recognizes_uncheckpointed_successful_swap() {
        let plan = plan();
        let mut checkpoint = RouteMigrationCheckpoint::default();
        checkpoint
            .record_staged(
                &plan,
                PathBuf::from(r"C:\Production\.shade-migration\Face.tif"),
                hash('n'),
                123,
            )
            .unwrap();
        checkpoint.begin_commit(&plan).unwrap();
        let action = classify_route_migration_recovery(
            &plan,
            &checkpoint,
            0,
            &RouteMigrationFileObservation {
                final_sha256: Some(hash('n')),
                staged_sha256: None,
                backup_sha256: Some(hash('o')),
            },
        )
        .unwrap();
        assert_eq!(action, RouteMigrationRecoveryAction::RecordAlreadyCommitted);
    }

    #[test]
    fn recovery_blocks_unknown_bytes_instead_of_guessing() {
        let plan = plan();
        let mut checkpoint = RouteMigrationCheckpoint::default();
        checkpoint
            .record_staged(
                &plan,
                PathBuf::from(r"C:\Production\.shade-migration\Face.tif"),
                hash('n'),
                123,
            )
            .unwrap();
        checkpoint.begin_commit(&plan).unwrap();
        let error = classify_route_migration_recovery(
            &plan,
            &checkpoint,
            0,
            &RouteMigrationFileObservation {
                final_sha256: Some(hash('x')),
                staged_sha256: Some(hash('n')),
                backup_sha256: None,
            },
        )
        .expect_err("unknown final bytes must fail closed");
        assert!(error.contains("do not match any safe recoverable"));
    }
}
