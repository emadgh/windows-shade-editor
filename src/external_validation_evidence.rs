use serde::{Deserialize, Serialize};

use crate::color_conversion::ConversionEngineMode;
use crate::conversion_audit::ConversionAuditRecord;

pub const EXTERNAL_VALIDATION_PACKET_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalValidationConsumerKind {
    AdobePhotoshop,
    CeramicRip,
}

impl ExternalValidationConsumerKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::AdobePhotoshop => "Adobe Photoshop",
            Self::CeramicRip => "Ceramic RIP",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExternalValidationStatus {
    #[default]
    Pending,
    Passed,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalValidationFixture {
    pub app_version: String,
    pub engine_mode: ConversionEngineMode,
    pub target_name: String,
    pub bit_depth: u8,
    pub channel_names: Vec<String>,
    pub source_file_sha256: String,
    pub source_profile_sha256: String,
    pub recipe_sha256: String,
    pub output_file: String,
    pub output_sha256: String,
    pub output_profile_sha256: Option<String>,
    pub device_link_sha256: Option<String>,
    pub characterization_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalConsumerEvidence {
    pub consumer_kind: ExternalValidationConsumerKind,
    pub status: ExternalValidationStatus,
    #[serde(default)]
    pub consumer_name: String,
    #[serde(default)]
    pub consumer_version: String,
    #[serde(default)]
    pub observed_bit_depth: Option<u8>,
    #[serde(default)]
    pub observed_channel_names: Vec<String>,
    #[serde(default)]
    pub opened_or_imported_without_repair_warning: Option<bool>,
    #[serde(default)]
    pub raster_dimensions_match: Option<bool>,
    #[serde(default)]
    pub polarity_or_coverage_matches: Option<bool>,
    #[serde(default)]
    pub profile_or_route_behavior_matches: Option<bool>,
    #[serde(default)]
    pub evidence_reference: String,
    #[serde(default)]
    pub reviewer: String,
    #[serde(default)]
    pub reviewed_at_unix_ms: Option<i64>,
    #[serde(default)]
    pub notes: String,
}

impl ExternalConsumerEvidence {
    pub fn pending(consumer_kind: ExternalValidationConsumerKind) -> Self {
        Self {
            consumer_kind,
            status: ExternalValidationStatus::Pending,
            consumer_name: String::new(),
            consumer_version: String::new(),
            observed_bit_depth: None,
            observed_channel_names: Vec::new(),
            opened_or_imported_without_repair_warning: None,
            raster_dimensions_match: None,
            polarity_or_coverage_matches: None,
            profile_or_route_behavior_matches: None,
            evidence_reference: String::new(),
            reviewer: String::new(),
            reviewed_at_unix_ms: None,
            notes: String::new(),
        }
    }

    fn validate_for(
        &self,
        expected_kind: ExternalValidationConsumerKind,
        fixture: &ExternalValidationFixture,
    ) -> Result<(), String> {
        if self.consumer_kind != expected_kind {
            return Err(format!(
                "External validation evidence is in the wrong consumer slot: expected {}, found {}.",
                expected_kind.label(),
                self.consumer_kind.label()
            ));
        }

        if self.status == ExternalValidationStatus::Pending {
            return Ok(());
        }

        for (label, value) in [
            ("consumer name", self.consumer_name.as_str()),
            ("consumer version", self.consumer_version.as_str()),
            ("evidence reference", self.evidence_reference.as_str()),
            ("reviewer", self.reviewer.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(format!(
                    "{} external validation requires a non-empty {label}.",
                    expected_kind.label()
                ));
            }
        }
        if self.reviewed_at_unix_ms.is_none_or(|value| value <= 0) {
            return Err(format!(
                "{} external validation requires a positive review timestamp.",
                expected_kind.label()
            ));
        }

        if self.status == ExternalValidationStatus::Failed {
            return Ok(());
        }

        if self.observed_bit_depth != Some(fixture.bit_depth) {
            return Err(format!(
                "{} pass evidence bit depth does not match the conversion audit fixture.",
                expected_kind.label()
            ));
        }
        if self.observed_channel_names != fixture.channel_names {
            return Err(format!(
                "{} pass evidence channel count/order/names do not match the conversion audit fixture.",
                expected_kind.label()
            ));
        }

        for (label, value) in [
            (
                "open/import without repair or semantic warning",
                self.opened_or_imported_without_repair_warning,
            ),
            ("raster dimensions", self.raster_dimensions_match),
            ("polarity/coverage", self.polarity_or_coverage_matches),
            (
                "profile/route behavior",
                self.profile_or_route_behavior_matches,
            ),
        ] {
            if value != Some(true) {
                return Err(format!(
                    "{} cannot be marked passed until {label} is explicitly verified true.",
                    expected_kind.label()
                ));
            }
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalValidationPacket {
    pub schema_version: u32,
    pub fixture: ExternalValidationFixture,
    pub photoshop: ExternalConsumerEvidence,
    pub ceramic_rip: ExternalConsumerEvidence,
}

impl ExternalValidationPacket {
    pub fn from_conversion_audit(audit: &ConversionAuditRecord) -> Result<Self, String> {
        audit.validate()?;
        let fixture = ExternalValidationFixture {
            app_version: audit.app_version.clone(),
            engine_mode: audit.target.engine_mode,
            target_name: audit.target.target_name.clone(),
            bit_depth: audit.target.bit_depth,
            channel_names: audit.target.channel_names.clone(),
            source_file_sha256: audit.source.source_file_sha256.clone(),
            source_profile_sha256: audit.source.source_profile_sha256.clone(),
            recipe_sha256: audit.recipe_sha256.clone(),
            output_file: portable_file_name(&audit.output.path),
            output_sha256: audit.output.sha256.clone(),
            output_profile_sha256: audit.target.output_profile_sha256.clone(),
            device_link_sha256: audit.target.device_link_sha256.clone(),
            characterization_id: audit.target.characterization_id.clone(),
        };
        let packet = Self {
            schema_version: EXTERNAL_VALIDATION_PACKET_SCHEMA_VERSION,
            fixture,
            photoshop: ExternalConsumerEvidence::pending(
                ExternalValidationConsumerKind::AdobePhotoshop,
            ),
            ceramic_rip: ExternalConsumerEvidence::pending(
                ExternalValidationConsumerKind::CeramicRip,
            ),
        };
        packet.validate()?;
        Ok(packet)
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let packet = serde_json::from_str::<Self>(json)
            .map_err(|error| format!("Cannot decode external validation packet JSON: {error}"))?;
        packet.validate()?;
        Ok(packet)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != EXTERNAL_VALIDATION_PACKET_SCHEMA_VERSION {
            return Err(format!(
                "Unsupported external validation packet schema {} (expected {}).",
                self.schema_version, EXTERNAL_VALIDATION_PACKET_SCHEMA_VERSION
            ));
        }
        if self.fixture.app_version.trim().is_empty()
            || self.fixture.target_name.trim().is_empty()
            || self.fixture.output_file.trim().is_empty()
            || self.fixture.bit_depth == 0
            || self.fixture.channel_names.is_empty()
            || self
                .fixture
                .channel_names
                .iter()
                .any(|name| name.trim().is_empty())
        {
            return Err("External validation packet fixture is incomplete.".to_owned());
        }
        for (label, value) in [
            ("source file SHA-256", self.fixture.source_file_sha256.as_str()),
            (
                "source profile SHA-256",
                self.fixture.source_profile_sha256.as_str(),
            ),
            ("recipe SHA-256", self.fixture.recipe_sha256.as_str()),
            ("output SHA-256", self.fixture.output_sha256.as_str()),
        ] {
            if !has_sha256(value) {
                return Err(format!(
                    "External validation packet {label} must be a full SHA-256."
                ));
            }
        }
        for (label, value) in [
            ("output profile SHA-256", self.fixture.output_profile_sha256.as_deref()),
            ("DeviceLink SHA-256", self.fixture.device_link_sha256.as_deref()),
        ] {
            if value.is_some_and(|value| !has_sha256(value)) {
                return Err(format!(
                    "External validation packet {label} must be a full SHA-256 when present."
                ));
            }
        }

        self.photoshop.validate_for(
            ExternalValidationConsumerKind::AdobePhotoshop,
            &self.fixture,
        )?;
        self.ceramic_rip
            .validate_for(ExternalValidationConsumerKind::CeramicRip, &self.fixture)?;
        Ok(())
    }

    /// Prove that a returned, manually completed packet still refers to the
    /// exact immutable Production conversion audit it was exported from. The
    /// consumer observations may be edited; the authority-bearing fixture may
    /// not drift without becoming a foreign packet.
    pub fn validate_against_conversion_audit(
        &self,
        audit: &ConversionAuditRecord,
    ) -> Result<(), String> {
        self.validate()?;
        audit.validate()?;
        let expected = Self::from_conversion_audit(audit)?.fixture;
        if self.fixture != expected {
            return Err(
                "External validation packet fixture identity does not match this Production conversion audit. Re-export a fresh packet instead of rewriting fixture identity fields."
                    .to_owned(),
            );
        }
        Ok(())
    }

    pub fn externally_accepted(&self) -> bool {
        self.validate().is_ok()
            && self.photoshop.status == ExternalValidationStatus::Passed
            && self.ceramic_rip.status == ExternalValidationStatus::Passed
    }

    pub fn to_pretty_json(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_string_pretty(self)
            .map_err(|error| format!("Cannot serialize external validation packet: {error}"))
    }
}

fn portable_file_name(path: &str) -> String {
    path.trim()
        .rsplit(['/', '\\'])
        .find(|part| !part.trim().is_empty())
        .unwrap_or("production-output.tif")
        .to_owned()
}

fn has_sha256(value: &str) -> bool {
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversion_audit::{
        ConversionAuditOutput, ConversionAuditSource, ConversionAuditTarget,
    };

    fn hash(ch: char) -> String {
        std::iter::repeat_n(ch, 64).collect()
    }

    fn audit() -> ConversionAuditRecord {
        ConversionAuditRecord {
            schema_version: crate::conversion_audit::CONVERSION_AUDIT_SCHEMA_VERSION,
            app_version: "0.21.92".to_owned(),
            source: ConversionAuditSource {
                project_path: r"C:\source\design.shade".to_owned(),
                project_file_sha256: hash('1'),
                face_path: r"C:\source\face.tif".to_owned(),
                snapshot_id: Some(7),
                source_file_sha256: hash('2'),
                source_profile_sha256: hash('3'),
                raster: None,
            },
            target: ConversionAuditTarget {
                engine_mode: ConversionEngineMode::Icc,
                target_name: "Ceramic 5C".to_owned(),
                channel_names: vec![
                    "Cyan".to_owned(),
                    "Magenta".to_owned(),
                    "Yellow".to_owned(),
                    "Black".to_owned(),
                    "Blue".to_owned(),
                ],
                bit_depth: 16,
                output_profile_sha256: Some(hash('4')),
                device_link_sha256: None,
                characterization_id: None,
            },
            recipe_sha256: hash('5'),
            custom_optimizer: None,
            usage: None,
            output: ConversionAuditOutput {
                path: r"D:\production\face-separated.tif".to_owned(),
                sha256: hash('6'),
                converted_at_unix_ms: 1_788_000_000_000,
            },
            findings: Vec::new(),
        }
    }

    fn complete_pass(
        evidence: &mut ExternalConsumerEvidence,
        fixture: &ExternalValidationFixture,
        name: &str,
    ) {
        evidence.status = ExternalValidationStatus::Passed;
        evidence.consumer_name = name.to_owned();
        evidence.consumer_version = "2026.1".to_owned();
        evidence.observed_bit_depth = Some(fixture.bit_depth);
        evidence.observed_channel_names = fixture.channel_names.clone();
        evidence.opened_or_imported_without_repair_warning = Some(true);
        evidence.raster_dimensions_match = Some(true);
        evidence.polarity_or_coverage_matches = Some(true);
        evidence.profile_or_route_behavior_matches = Some(true);
        evidence.evidence_reference = "evidence/screenshots/run-01".to_owned();
        evidence.reviewer = "operator".to_owned();
        evidence.reviewed_at_unix_ms = Some(1_788_100_000_000);
    }

    fn assert_foreign_fixture_rejected(mutator: impl FnOnce(&mut ExternalValidationFixture)) {
        let source_audit = audit();
        let mut packet = ExternalValidationPacket::from_conversion_audit(&source_audit).unwrap();
        mutator(&mut packet.fixture);
        assert!(packet.validate().is_ok());
        assert!(packet
            .validate_against_conversion_audit(&source_audit)
            .is_err());
    }

    #[test]
    fn pending_packet_is_portable_and_not_external_acceptance() {
        let packet = ExternalValidationPacket::from_conversion_audit(&audit()).unwrap();
        assert_eq!(packet.fixture.output_file, "face-separated.tif");
        assert_eq!(packet.photoshop.status, ExternalValidationStatus::Pending);
        assert_eq!(packet.ceramic_rip.status, ExternalValidationStatus::Pending);
        assert!(!packet.externally_accepted());
        let json = packet.to_pretty_json().unwrap();
        assert!(!json.contains(r"D:\production"));
        assert!(json.contains("face-separated.tif"));
    }

    #[test]
    fn json_round_trip_binds_to_exact_source_audit() {
        let source_audit = audit();
        let packet = ExternalValidationPacket::from_conversion_audit(&source_audit).unwrap();
        let json = packet.to_pretty_json().unwrap();
        let decoded = ExternalValidationPacket::from_json(&json).unwrap();
        assert_eq!(decoded, packet);
        decoded
            .validate_against_conversion_audit(&source_audit)
            .unwrap();
    }

    #[test]
    fn malformed_packet_json_is_rejected() {
        assert!(ExternalValidationPacket::from_json("{not-json}").is_err());
    }

    #[test]
    fn incomplete_pass_is_rejected() {
        let mut packet = ExternalValidationPacket::from_conversion_audit(&audit()).unwrap();
        packet.photoshop.status = ExternalValidationStatus::Passed;
        packet.photoshop.consumer_name = "Adobe Photoshop".to_owned();
        packet.photoshop.consumer_version = "2026.1".to_owned();
        packet.photoshop.evidence_reference = "capture".to_owned();
        packet.photoshop.reviewer = "operator".to_owned();
        packet.photoshop.reviewed_at_unix_ms = Some(1_788_100_000_000);
        assert!(packet.validate().is_err());
    }

    #[test]
    fn passed_topology_must_match_audit_fixture_exactly() {
        let mut packet = ExternalValidationPacket::from_conversion_audit(&audit()).unwrap();
        let fixture = packet.fixture.clone();
        complete_pass(&mut packet.photoshop, &fixture, "Adobe Photoshop");
        packet.photoshop.observed_channel_names.swap(0, 1);
        assert!(packet.validate().is_err());
    }

    #[test]
    fn every_authority_bearing_fixture_field_is_audit_bound() {
        assert_foreign_fixture_rejected(|fixture| fixture.app_version.push_str("-foreign"));
        assert_foreign_fixture_rejected(|fixture| {
            fixture.engine_mode = ConversionEngineMode::DeviceLink
        });
        assert_foreign_fixture_rejected(|fixture| fixture.target_name.push_str(" foreign"));
        assert_foreign_fixture_rejected(|fixture| fixture.bit_depth = 8);
        assert_foreign_fixture_rejected(|fixture| fixture.channel_names.swap(0, 1));
        assert_foreign_fixture_rejected(|fixture| fixture.source_file_sha256 = hash('7'));
        assert_foreign_fixture_rejected(|fixture| fixture.source_profile_sha256 = hash('8'));
        assert_foreign_fixture_rejected(|fixture| fixture.recipe_sha256 = hash('9'));
        assert_foreign_fixture_rejected(|fixture| fixture.output_file.push_str(".foreign"));
        assert_foreign_fixture_rejected(|fixture| fixture.output_sha256 = hash('a'));
        assert_foreign_fixture_rejected(|fixture| fixture.output_profile_sha256 = Some(hash('b')));
        assert_foreign_fixture_rejected(|fixture| fixture.device_link_sha256 = Some(hash('c')));
        assert_foreign_fixture_rejected(|fixture| {
            fixture.characterization_id = Some("foreign-characterization".to_owned())
        });
    }

    #[test]
    fn complete_photoshop_and_rip_pass_is_accepted_only_for_bound_audit() {
        let source_audit = audit();
        let mut packet = ExternalValidationPacket::from_conversion_audit(&source_audit).unwrap();
        let fixture = packet.fixture.clone();
        complete_pass(&mut packet.photoshop, &fixture, "Adobe Photoshop");
        complete_pass(&mut packet.ceramic_rip, &fixture, "Ceramic RIP");
        assert!(packet.validate().is_ok());
        packet
            .validate_against_conversion_audit(&source_audit)
            .unwrap();
        assert!(packet.externally_accepted());
    }
}
