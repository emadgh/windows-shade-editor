use std::path::{Path, PathBuf};

use crate::color_conversion::ConversionRecipe;
use crate::production_destination::ProductionDestinationCandidate;
use crate::production_project_disposition::ProductionProjectDisposition;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenProductionDestination {
    pub production_project_path: PathBuf,
    pub disposition: ProductionProjectDisposition,
}

impl FrozenProductionDestination {
    pub fn create_new(production_project_path: PathBuf) -> Result<Self, String> {
        validate_project_path(&production_project_path)?;
        Ok(Self {
            production_project_path,
            disposition: ProductionProjectDisposition::CreateNew,
        })
    }

    /// Freeze one exact existing Production destination. The selected project
    /// path, SHA-256 and compatibility key are captured together so later UI
    /// changes or filesystem state cannot silently alter queue intent.
    pub fn append_existing(
        candidate: &ProductionDestinationCandidate,
        recipe: &ConversionRecipe,
    ) -> Result<Self, String> {
        if !candidate.can_append() {
            return Err(
                candidate
                    .diagnostic
                    .clone()
                    .unwrap_or_else(|| "Selected Production project is not append-compatible.".to_owned()),
            );
        }
        if !candidate.matches_recipe(recipe)? {
            return Err(
                "Current target setup no longer matches the selected Production project. Restore the Production target recipe or choose Create New."
                    .to_owned(),
            );
        }
        validate_project_path(&candidate.path)?;
        let project_sha256 = candidate
            .project_sha256
            .as_ref()
            .ok_or_else(|| "Selected Production project has no stable SHA-256 capture.".to_owned())?;
        let compatibility = candidate
            .compatibility
            .as_ref()
            .ok_or_else(|| "Selected Production project has no validated target compatibility.".to_owned())?;
        let disposition = ProductionProjectDisposition::append_existing(
            project_sha256.clone(),
            compatibility,
        )?;
        Ok(Self {
            production_project_path: candidate.path.clone(),
            disposition,
        })
    }
}

fn validate_project_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("shade"))
    {
        return Err("Production project destination must be a .shade path.".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRenderingIntent,
        ConversionTargetDefinition, SeparationStrategy, TargetChannelDefinition,
    };
    use crate::model::IccProfileIdentity;
    use crate::production_destination::{
        ProductionDestinationAvailability, compatibility_for_recipe,
    };

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn recipe() -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: hash('s'),
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

    fn candidate() -> ProductionDestinationCandidate {
        let recipe = recipe();
        ProductionDestinationCandidate {
            path: PathBuf::from(r"C:\Production\Job.shade"),
            availability: ProductionDestinationAvailability::Ready,
            project_name: Some("Production".to_owned()),
            face_count: Some(4),
            compatibility: Some(compatibility_for_recipe(&recipe).unwrap()),
            project_sha256: Some(hash('a')),
            baseline_recipe: Some(recipe),
            diagnostic: None,
        }
    }

    #[test]
    fn append_freezes_exact_sha_and_compatibility() {
        let candidate = candidate();
        let frozen = FrozenProductionDestination::append_existing(&candidate, &recipe()).unwrap();
        assert_eq!(frozen.production_project_path, candidate.path);
        let ProductionProjectDisposition::AppendExisting {
            expected_project_sha256,
            expected_compatibility,
        } = frozen.disposition else {
            panic!("append destination must freeze AppendExisting disposition");
        };
        assert_eq!(expected_project_sha256, hash('a'));
        assert_eq!(expected_compatibility.channel_names[0], "Cyan");
    }

    #[test]
    fn target_edit_after_selection_blocks_append() {
        let candidate = candidate();
        let mut changed = recipe();
        changed.target.bit_depth = 8;
        let error = FrozenProductionDestination::append_existing(&candidate, &changed)
            .expect_err("target drift must fail closed");
        assert!(error.contains("no longer matches"));
    }

    #[test]
    fn create_new_preserves_current_explicit_project_path() {
        let path = PathBuf::from(r"C:\Production\NewJob.shade");
        let frozen = FrozenProductionDestination::create_new(path.clone()).unwrap();
        assert_eq!(frozen.production_project_path, path);
        assert_eq!(frozen.disposition, ProductionProjectDisposition::CreateNew);
    }
}