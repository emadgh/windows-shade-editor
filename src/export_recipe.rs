use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::{ChannelAdjustment, ShadeProject, TestCodeConfig};

/// Immutable subset of project state required by TIFF export. Queue entries do
/// not retain thumbnails, snapshot histories, metadata caches or preview ICC state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportRecipe {
    pub adjustments: BTreeMap<String, ChannelAdjustment>,
    pub test_code: TestCodeConfig,
}

impl ExportRecipe {
    /// Normal Face/Export All recipe. Test-code drawing is deliberately disabled:
    /// coded test output belongs exclusively to the Snapshot export workflow.
    pub fn from_project(project: &ShadeProject) -> Self {
        let mut test_code = project.test_code.clone();
        test_code.enabled = false;
        Self {
            adjustments: project.adjustments.clone(),
            test_code,
        }
    }

    /// Snapshot/test export recipe. This is the only export recipe constructor
    /// that preserves enabled Test Code drawing.
    pub fn from_snapshot_project(project: &ShadeProject) -> Self {
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

    /// Exact code frozen into this queued export. Empty means this recipe is uncoded.
    pub fn exported_test_code(&self) -> String {
        if self.test_code.enabled {
            self.test_code.text.trim().to_owned()
        } else {
            String::new()
        }
    }

    /// Stable identity of the exact adjustment payload consumed by the exporter.
    pub fn adjustment_sha256(&self) -> String {
        let bytes = serde_json::to_vec(&self.adjustments).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
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
    fn normal_recipe_never_writes_test_code() {
        let mut project = ShadeProject::default();
        project.test_code.enabled = true;
        project.test_code.text = "TEST-42".to_owned();
        let recipe = ExportRecipe::from_project(&project);
        assert!(!recipe.test_code.enabled);
        assert_eq!(recipe.test_code.text, "TEST-42");
        assert!(recipe.exported_test_code().is_empty());
    }

    #[test]
    fn snapshot_recipe_freezes_effective_test_code() {
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
        let recipe = ExportRecipe::from_snapshot_project(&project);
        assert!(recipe.test_code.enabled);
        assert_eq!(recipe.test_code.text, expected);
        assert_eq!(recipe.exported_test_code(), expected);
        assert_eq!(recipe.adjustment_sha256().len(), 64);
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
