use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::color_conversion::{
    ConversionEngineMode, ConversionRecipe, ConversionSourceRef, ProductionProvenance, ProjectRole,
};
use crate::conversion_audit::{ConversionAuditFinding, ConversionAuditRecord};
use crate::conversion_output::validate_conversion_output_path;
use crate::conversion_recipe::recipe_sha256;
use crate::custom_optimizer_evidence::CapturedCustomOptimizerEvidence;
use crate::export_recipe::ExportRecipe;
use crate::model::ShadeProject;
use crate::production_project::{
    ProductionProjectSpec, build_production_project_with_audit,
};
use crate::tiff_output;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversionPhase {
    CaptureValidation,
    Decode,
    SourceAdjustments,
    ColorConversion,
    MetadataGeneration,
    OutputWrite,
    OutputValidation,
    OutputCommit,
    ProductionProjectSave,
    Complete,
}

impl ConversionPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::CaptureValidation => "Validating captured conversion job",
            Self::Decode => "Decoding source",
            Self::SourceAdjustments => "Rendering saved source adjustments",
            Self::ColorConversion => "Converting production color",
            Self::MetadataGeneration => "Generating production metadata",
            Self::OutputWrite => "Writing staged production TIFF",
            Self::OutputValidation => "Validating staged production TIFF",
            Self::OutputCommit => "Committing production TIFF atomically",
            Self::ProductionProjectSave => "Saving Production project",
            Self::Complete => "Production conversion complete",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConversionProgress {
    pub phase: ConversionPhase,
    pub fraction: f32,
    pub detail: String,
}

