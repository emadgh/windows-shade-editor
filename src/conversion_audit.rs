use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::color_conversion::{
    ConversionEngineMode, ProductionProvenance,
    production_provenance::{
        CustomOptimizerProductionPcsMethod, CustomOptimizerProductionProvenance,
        validate_production_provenance,
    },
};
use crate::conversion_analytics::{ConversionUsageReport, analyze_conversion_tiff};
use crate::conversion_transaction::{
    CapturedSourceRasterFacts, CommittedConversionOutput, ConversionJobCapture,
};

pub const CONVERSION_AUDIT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversionAuditSource {
    pub project_path: String,
    pub project_file_sha256: String,
    pub face_path: String,
    pub snapshot_id: Option<u64>,
    pub source_file_sha256: String,
    pub source_profile_sha256: String,
    /// Exact format-neutral raster facts frozen at queue capture. Audit records
    /// written before this field existed deserialize it as `None` and must be
    /// reported as legacy/unknown rather than inferred from the path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raster: Option<CapturedSourceRasterFacts>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversionAuditTarget {
    pub engine_mode: ConversionEngineMode,
    pub target_name: String,
    pub channel_names: Vec<String>,
    pub bit_depth: u8,
    pub output_profile_sha256: Option<String>,
    pub device_link_sha256: Option<String>,
    pub characterization_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversionAuditOutput {
    pub path: String,
    pub sha256: String,
    pub converted_at_unix_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversionAuditFinding {
    pub code: String,
    pub message: String,
    pub acknowledged: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConversionAuditRecord {
    pub schema_version: u32,
    pub app_version: String,
    pub source: ConversionAuditSource,
    pub target: ConversionAuditTarget,
    pub recipe_sha256: String,
    /// Exact authority-bearing Custom Optimizer evidence used by the completed
    /// production conversion. ICC/DeviceLink records must not carry this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_optimizer: Option<CustomOptimizerProductionProvenance>,
    /// Bounded-memory statistics derived from the exact committed TIFF under
    /// the immutable conversion recipe. Legacy/mock records may omit this;
    /// operator surfaces must never reconstruct it from current UI state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ConversionUsageReport>,
    pub output: ConversionAuditOutput,
    /// Findings must come from the actual preflight/transaction path. The audit
    /// model never reconstructs warnings from current UI or project state.
    pub findings: Vec<ConversionAuditFinding>,
}

impl ConversionAuditRecord {
    pub fn from_committed_job(
        capture: &ConversionJobCapture,
        committed_output: &CommittedConversionOutput,
        findings: Vec<ConversionAuditFinding>,
    ) -> Result<Self, String> {
        capture.validate()?;
        if !paths_match(
            committed_output.path.to_string_lossy().as_ref(),
            capture.output_tiff_path.to_string_lossy().as_ref(),
        ) {
            return Err(
                "Committed conversion output path does not match the captured job destination."
                    .to_owned(),
            );
        }
        if !has_sha256(&committed_output.sha256) {
            return Err("Committed conversion output requires a full SHA-256.".to_owned());
        }
        if committed_output.converted_at_unix_ms <= 0 {
            return Err("Committed conversion output requires a valid conversion timestamp.".to_owned());
        }
        validate_findings(&findings)?;

        let custom_optimizer = capture
            .custom_optimizer_evidence
            .as_ref()
            .map(|evidence| evidence.production_provenance(&capture.conversion_recipe_sha256))
            .transpose()
            .map_err(|errors| {
                format!(
                    "Cannot build Custom Optimizer conversion audit evidence: {}",
                    errors.join(" ")
                )
            })?;

        // The production backend guarantees that a returned committed output is
        // durable. When the exact file is available here, bind real TIFF
        // analytics into the immutable audit. Unit/mock backends intentionally
        // use non-existent destinations and therefore do not fabricate usage.
        let usage = if committed_output.path.is_file() {
            Some(
                analyze_conversion_tiff(&committed_output.path, &capture.conversion_recipe)
                    .map_err(|error| {
                        format!(
                            "Cannot analyze committed conversion TIFF for audit evidence: {error}"
                        )
                    })?,
            )
        } else {
            None
        };

        let target = &capture.conversion_recipe.target;
        let record = Self {
            schema_version: CONVERSION_AUDIT_SCHEMA_VERSION,
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            source: ConversionAuditSource {
                project_path: capture.source_project_path.to_string_lossy().into_owned(),
                project_file_sha256: capture.source_project_file_sha256.clone(),
                face_path: capture.source_face_path.to_string_lossy().into_owned(),
                snapshot_id: capture.source_snapshot_id,
                source_file_sha256: capture.source_file_sha256.clone(),
                source_profile_sha256: capture
                    .conversion_recipe
                    .source_profile_identity
                    .sha256
                    .clone(),
                raster: capture.source_raster,
            },
            target: ConversionAuditTarget {
                engine_mode: capture.conversion_recipe.engine_mode,
                target_name: target.name.clone(),
                channel_names: target
                    .channels
                    .iter()
                    .map(|channel| channel.name.clone())
                    .collect(),
                bit_depth: target.bit_depth,
                output_profile_sha256: target
                    .output_profile_identity
                    .as_ref()
                    .map(|identity| identity.sha256.clone()),
                device_link_sha256: target
                    .device_link_identity
                    .as_ref()
                    .map(|identity| identity.sha256.clone()),
                characterization_id: target.characterization_id.clone(),
            },
            recipe_sha256: capture.conversion_recipe_sha256.clone(),
            custom_optimizer,
            usage,
            output: ConversionAuditOutput {
                path: committed_output.path.to_string_lossy().into_owned(),
                sha256: committed_output.sha256.clone(),
                converted_at_unix_ms: committed_output.converted_at_unix_ms,
            },
            findings,
        };
        record.validate()?;
        Ok(record)
    }

    /// Validate the audit object independently of current UI state. This makes
    /// persisted audit data self-checking before it is displayed or exported.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != CONVERSION_AUDIT_SCHEMA_VERSION {
            return Err(format!(
                "Unsupported conversion audit schema {} (expected {}).",
                self.schema_version, CONVERSION_AUDIT_SCHEMA_VERSION
            ));
        }
        if self.app_version.trim().is_empty() {
            return Err("Conversion audit requires the producing app version.".to_owned());
        }
        for (name, value) in [
            ("source project SHA-256", self.source.project_file_sha256.as_str()),
            ("source file SHA-256", self.source.source_file_sha256.as_str()),
            ("source profile SHA-256", self.source.source_profile_sha256.as_str()),
            ("recipe SHA-256", self.recipe_sha256.as_str()),
            ("output SHA-256", self.output.sha256.as_str()),
        ] {
            if !has_sha256(value) {
                return Err(format!("Conversion audit {name} must be a full SHA-256."));
            }
        }
        if self.source.project_path.trim().is_empty() || self.source.face_path.trim().is_empty() {
            return Err("Conversion audit requires Source project and Face paths.".to_owned());
        }
        if let Some(raster) = self.source.raster {
            raster.validate()?;
        }
        if self.target.target_name.trim().is_empty() || self.target.channel_names.is_empty() {
            return Err("Conversion audit requires a named target and channel topology.".to_owned());
        }
        if self.target.channel_names.iter().any(|name| name.trim().is_empty()) {
            return Err("Conversion audit target channels cannot be empty.".to_owned());
        }
        if self.output.path.trim().is_empty() || self.output.converted_at_unix_ms <= 0 {
            return Err("Conversion audit requires committed output identity and timestamp.".to_owned());
        }
        if let Some(usage) = self.usage.as_ref() {
            validate_usage_report(usage, &self.target.channel_names)?;
        }

        match (self.target.engine_mode, self.custom_optimizer.as_ref()) {
            (ConversionEngineMode::CustomOptimizer, Some(custom)) => {
                custom.validate().map_err(|errors| {
                    format!(
                        "Invalid Custom Optimizer conversion audit evidence: {}",
                        errors.join(" ")
                    )
                })?;
                if !hashes_match(&custom.conversion_recipe_sha256, &self.recipe_sha256) {
                    return Err(
                        "Custom Optimizer conversion audit recipe SHA-256 does not match the audit recipe."
                            .to_owned(),
                    );
                }
                if self.target.characterization_id.as_deref()
                    != Some(custom.characterization_id.as_str())
                {
                    return Err(
                        "Custom Optimizer conversion audit characterization does not match the target."
                            .to_owned(),
                    );
                }
            }
            (ConversionEngineMode::CustomOptimizer, None) => {
                return Err(
                    "Custom Optimizer conversion audit requires immutable LUT/validation/calibration evidence."
                        .to_owned(),
                );
            }
            (_, Some(_)) => {
                return Err(
                    "ICC/DeviceLink conversion audit cannot carry Custom Optimizer evidence."
                        .to_owned(),
                );
            }
            (_, None) => {}
        }

        validate_findings(&self.findings)
    }

    /// Prove that this record belongs to one exact persisted Production
    /// provenance entry before attaching, displaying or exporting it. The
    /// Source-project file hash is intentionally not reconstructed here because
    /// ProductionProvenance stores Source artwork identity, not mutable Source
    /// project bytes.
    pub fn validate_against_provenance(
        &self,
        provenance: &ProductionProvenance,
    ) -> Result<(), String> {
        self.validate()?;
        validate_production_provenance(provenance)?;

        let recipe_sha256 = recipe_sha256(&provenance.recipe)?;
        let target = &provenance.recipe.target;
        let channel_names = target
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect::<Vec<_>>();

        if !paths_match(&self.source.project_path, &provenance.source.source_project_path)
            || !paths_match(&self.source.face_path, &provenance.source.source_face_path)
            || self.source.snapshot_id != provenance.source.source_snapshot_id
            || !hashes_match(&self.source.source_file_sha256, &provenance.source.source_file_sha256)
            || !hashes_match(
                &self.source.source_profile_sha256,
                &provenance.recipe.source_profile_identity.sha256,
            )
        {
            return Err(
                "Conversion audit Source identity does not match Production provenance.".to_owned(),
            );
        }
        if !hashes_match(&self.recipe_sha256, &recipe_sha256) {
            return Err(
                "Conversion audit recipe SHA-256 does not match Production provenance.".to_owned(),
            );
        }
        if self.target.engine_mode != provenance.recipe.engine_mode
            || self.target.target_name != target.name
            || self.target.bit_depth != target.bit_depth
            || self.target.channel_names.len() != channel_names.len()
            || !self
                .target
                .channel_names
                .iter()
                .zip(channel_names.iter())
                .all(|(left, right)| left == right)
            || !optional_hashes_match(
                self.target.output_profile_sha256.as_deref(),
                target
                    .output_profile_identity
                    .as_ref()
                    .map(|identity| identity.sha256.as_str()),
            )
            || !optional_hashes_match(
                self.target.device_link_sha256.as_deref(),
                target
                    .device_link_identity
                    .as_ref()
                    .map(|identity| identity.sha256.as_str()),
            )
            || self.target.characterization_id != target.characterization_id
        {
            return Err(
                "Conversion audit target identity does not match Production provenance.".to_owned(),
            );
        }
        if self.custom_optimizer != provenance.custom_optimizer {
            return Err(
                "Conversion audit Custom Optimizer evidence does not match Production provenance."
                    .to_owned(),
            );
        }
        if !paths_match(&self.output.path, &provenance.output_path)
            || !hashes_match(&self.output.sha256, &provenance.output_sha256)
            || self.output.converted_at_unix_ms != provenance.converted_at_unix_ms
        {
            return Err(
                "Conversion audit output identity does not match Production provenance.".to_owned(),
            );
        }
        Ok(())
    }

    pub fn to_pretty_json(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_string_pretty(self)
            .map_err(|error| format!("Cannot serialize conversion audit record: {error}"))
    }

    /// Export a portable copy without leaking absolute Source/Production paths.
    /// Authority-bearing content identities are left byte-for-byte unchanged.
    pub fn to_portable_pretty_json(&self) -> Result<String, String> {
        self.validate()?;
        let mut portable = self.clone();
        portable.source.project_path = portable_path("source-project", &self.source.project_path);
        portable.source.face_path = portable_path("source-face", &self.source.face_path);
        portable.output.path = portable_path("production-output", &self.output.path);
        serde_json::to_string_pretty(&portable).map_err(|error| {
            format!("Cannot serialize portable conversion audit record: {error}")
        })
    }
}

fn recipe_sha256(recipe: &crate::color_conversion::ConversionRecipe) -> Result<String, String> {
    let bytes = serde_json::to_vec(recipe)
        .map_err(|error| format!("Cannot serialize conversion recipe for audit binding: {error}"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn validate_usage_report(
    usage: &ConversionUsageReport,
    target_channels: &[String],
) -> Result<(), String> {
    if usage.pixel_count == 0 {
        return Err("Conversion audit usage requires at least one committed output pixel.".to_owned());
    }
    if usage.channels.len() != target_channels.len() {
        return Err("Conversion audit usage channel count does not match target topology.".to_owned());
    }

    for (channel, expected_name) in usage.channels.iter().zip(target_channels.iter()) {
        if channel.name != *expected_name {
            return Err("Conversion audit usage channel order does not match target topology.".to_owned());
        }
        for (label, value) in [
            ("mean coverage", channel.mean_coverage),
            ("peak coverage", channel.peak_coverage),
            ("p50 coverage", channel.percentiles.p50),
            ("p95 coverage", channel.percentiles.p95),
            ("p99 coverage", channel.percentiles.p99),
        ] {
            if !unit_interval(value) {
                return Err(format!(
                    "Conversion audit usage {label} for '{}' must be finite in 0..=1.",
                    channel.name
                ));
            }
        }
        if channel.percentiles.p50 > channel.percentiles.p95
            || channel.percentiles.p95 > channel.percentiles.p99
            || channel.percentiles.p99 > channel.peak_coverage + 1.0e-4
        {
            return Err(format!(
                "Conversion audit usage percentiles for '{}' are not monotonic/bounded by peak coverage.",
                channel.name
            ));
        }
        if !percentage(channel.nonzero_percent) {
            return Err(format!(
                "Conversion audit non-zero coverage for '{}' must be finite in 0..=100.",
                channel.name
            ));
        }
        if channel.limit_hit_percent.is_some_and(|value| !percentage(value)) {
            return Err(format!(
                "Conversion audit channel-limit hits for '{}' must be finite in 0..=100.",
                channel.name
            ));
        }
        if !channel.integrated_coverage.is_finite() || channel.integrated_coverage < 0.0 {
            return Err(format!(
                "Conversion audit integrated coverage for '{}' must be finite and non-negative.",
                channel.name
            ));
        }
    }

    let max_total = target_channels.len() as f32;
    for (label, value) in [
        ("mean total ink", usage.mean_total_ink),
        ("peak total ink", usage.peak_total_ink),
        ("p50 total ink", usage.total_ink_percentiles.p50),
        ("p95 total ink", usage.total_ink_percentiles.p95),
        ("p99 total ink", usage.total_ink_percentiles.p99),
    ] {
        if !value.is_finite() || !(0.0..=max_total).contains(&value) {
            return Err(format!(
                "Conversion audit usage {label} must be finite in 0..={max_total}."
            ));
        }
    }
    if usage.total_ink_percentiles.p50 > usage.total_ink_percentiles.p95
        || usage.total_ink_percentiles.p95 > usage.total_ink_percentiles.p99
        || usage.total_ink_percentiles.p99 > usage.peak_total_ink + 1.0e-4
    {
        return Err(
            "Conversion audit total-ink percentiles are not monotonic/bounded by peak total ink."
                .to_owned(),
        );
    }
    if usage
        .total_ink_limit_hit_percent
        .is_some_and(|value| !percentage(value))
    {
        return Err(
            "Conversion audit total-ink-limit hits must be finite in 0..=100.".to_owned(),
        );
    }
    if usage
        .neutral_black_share
        .is_some_and(|value| !unit_interval(value))
    {
        return Err(
            "Conversion audit neutral Black share must be finite in 0..=1 when available."
                .to_owned(),
        );
    }
    Ok(())
}

fn unit_interval(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

fn percentage(value: f32) -> bool {
    value.is_finite() && (0.0..=100.0).contains(&value)
}

fn validate_findings(findings: &[ConversionAuditFinding]) -> Result<(), String> {
    for finding in findings {
        if finding.code.trim().is_empty() || finding.message.trim().is_empty() {
            return Err("Conversion audit findings require non-empty code and message.".to_owned());
        }
    }
    Ok(())
}

fn has_sha256(value: &str) -> bool {
    let value = value.trim();
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn hashes_match(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

fn optional_hashes_match(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => hashes_match(left, right),
        (None, None) => true,
        _ => false,
    }
}

fn paths_match(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

fn portable_path(scope: &str, value: &str) -> String {
    let leaf = value
        .rsplit(|character| character == '/' || character == '\\')
        .find(|segment| !segment.trim().is_empty())
        .unwrap_or("redacted");
    format!("<{scope}>/{leaf}")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRecipe,
        ConversionRenderingIntent, ConversionSourceRef, ConversionTargetDefinition,
        SeparationStrategy, TargetChannelDefinition,
    };
    use crate::conversion_analytics::{ChannelUsageStats, CoveragePercentiles};
    use crate::conversion_transaction::{
        CapturedOutputPolicy, CapturedSourceColorModel, CapturedSourceFormat,
        CapturedSourceProfile, CapturedSourceRasterFacts,
    };
    use crate::model::{IccProfileIdentity, ShadeProject};

    fn hash(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn prefixed_hash(character: char) -> String {
        format!("sha256:{}", hash(character))
    }

    fn capture() -> ConversionJobCapture {
        let recipe = ConversionRecipe {
            source_transparency_policy: None,
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::Icc,
            source_profile_identity: IccProfileIdentity {
                description: "Source RGB".to_owned(),
                sha256: hash('a'),
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
                    description: "Press CMYK".to_owned(),
                    sha256: hash('b'),
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
        };
        ConversionJobCapture::capture(
            &ShadeProject::default(),
            PathBuf::from(r"C:\Design\Source.shade"),
            hash('c'),
            PathBuf::from(r"C:\Design\Face.tif"),
            Some(7),
            hash('d'),
            CapturedSourceProfile::Embedded,
            recipe,
            CapturedOutputPolicy::MustNotExist,
            PathBuf::from(r"C:\Production\Face-CMYK.tif"),
            PathBuf::from(r"C:\Production\Job.shade"),
            "Production".to_owned(),
            "Face CMYK".to_owned(),
        )
        .unwrap()
        .with_source_raster_facts(CapturedSourceRasterFacts::new(
            CapturedSourceFormat::Tiff,
            CapturedSourceColorModel::Rgb,
            16,
            3,
        ))
        .unwrap()
    }

    fn committed(capture: &ConversionJobCapture) -> CommittedConversionOutput {
        CommittedConversionOutput {
            path: capture.output_tiff_path.clone(),
            sha256: hash('e'),
            converted_at_unix_ms: 1234,
        }
    }

    fn provenance(
        capture: &ConversionJobCapture,
        output: &CommittedConversionOutput,
    ) -> ProductionProvenance {
        ProductionProvenance {
            source: ConversionSourceRef {
                source_project_path: capture.source_project_path.to_string_lossy().into_owned(),
                source_face_path: capture.source_face_path.to_string_lossy().into_owned(),
                source_snapshot_id: capture.source_snapshot_id,
                source_file_sha256: capture.source_file_sha256.clone(),
            },
            recipe: capture.conversion_recipe.clone(),
            custom_optimizer: None,
            output_path: output.path.to_string_lossy().into_owned(),
            output_sha256: output.sha256.clone(),
            converted_at_unix_ms: output.converted_at_unix_ms,
        }
    }

    fn usage() -> ConversionUsageReport {
        ConversionUsageReport {
            pixel_count: 10,
            channels: ["Cyan", "Magenta", "Yellow", "Black"]
                .into_iter()
                .enumerate()
                .map(|(index, name)| ChannelUsageStats {
                    name: name.to_owned(),
                    mean_coverage: 0.1 + index as f32 * 0.05,
                    peak_coverage: 0.8,
                    percentiles: CoveragePercentiles {
                        p50: 0.1,
                        p95: 0.5,
                        p99: 0.7,
                    },
                    nonzero_percent: 80.0,
                    limit_hit_percent: None,
                    integrated_coverage: 1.0 + index as f64,
                })
                .collect(),
            mean_total_ink: 0.7,
            peak_total_ink: 2.5,
            total_ink_percentiles: CoveragePercentiles {
                p50: 0.6,
                p95: 1.8,
                p99: 2.3,
            },
            total_ink_limit_hit_percent: Some(2.0),
            neutral_black_share: None,
        }
    }

    fn custom_optimizer_provenance(recipe_sha256: String) -> CustomOptimizerProductionProvenance {
        CustomOptimizerProductionProvenance {
            schema_version:
                crate::color_conversion::production_provenance::CUSTOM_OPTIMIZER_PRODUCTION_PROVENANCE_SCHEMA_VERSION,
            lut_identity_content_id: prefixed_hash('1'),
            lut_payload_sha256: hash('2'),
            validation_report_content_id: prefixed_hash('3'),
            characterization_id: prefixed_hash('4'),
            threshold_set_content_id: prefixed_hash('5'),
            calibration_manifest_content_id: prefixed_hash('6'),
            calibration_approval_content_id: prefixed_hash('7'),
            pcs_compatibility_method: CustomOptimizerProductionPcsMethod::IccPcsLabD50TwoDegreeV1,
            pcs_compatibility_content_id: prefixed_hash('8'),
            conversion_recipe_sha256: recipe_sha256,
        }
    }

    #[test]
    fn audit_is_derived_from_frozen_job_and_committed_output() {
        let capture = capture();
        let output = committed(&capture);
        let audit = ConversionAuditRecord::from_committed_job(
            &capture,
            &output,
            vec![ConversionAuditFinding {
                code: "lossy_source".to_owned(),
                message: "Source artwork is JPEG-derived.".to_owned(),
                acknowledged: true,
            }],
        )
        .unwrap();

        assert_eq!(audit.source.snapshot_id, Some(7));
        assert_eq!(audit.source.raster, capture.source_raster);
        assert_eq!(audit.target.channel_names, ["Cyan", "Magenta", "Yellow", "Black"]);
        assert_eq!(audit.recipe_sha256, capture.conversion_recipe_sha256);
        assert_eq!(audit.output.sha256, hash('e'));
        assert!(audit.custom_optimizer.is_none());
        assert!(audit.usage.is_none());
        assert!(audit.findings[0].acknowledged);
        assert!(audit.to_pretty_json().unwrap().contains("Press CMYK"));
    }

    #[test]
    fn audit_binds_to_exact_production_provenance() {
        let capture = capture();
        let output = committed(&capture);
        let audit = ConversionAuditRecord::from_committed_job(&capture, &output, Vec::new()).unwrap();
        let provenance = provenance(&capture, &output);
        audit.validate_against_provenance(&provenance).unwrap();

        let mut foreign = provenance.clone();
        foreign.output_sha256 = hash('f');
        assert!(
            audit
                .validate_against_provenance(&foreign)
                .unwrap_err()
                .contains("output identity")
        );
    }

    #[test]
    fn audit_rejects_output_from_a_different_transaction() {
        let capture = capture();
        let output = CommittedConversionOutput {
            path: PathBuf::from(r"C:\Production\Other.tif"),
            sha256: hash('e'),
            converted_at_unix_ms: 1234,
        };
        let error = ConversionAuditRecord::from_committed_job(&capture, &output, Vec::new())
            .expect_err("foreign output path must fail closed");
        assert!(error.contains("does not match"));
    }

    #[test]
    fn audit_findings_must_be_actual_named_diagnostics() {
        let capture = capture();
        let output = committed(&capture);
        let error = ConversionAuditRecord::from_committed_job(
            &capture,
            &output,
            vec![ConversionAuditFinding {
                code: String::new(),
                message: "warning".to_owned(),
                acknowledged: false,
            }],
        )
        .expect_err("anonymous findings must be rejected");
        assert!(error.contains("non-empty code"));
    }

    #[test]
    fn usage_must_match_exact_target_topology_and_ranges() {
        let capture = capture();
        let output = committed(&capture);
        let mut audit = ConversionAuditRecord::from_committed_job(&capture, &output, Vec::new()).unwrap();
        audit.usage = Some(usage());
        audit.validate().unwrap();

        audit.usage.as_mut().unwrap().channels.swap(0, 1);
        assert!(audit.validate().unwrap_err().contains("channel order"));

        audit.usage = Some(usage());
        audit.usage.as_mut().unwrap().total_ink_limit_hit_percent = Some(101.0);
        assert!(audit.validate().unwrap_err().contains("total-ink-limit hits"));
    }

    #[test]
    fn legacy_audit_without_raster_facts_remains_readable() {
        let capture = capture();
        let output = committed(&capture);
        let audit = ConversionAuditRecord::from_committed_job(&capture, &output, Vec::new()).unwrap();
        let mut value = serde_json::to_value(audit).unwrap();
        value
            .get_mut("source")
            .and_then(serde_json::Value::as_object_mut)
            .unwrap()
            .remove("raster");
        let restored: ConversionAuditRecord = serde_json::from_value(value).unwrap();
        assert!(restored.source.raster.is_none());
        restored.validate().unwrap();
    }

    #[test]
    fn custom_optimizer_audit_requires_exact_authority_evidence() {
        let capture = capture();
        let output = committed(&capture);
        let mut audit =
            ConversionAuditRecord::from_committed_job(&capture, &output, Vec::new()).unwrap();
        audit.target.engine_mode = ConversionEngineMode::CustomOptimizer;
        audit.target.output_profile_sha256 = None;
        audit.target.characterization_id = Some(prefixed_hash('4'));
        audit.custom_optimizer = Some(custom_optimizer_provenance(audit.recipe_sha256.clone()));
        audit.validate().unwrap();

        audit
            .custom_optimizer
            .as_mut()
            .unwrap()
            .conversion_recipe_sha256 = hash('9');
        assert!(audit.validate().unwrap_err().contains("recipe SHA-256"));
    }

    #[test]
    fn portable_export_redacts_absolute_paths_but_preserves_authority_ids() {
        let capture = capture();
        let output = committed(&capture);
        let mut audit = ConversionAuditRecord::from_committed_job(&capture, &output, Vec::new()).unwrap();
        audit.usage = Some(usage());
        let portable = audit.to_portable_pretty_json().unwrap();

        assert!(!portable.contains(r"C:\Design"));
        assert!(!portable.contains(r"C:\Production"));
        assert!(portable.contains("<source-project>/Source.shade"));
        assert!(portable.contains("<source-face>/Face.tif"));
        assert!(portable.contains("<production-output>/Face-CMYK.tif"));
        assert!(portable.contains(&audit.recipe_sha256));
        assert!(portable.contains(&audit.output.sha256));
        assert!(portable.contains("mean_total_ink"));
        assert!(portable.contains("\"raster\""));
    }
}
