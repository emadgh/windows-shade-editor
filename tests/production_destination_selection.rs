#![cfg(windows)]

use std::path::PathBuf;

use windows_shade_editor::color_conversion::{
    CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
    ConversionRenderingIntent, ConversionTargetDefinition, SeparationStrategy,
    TargetChannelDefinition,
};
use windows_shade_editor::model::IccProfileIdentity;
use windows_shade_editor::production_destination::{
    ProductionDestinationAvailability, ProductionDestinationCandidate, compatibility_for_recipe,
};
use windows_shade_editor::production_destination_selection::FrozenProductionDestination;
use windows_shade_editor::production_project_disposition::ProductionProjectDisposition;

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
            sha256: hash('1'),
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
                sha256: hash('2'),
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

#[test]
fn create_new_destination_freezes_only_a_shade_project_path() {
    let path = PathBuf::from(r"C:\Production\Job.shade");
    let frozen = FrozenProductionDestination::create_new(path.clone()).unwrap();

    assert_eq!(frozen.production_project_path, path);
    assert_eq!(frozen.disposition, ProductionProjectDisposition::CreateNew);
    assert!(FrozenProductionDestination::create_new(PathBuf::from(r"C:\Production\Job.tif")).is_err());
}

#[test]
fn append_existing_public_api_freezes_exact_project_baseline() {
    let recipe = recipe();
    let compatibility = compatibility_for_recipe(&recipe).unwrap();
    let candidate = ProductionDestinationCandidate {
        path: PathBuf::from(r"C:\Production\Existing.shade"),
        availability: ProductionDestinationAvailability::Ready,
        project_name: Some("Existing Production".to_owned()),
        face_count: Some(3),
        compatibility: Some(compatibility.clone()),
        project_sha256: Some(hash('a')),
        baseline_recipe: Some(recipe.clone()),
        diagnostic: None,
    };

    let frozen = FrozenProductionDestination::append_existing(&candidate, &recipe).unwrap();
    assert_eq!(frozen.production_project_path, candidate.path);

    let ProductionProjectDisposition::AppendExisting {
        expected_project_sha256,
        expected_compatibility,
    } = frozen.disposition
    else {
        panic!("existing Production destination must freeze AppendExisting");
    };

    assert_eq!(expected_project_sha256, hash('a'));
    assert!(expected_compatibility.matches_runtime(&compatibility));
}