impl ConversionProgress {
    pub fn new(phase: ConversionPhase, fraction: f32, detail: impl Into<String>) -> Self {
        Self {
            phase,
            fraction: if fraction.is_finite() {
                fraction.clamp(0.0, 1.0)
            } else {
                0.0
            },
            detail: detail.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapturedSourceProfile {
    /// Re-open and hash the ICC payload embedded in the captured source TIFF.
    Embedded,
    /// Re-open and hash an explicitly assigned external production Source ICC.
    External { path: PathBuf },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapturedSourceFormat {
    Tiff,
    Png,
    Jpeg,
}

impl CapturedSourceFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tiff => "TIFF",
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapturedSourceColorModel {
    Gray,
    Rgb,
    Cmyk,
    Multichannel,
}

impl CapturedSourceColorModel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Gray => "Gray",
            Self::Rgb => "RGB",
            Self::Cmyk => "CMYK",
            Self::Multichannel => "Multichannel",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapturedSourceRasterFacts {
    pub format: CapturedSourceFormat,
    pub color_model: CapturedSourceColorModel,
    pub bit_depth: u8,
    pub channel_count: usize,
}

impl CapturedSourceRasterFacts {
    pub fn new(
        format: CapturedSourceFormat,
        color_model: CapturedSourceColorModel,
        bit_depth: u8,
        channel_count: usize,
    ) -> Self {
        Self {
            format,
            color_model,
            bit_depth,
            channel_count,
        }
    }

    pub fn validate(self) -> Result<(), String> {
        if self.bit_depth == 0 {
            return Err("Captured source raster bit depth must be non-zero.".to_owned());
        }
        if self.channel_count == 0 {
            return Err("Captured source raster channel count must be non-zero.".to_owned());
        }
        let expected_channels = match self.color_model {
            CapturedSourceColorModel::Gray => Some(1),
            CapturedSourceColorModel::Rgb => Some(3),
            CapturedSourceColorModel::Cmyk => Some(4),
            CapturedSourceColorModel::Multichannel => None,
        };
        if expected_channels.is_some_and(|expected| expected != self.channel_count) {
            return Err(format!(
                "Captured source raster {} model requires {} channel(s); found {}.",
                self.color_model.label(),
                expected_channels.unwrap(),
                self.channel_count
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapturedOutputPolicy {
    MustNotExist,
    TransactionalReplace,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversionJobCapture {
    pub source_project_path: PathBuf,
    pub source_project_file_sha256: String,
    pub source_face_path: PathBuf,
    pub source_snapshot_id: Option<u64>,
    pub source_file_sha256: String,
    pub source_profile: CapturedSourceProfile,
    /// Format-neutral raster facts frozen from the exact Source descriptor at
    /// queue-capture time. Legacy persisted jobs deserialize this as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_raster: Option<CapturedSourceRasterFacts>,
    pub source_recipe: ExportRecipe,
    pub conversion_recipe: ConversionRecipe,
    pub conversion_recipe_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_optimizer_evidence: Option<CapturedCustomOptimizerEvidence>,
    /// Non-blocking findings frozen at queue-capture time. Legacy persisted jobs
    /// deserialize this as empty; new unified conversion jobs populate it from
    /// the typed production preflight report rather than current UI state.
    #[serde(default)]
    pub audit_findings: Vec<ConversionAuditFinding>,
    pub output_policy: CapturedOutputPolicy,
    pub output_tiff_path: PathBuf,
    pub production_project_path: PathBuf,
    pub production_project_name: String,
    pub output_face_label: String,
}

impl ConversionJobCapture {
    pub fn capture(
        source_project: &ShadeProject,
        source_project_path: PathBuf,
        source_project_file_sha256: String,
        source_face_path: PathBuf,
        source_snapshot_id: Option<u64>,
        source_file_sha256: String,
        source_profile: CapturedSourceProfile,
        conversion_recipe: ConversionRecipe,
        output_policy: CapturedOutputPolicy,
        output_tiff_path: PathBuf,
        production_project_path: PathBuf,
        production_project_name: String,
        output_face_label: String,
    ) -> Result<Self, String> {
        let conversion_recipe_sha256 = recipe_sha256(&conversion_recipe)?;
        let output_tiff_path = tiff_output::canonical_destination(&output_tiff_path);
        let capture = Self {
            source_project_path,
            source_project_file_sha256,
            source_face_path,
            source_snapshot_id,
            source_file_sha256,
            source_profile,
            source_raster: None,
            source_recipe: ExportRecipe::from_project(source_project),
            conversion_recipe,
            conversion_recipe_sha256,
            custom_optimizer_evidence: None,
            audit_findings: Vec::new(),
            output_policy,
            output_tiff_path,
            production_project_path,
            production_project_name,
            output_face_label,
        };
        capture.validate()?;
        Ok(capture)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn capture_custom_optimizer(
        source_project: &ShadeProject,
        source_project_path: PathBuf,
        source_project_file_sha256: String,
        source_face_path: PathBuf,
        source_snapshot_id: Option<u64>,
        source_file_sha256: String,
        source_profile: CapturedSourceProfile,
        conversion_recipe: ConversionRecipe,
        custom_optimizer_evidence: CapturedCustomOptimizerEvidence,
        output_policy: CapturedOutputPolicy,
        output_tiff_path: PathBuf,
        production_project_path: PathBuf,
        production_project_name: String,
        output_face_label: String,
    ) -> Result<Self, String> {
        let conversion_recipe_sha256 = recipe_sha256(&conversion_recipe)?;
        let output_tiff_path = tiff_output::canonical_destination(&output_tiff_path);
        let capture = Self {
            source_project_path,
            source_project_file_sha256,
            source_face_path,
            source_snapshot_id,
            source_file_sha256,
            source_profile,
            source_raster: None,
            source_recipe: ExportRecipe::from_project(source_project),
            conversion_recipe,
            conversion_recipe_sha256,
            custom_optimizer_evidence: Some(custom_optimizer_evidence),
            audit_findings: Vec::new(),
            output_policy,
            output_tiff_path,
            production_project_path,
            production_project_name,
            output_face_label,
        };
        capture.validate()?;
        Ok(capture)
    }

    pub fn with_source_raster_facts(
        mut self,
        source_raster: CapturedSourceRasterFacts,
    ) -> Result<Self, String> {
        source_raster.validate()?;
        self.source_raster = Some(source_raster);
        self.validate()?;
        Ok(self)
    }

    pub fn with_audit_findings(
        mut self,
        findings: Vec<ConversionAuditFinding>,
    ) -> Result<Self, String> {
        self.audit_findings = findings;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.source_project_path.as_os_str().is_empty()
            || self.source_face_path.as_os_str().is_empty()
        {
            return Err(
                "Conversion capture requires saved Source project and Face paths.".to_owned(),
            );
        }
        if !has_sha256(&self.source_project_file_sha256) {
            return Err("Conversion capture requires a full Source-project SHA-256.".to_owned());
        }
        if !has_sha256(&self.source_file_sha256) {
            return Err("Conversion capture requires a full source-file SHA-256.".to_owned());
        }
        if matches!(
            &self.source_profile,
            CapturedSourceProfile::External { path } if path.as_os_str().is_empty()
        ) {
            return Err("Assigned production Source ICC path cannot be empty.".to_owned());
        }
        if let Some(source_raster) = self.source_raster {
            source_raster.validate()?;
        }
        self.conversion_recipe.validate().map_err(|errors| {
            format!("Invalid captured conversion recipe: {}", errors.join(" "))
        })?;
        let actual_recipe_sha256 = recipe_sha256(&self.conversion_recipe)?;
        if !actual_recipe_sha256.eq_ignore_ascii_case(self.conversion_recipe_sha256.trim()) {
            return Err(
                "Captured conversion recipe SHA-256 does not match its payload.".to_owned(),
            );
        }
        match (
            self.conversion_recipe.engine_mode,
            self.custom_optimizer_evidence.as_ref(),
        ) {
            (ConversionEngineMode::CustomOptimizer, Some(evidence)) => {
                evidence.validate().map_err(|errors| {
                    format!(
                        "Invalid captured Custom Optimizer evidence: {}",
                        errors.join(" ")
                    )
                })?;
            }
            (ConversionEngineMode::CustomOptimizer, None) => {
                return Err(
                    "Custom Optimizer conversion capture requires immutable production evidence."
                        .to_owned(),
                );
            }
            (_, Some(_)) => {
                return Err(
                    "ICC/DeviceLink conversion capture cannot carry Custom Optimizer evidence."
                        .to_owned(),
                );
            }
            (_, None) => {}
        }
        for finding in &self.audit_findings {
            if finding.code.trim().is_empty() || finding.message.trim().is_empty() {
                return Err(
                    "Captured conversion audit findings require non-empty code and message."
                        .to_owned(),
                );
            }
        }
        validate_conversion_output_path(&self.source_face_path, &self.output_tiff_path)
            .map_err(|err| err.to_string())?;
        if self.production_project_path.as_os_str().is_empty()
            || !self
                .production_project_path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("shade"))
        {
            return Err("Production project destination must use .shade.".to_owned());
        }
        if paths_match(&self.output_tiff_path, &self.production_project_path) {
            return Err(
                "Production TIFF and Production project paths must be distinct.".to_owned(),
            );
        }
        if self.production_project_name.trim().is_empty()
            || self.output_face_label.trim().is_empty()
        {
            return Err("Production project and Face labels cannot be empty.".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct ConversionCancellation {
    requested: Arc<AtomicBool>,
}

impl ConversionCancellation {
    pub fn request(&self) {
        self.requested.store(true, Ordering::Release);
    }

    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    pub fn check_before_commit(&self) -> Result<(), String> {
        if self.is_requested() {
            Err("Production conversion cancelled before output commit.".to_owned())
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommittedConversionOutput {
    pub path: PathBuf,
    pub sha256: String,
    pub converted_at_unix_ms: i64,
}

pub trait ConversionTransactionBackend {
    /// Render, convert, stage, validate and atomically commit the TIFF. This
    /// method must honor cancellation until its atomic commit point and return
    /// only after the final destination exists and is durable.
    fn render_convert_and_commit(
        &mut self,
        capture: &ConversionJobCapture,
        cancellation: &ConversionCancellation,
        report: &mut dyn FnMut(ConversionProgress),
    ) -> Result<CommittedConversionOutput, String>;

    /// Persist the already-built clean Production project atomically.
    fn save_production_project(
        &mut self,
        path: &Path,
        project: &ShadeProject,
    ) -> Result<(), String>;
}

#[derive(Clone, Debug)]
pub struct CompletedConversionTransaction {
    pub committed_output: CommittedConversionOutput,
    pub production_project_path: PathBuf,
    pub production_project: ShadeProject,
}

#[derive(Clone, Debug)]
pub enum ConversionTransactionOutcome {
    Completed(CompletedConversionTransaction),
    CancelledBeforeCommit {
        phase: ConversionPhase,
        message: String,
    },
    FailedBeforeCommit {
        phase: ConversionPhase,
        error: String,
    },
    OutputCommittedNeedsRecovery {
        committed_output: CommittedConversionOutput,
        production_project_path: PathBuf,
        production_project: Option<ShadeProject>,
        error: String,
    },
}

pub fn run_conversion_transaction<B, F>(
    capture: &ConversionJobCapture,
    cancellation: &ConversionCancellation,
    backend: &mut B,
    mut report: F,
) -> ConversionTransactionOutcome
where
    B: ConversionTransactionBackend,
    F: FnMut(ConversionProgress),
{
    report(ConversionProgress::new(
        ConversionPhase::CaptureValidation,
        0.0,
        ConversionPhase::CaptureValidation.label(),
    ));
    if let Err(error) = capture.validate() {
        return ConversionTransactionOutcome::FailedBeforeCommit {
            phase: ConversionPhase::CaptureValidation,
            error,
        };
    }
    if let Err(message) = cancellation.check_before_commit() {
        return ConversionTransactionOutcome::CancelledBeforeCommit {
            phase: ConversionPhase::CaptureValidation,
            message,
        };
    }

    let mut active_phase = ConversionPhase::Decode;
    let mut backend_report = |progress: ConversionProgress| {
        active_phase = progress.phase;
        report(progress);
    };
    let backend_result =
        backend.render_convert_and_commit(capture, cancellation, &mut backend_report);
    drop(backend_report);
    let committed_output = match backend_result {
        Ok(output) => output,
        Err(error) if cancellation.is_requested() => {
            return ConversionTransactionOutcome::CancelledBeforeCommit {
                phase: active_phase,
                message: error,
            };
        }
        Err(error) => {
            return ConversionTransactionOutcome::FailedBeforeCommit {
                phase: active_phase,
                error,
            };
        }
    };

    if !paths_match(&committed_output.path, &capture.output_tiff_path)
        || !has_sha256(&committed_output.sha256)
    {
        return ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
            committed_output,
            production_project_path: capture.production_project_path.clone(),
            production_project: None,
            error:
                "Committed conversion output identity is invalid; Production project was not saved."
                    .to_owned(),
        };
    }
    report(ConversionProgress::new(
        ConversionPhase::OutputCommit,
        0.92,
        ConversionPhase::OutputCommit.label(),
    ));

    let custom_optimizer = match capture.custom_optimizer_evidence.as_ref() {
        Some(evidence) => match evidence.production_provenance(&capture.conversion_recipe_sha256) {
            Ok(provenance) => Some(provenance),
            Err(errors) => {
                return ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
                    committed_output,
                    production_project_path: capture.production_project_path.clone(),
                    production_project: None,
                    error: format!(
                        "Cannot persist Custom Optimizer production provenance: {}",
                        errors.join(" ")
                    ),
                };
            }
        },
        None => None,
    };

    let provenance = ProductionProvenance {
        source: ConversionSourceRef {
            source_project_path: capture.source_project_path.display().to_string(),
            source_face_path: capture.source_face_path.display().to_string(),
            source_snapshot_id: capture.source_snapshot_id,
            source_file_sha256: capture.source_file_sha256.clone(),
        },
        recipe: capture.conversion_recipe.clone(),
        custom_optimizer,
        output_path: committed_output.path.display().to_string(),
        output_sha256: committed_output.sha256.clone(),
        converted_at_unix_ms: committed_output.converted_at_unix_ms,
    };
    let audit = match ConversionAuditRecord::from_committed_job(
        capture,
        &committed_output,
        capture.audit_findings.clone(),
    ) {
        Ok(audit) => audit,
        Err(error) => {
            return ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
                committed_output,
                production_project_path: capture.production_project_path.clone(),
                production_project: None,
                error: format!("Cannot build committed conversion audit record: {error}"),
            };
        }
    };
    let production_project = match build_production_project_with_audit(
        ProductionProjectSpec {
            project_name: &capture.production_project_name,
            source_project_path: &capture.source_project_path,
            output_tiff_path: &committed_output.path,
            output_face_label: &capture.output_face_label,
            provenance,
        },
        audit,
    ) {
        Ok(project) => project,
        Err(error) => {
            return ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
                committed_output,
                production_project_path: capture.production_project_path.clone(),
                production_project: None,
                error,
            };
        }
    };

    // Cancellation after the TIFF commit point cannot roll the output back. We
    // finish the small project-save boundary so the committed TIFF is linked.
    report(ConversionProgress::new(
        ConversionPhase::ProductionProjectSave,
        0.96,
        ConversionPhase::ProductionProjectSave.label(),
    ));
    if let Err(error) =
        backend.save_production_project(&capture.production_project_path, &production_project)
    {
        return ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
            committed_output,
            production_project_path: capture.production_project_path.clone(),
            production_project: Some(production_project),
            error,
        };
    }
    report(ConversionProgress::new(
        ConversionPhase::Complete,
        1.0,
        ConversionPhase::Complete.label(),
    ));
    ConversionTransactionOutcome::Completed(CompletedConversionTransaction {
        committed_output,
        production_project_path: capture.production_project_path.clone(),
        production_project,
    })
}

pub fn production_link_for_completed(
    completed: &CompletedConversionTransaction,
) -> crate::color_conversion::LinkedProjectRef {
    crate::color_conversion::LinkedProjectRef {
        role: ProjectRole::Production,
        path: completed.production_project_path.display().to_string(),
    }
}

fn has_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRenderingIntent,
        ConversionTargetDefinition, SeparationStrategy, TargetChannelDefinition,
    };
    use crate::model::{ChannelAdjustment, IccProfileIdentity};

    const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn recipe() -> ConversionRecipe {
        ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "sRGB".to_owned(),
                sha256: HASH_A.to_owned(),
            },
            target: ConversionTargetDefinition {
                name: "Press CMYK".to_owned(),
                channels: ["Cyan", "Magenta", "Yellow", "Black"]
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: None,
                    })
                    .to_vec(),
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

    fn capture() -> ConversionJobCapture {
        let mut source = ShadeProject::default();
        source
            .adjustments
            .insert("Red".to_owned(), ChannelAdjustment::default());
        ConversionJobCapture::capture(
            &source,
            PathBuf::from(r"C:\Design\Source.shade"),
            HASH_A.to_owned(),
            PathBuf::from(r"C:\Design\Face.tif"),
            Some(7),
            HASH_A.to_owned(),
            CapturedSourceProfile::Embedded,
            recipe(),
            CapturedOutputPolicy::MustNotExist,
            PathBuf::from(r"C:\Production\Face_CMYK.tif"),
            PathBuf::from(r"C:\Production\Job.shade"),
            "Production Job".to_owned(),
            "Face CMYK".to_owned(),
        )
        .unwrap()
    }

    struct MockBackend {
        cancellation_during_commit: bool,
        commit_error: Option<String>,
        save_error: Option<String>,
        commit_calls: usize,
        save_calls: usize,
    }

    impl MockBackend {
        fn success() -> Self {
            Self {
                cancellation_during_commit: false,
                commit_error: None,
                save_error: None,
                commit_calls: 0,
                save_calls: 0,
            }
        }
    }

    impl ConversionTransactionBackend for MockBackend {
        fn render_convert_and_commit(
            &mut self,
            capture: &ConversionJobCapture,
            cancellation: &ConversionCancellation,
            report: &mut dyn FnMut(ConversionProgress),
        ) -> Result<CommittedConversionOutput, String> {
            self.commit_calls += 1;
            report(ConversionProgress::new(
                ConversionPhase::ColorConversion,
                0.5,
                "mock transform",
            ));
            if self.cancellation_during_commit {
                cancellation.request();
            }
            if let Some(error) = &self.commit_error {
                return Err(error.clone());
            }
            Ok(CommittedConversionOutput {
                path: capture.output_tiff_path.clone(),
                sha256: HASH_B.to_owned(),
                converted_at_unix_ms: 1234,
            })
        }

        fn save_production_project(
            &mut self,
            _path: &Path,
            _project: &ShadeProject,
        ) -> Result<(), String> {
            self.save_calls += 1;
            match &self.save_error {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }
    }

    #[test]
    fn capture_freezes_source_adjustments_and_round_trips() {
        let mut source = ShadeProject::default();
        source
            .adjustments
            .entry("Red".to_owned())
            .or_default()
            .levels
            .gamma = 1.25;
        let captured = ConversionJobCapture::capture(
            &source,
            PathBuf::from(r"C:\Design\Source.shade"),
            HASH_A.to_owned(),
            PathBuf::from(r"C:\Design\Face.tif"),
            None,
            HASH_A.to_owned(),
            CapturedSourceProfile::Embedded,
            recipe(),
            CapturedOutputPolicy::MustNotExist,
            PathBuf::from(r"C:\Production\Face.tif"),
            PathBuf::from(r"C:\Production\Job.shade"),
            "Job".to_owned(),
            "Face".to_owned(),
        )
        .unwrap()
        .with_source_raster_facts(CapturedSourceRasterFacts::new(
            CapturedSourceFormat::Tiff,
            CapturedSourceColorModel::Rgb,
            16,
            3,
        ))
        .unwrap()
        .with_audit_findings(vec![ConversionAuditFinding {
            code: "jpeg_lossy_source".to_owned(),
            message: "JPEG source is lossy.".to_owned(),
            acknowledged: false,
        }])
        .unwrap();
        source.adjustments.get_mut("Red").unwrap().levels.gamma = 0.8;
        let restored: ConversionJobCapture =
            serde_json::from_slice(&serde_json::to_vec(&captured).unwrap()).unwrap();
        assert_eq!(restored.source_recipe.adjustments["Red"].levels.gamma, 1.25);
        assert_eq!(
            restored.source_raster,
            Some(CapturedSourceRasterFacts::new(
                CapturedSourceFormat::Tiff,
                CapturedSourceColorModel::Rgb,
                16,
                3,
            ))
        );
        assert_eq!(restored.audit_findings.len(), 1);
        assert_eq!(restored.audit_findings[0].code, "jpeg_lossy_source");
        assert!(restored.validate().is_ok());
    }

    #[test]
    fn source_raster_facts_fail_closed_on_impossible_topology() {
        let error = capture()
            .with_source_raster_facts(CapturedSourceRasterFacts::new(
                CapturedSourceFormat::Png,
                CapturedSourceColorModel::Rgb,
                8,
                4,
            ))
            .expect_err("RGB capture with four color channels must fail closed");
        assert!(error.contains("RGB model requires 3 channel"));
    }

    #[test]
    fn tampered_recipe_is_rejected_before_backend_runs() {
        let mut captured = capture();
        captured.conversion_recipe.target.name = "Changed".to_owned();
        let mut backend = MockBackend::success();
        let outcome = run_conversion_transaction(
            &captured,
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        assert!(matches!(
            outcome,
            ConversionTransactionOutcome::FailedBeforeCommit {
                phase: ConversionPhase::CaptureValidation,
                ..
            }
        ));
        assert_eq!(backend.commit_calls, 0);
    }

    #[test]
    fn cancellation_before_worker_never_reaches_commit() {
        let cancellation = ConversionCancellation::default();
        cancellation.request();
        let mut backend = MockBackend::success();
        let outcome = run_conversion_transaction(&capture(), &cancellation, &mut backend, |_| {});
        assert!(matches!(
            outcome,
            ConversionTransactionOutcome::CancelledBeforeCommit { .. }
        ));
        assert_eq!(backend.commit_calls, 0);
        assert_eq!(backend.save_calls, 0);
    }

    #[test]
    fn successful_commit_builds_and_saves_clean_production_project() {
        let captured = capture()
            .with_audit_findings(vec![ConversionAuditFinding {
                code: "rgb_not_production_separated".to_owned(),
                message: "RGB source requires production separation.".to_owned(),
                acknowledged: false,
            }])
            .unwrap();
        let mut backend = MockBackend::success();
        let mut phases = Vec::new();
        let outcome = run_conversion_transaction(
            &captured,
            &ConversionCancellation::default(),
            &mut backend,
            |progress| phases.push(progress.phase),
        );
        let ConversionTransactionOutcome::Completed(completed) = outcome else {
            panic!("expected completed transaction");
        };
        assert_eq!(backend.commit_calls, 1);
        assert_eq!(backend.save_calls, 1);
        assert_eq!(
            completed.production_project.project_role,
            ProjectRole::Production
        );
        assert!(completed.production_project.snapshots.is_empty());
        assert_eq!(completed.production_project.conversion_audits.len(), 1);
        assert_eq!(completed.production_project.conversion_audits[0].findings.len(), 1);
        assert_eq!(
            completed.production_project.conversion_audits[0].findings[0].code,
            "rgb_not_production_separated"
        );
        completed.production_project.conversion_audits[0]
            .validate_against_provenance(&completed.production_project.production_provenance[0])
            .unwrap();
        assert_eq!(phases.last(), Some(&ConversionPhase::Complete));
        assert_eq!(
            production_link_for_completed(&completed).role,
            ProjectRole::Production
        );
    }

    #[test]
    fn cancellation_after_output_commit_finishes_project_save_boundary() {
        let mut backend = MockBackend::success();
        backend.cancellation_during_commit = true;
        let outcome = run_conversion_transaction(
            &capture(),
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        assert!(matches!(
            outcome,
            ConversionTransactionOutcome::Completed(_)
        ));
        assert_eq!(backend.save_calls, 1);
    }

    #[test]
    fn project_save_failure_preserves_recoverable_committed_output_and_project() {
        let mut backend = MockBackend::success();
        backend.save_error = Some("simulated project save failure".to_owned());
        let outcome = run_conversion_transaction(
            &capture(),
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        let ConversionTransactionOutcome::OutputCommittedNeedsRecovery {
            committed_output,
            production_project,
            error,
            ..
        } = outcome
        else {
            panic!("expected recoverable output");
        };
        assert!(error.contains("simulated"));
        let project = production_project.expect("recoverable Production project");
        assert_eq!(project.conversion_audits.len(), 1);
        assert!(committed_output.path.ends_with("Face_CMYK.tif"));
    }

    #[test]
    fn worker_failure_before_commit_never_saves_project() {
        let mut backend = MockBackend::success();
        backend.commit_error = Some("transform failed".to_owned());
        let outcome = run_conversion_transaction(
            &capture(),
            &ConversionCancellation::default(),
            &mut backend,
            |_| {},
        );
        assert!(matches!(
            outcome,
            ConversionTransactionOutcome::FailedBeforeCommit {
                phase: ConversionPhase::ColorConversion,
                ..
            }
        ));
        assert_eq!(backend.save_calls, 0);
    }

    #[test]
    fn source_path_and_project_extension_are_validated() {
        let mut captured = capture();
        captured.output_tiff_path = captured.source_face_path.clone();
        assert!(captured.validate().is_err());
        captured.output_tiff_path = PathBuf::from(r"C:\Production\Face.tif");
        captured.production_project_path = PathBuf::from(r"C:\Production\Job.json");
        assert!(captured.validate().is_err());
    }
}
