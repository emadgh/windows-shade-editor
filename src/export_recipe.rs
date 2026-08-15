use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{ChannelAdjustment, ShadeProject, TestCodeConfig};

/// Immutable subset of project state required by TIFF export. Queue entries do
/// not retain thumbnails, snapshot histories, metadata caches or preview ICC state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportRecipe {
    pub adjustments: BTreeMap<String, ChannelAdjustment>,
    pub test_code: TestCodeConfig,
}

impl ExportRecipe {
    pub fn from_project(project: &ShadeProject) -> Self {
        let mut test_code = project.test_code.clone();
        // Freeze the effective fallback text at enqueue time. Otherwise a recipe
        // detached from the full snapshot collection could change meaning later.
        if test_code.enabled && test_code.text.trim().is_empty() {
            test_code.text = project.effective_test_code_text();
        }
        Self {
            adjustments: project.adjustments.clone(),
            test_code,
        }
    }

    pub fn materialize_project(&self) -> ShadeProject {
        let mut project = ShadeProject::default();
        project.adjustments = self.adjustments.clone();
        project.test_code = self.test_code.clone();
        project
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MASTER_ADJUSTMENT_KEY;

    #[test]
    fn recipe_excludes_heavy_project_state_and_freezes_test_code() {
        let mut project = ShadeProject::default();
        project.test_code.enabled = true;
        project.create_snapshot();
        let expected = project.active_snapshot_name().unwrap().to_owned();
        project.thumbnail = Some(crate::model::ProjectThumbnail {
            mime_type: "image/png".to_owned(),
            thumbnail_version: 1,
            width: 1,
            height: 1,
            encoded_bytes: 4,
            data_base64: "AAAA".to_owned(),
        });
        project
            .adjustments
            .entry(MASTER_ADJUSTMENT_KEY.to_owned())
            .or_default()
            .levels
            .gamma = 1.25;
        let recipe = ExportRecipe::from_project(&project);
        assert_eq!(recipe.test_code.text, expected);
        assert_eq!(
            recipe
                .adjustments
                .get(MASTER_ADJUSTMENT_KEY)
                .unwrap()
                .levels
                .gamma,
            1.25
        );
        let materialized = recipe.materialize_project();
        assert!(materialized.thumbnail.is_none());
        assert!(materialized.snapshots.is_empty());
    }
}
