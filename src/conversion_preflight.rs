use serde::{Deserialize, Serialize};

use crate::model::IccProfileIdentity;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceImageFormat {
    Tiff,
    Png,
    Jpeg,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceColorModel {
    Gray,
    Rgb,
    Cmyk,
    Multichannel,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "state", content = "profile")]
pub enum SourceProfileState {
    Embedded(IccProfileIdentity),
    Assigned(IccProfileIdentity),
    Missing,
    Invalid,
}

impl SourceProfileState {
    pub fn usable_identity(&self) -> Option<&IccProfileIdentity> {
        match self {
            Self::Embedded(identity) | Self::Assigned(identity)
                if !identity.sha256.trim().is_empty() =>
            {
                Some(identity)
            }
            Self::Embedded(_) | Self::Assigned(_) | Self::Missing | Self::Invalid => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlphaFlatteningPolicy {
    White,
    Black,
    CustomRgb([u8; 3]),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceConversionDescriptor {
    pub format: SourceImageFormat,
    pub color_model: SourceColorModel,
    pub bit_depth: u8,
    pub profile: SourceProfileState,
    #[serde(default)]
    pub has_alpha: bool,
    #[serde(default)]
    pub alpha_flattening: Option<AlphaFlatteningPolicy>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreflightSeverity {
    Info,
    Warning,
    Blocking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourcePreflightCode {
    RgbNotProductionSeparated,
    JpegLossySource,
    MissingSourceProfile,
    InvalidSourceProfile,
    MissingProfileIdentity,
    AlphaFlatteningRequired,
    UnsupportedSourceColorModel,
    UnsupportedBitDepth,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourcePreflightFinding {
    pub severity: PreflightSeverity,
    pub code: SourcePreflightCode,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourcePreflightReport {
    pub findings: Vec<SourcePreflightFinding>,
}

impl SourcePreflightReport {
    pub fn can_convert(&self) -> bool {
        !self
            .findings
            .iter()
            .any(|finding| finding.severity == PreflightSeverity::Blocking)
    }

    pub fn has_code(&self, code: SourcePreflightCode) -> bool {
        self.findings.iter().any(|finding| finding.code == code)
    }
}

/// Validate source-image prerequisites that are independent from the selected
/// production target. This module is deliberately UI-free so Color Management,
/// the conversion dialog and queued conversion jobs all share one policy.
pub fn preflight_source(source: &SourceConversionDescriptor) -> SourcePreflightReport {
    let mut report = SourcePreflightReport::default();

    if !matches!(source.bit_depth, 8 | 16) {
        report.findings.push(SourcePreflightFinding {
            severity: PreflightSeverity::Blocking,
            code: SourcePreflightCode::UnsupportedBitDepth,
            message: format!(
                "{}-bit source precision is not supported for production color conversion.",
                source.bit_depth
            ),
        });
    }

    match source.color_model {
        SourceColorModel::Rgb => report.findings.push(SourcePreflightFinding {
            severity: PreflightSeverity::Warning,
            code: SourcePreflightCode::RgbNotProductionSeparated,
            message: "RGB source — not production separated. Convert to the target CMYK/Multichannel printing space before production output.".to_owned(),
        }),
        SourceColorModel::Cmyk => {}
        SourceColorModel::Gray | SourceColorModel::Multichannel => {
            report.findings.push(SourcePreflightFinding {
                severity: PreflightSeverity::Blocking,
                code: SourcePreflightCode::UnsupportedSourceColorModel,
                message: format!(
                    "{:?} is not a supported source color model for the initial production-conversion workflow.",
                    source.color_model
                ),
            });
        }
    }

    if source.format == SourceImageFormat::Jpeg {
        report.findings.push(SourcePreflightFinding {
            severity: PreflightSeverity::Warning,
            code: SourcePreflightCode::JpegLossySource,
            message: "JPEG source — lossy image format. Conversion is allowed, but TIFF/PNG sources preserve source samples more faithfully.".to_owned(),
        });
    }

    match &source.profile {
        SourceProfileState::Missing => report.findings.push(SourcePreflightFinding {
            severity: PreflightSeverity::Blocking,
            code: SourcePreflightCode::MissingSourceProfile,
            message: "Source has no defined ICC profile. Assign the correct source profile before production color conversion.".to_owned(),
        }),
        SourceProfileState::Invalid => report.findings.push(SourcePreflightFinding {
            severity: PreflightSeverity::Blocking,
            code: SourcePreflightCode::InvalidSourceProfile,
            message: "The embedded/assigned source ICC profile is invalid and cannot define source colorimetry.".to_owned(),
        }),
        SourceProfileState::Embedded(identity) | SourceProfileState::Assigned(identity)
            if identity.sha256.trim().is_empty() =>
        {
            report.findings.push(SourcePreflightFinding {
                severity: PreflightSeverity::Blocking,
                code: SourcePreflightCode::MissingProfileIdentity,
                message: "Source ICC profile has no stable identity hash. Reinspect/reassign the profile before conversion.".to_owned(),
            });
        }
        SourceProfileState::Embedded(_) | SourceProfileState::Assigned(_) => {}
    }

    if source.has_alpha && source.alpha_flattening.is_none() {
        report.findings.push(SourcePreflightFinding {
            severity: PreflightSeverity::Blocking,
            code: SourcePreflightCode::AlphaFlatteningRequired,
            message: "Source transparency must be flattened explicitly before production color conversion. Alpha is not a printing ink channel.".to_owned(),
        });
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> IccProfileIdentity {
        IccProfileIdentity {
            description: "sRGB IEC61966-2.1".to_owned(),
            sha256: "source-profile-hash".to_owned(),
        }
    }

    fn rgb_tiff() -> SourceConversionDescriptor {
        SourceConversionDescriptor {
            format: SourceImageFormat::Tiff,
            color_model: SourceColorModel::Rgb,
            bit_depth: 16,
            profile: SourceProfileState::Embedded(profile()),
            has_alpha: false,
            alpha_flattening: None,
        }
    }

    #[test]
    fn rgb_warning_is_actionable_but_not_a_blocker() {
        let report = preflight_source(&rgb_tiff());
        assert!(report.can_convert());
        assert!(report.has_code(SourcePreflightCode::RgbNotProductionSeparated));
    }

    #[test]
    fn missing_source_profile_blocks_conversion() {
        let mut source = rgb_tiff();
        source.profile = SourceProfileState::Missing;
        let report = preflight_source(&source);
        assert!(!report.can_convert());
        assert!(report.has_code(SourcePreflightCode::MissingSourceProfile));
    }

    #[test]
    fn jpeg_is_allowed_but_marked_lossy() {
        let mut source = rgb_tiff();
        source.format = SourceImageFormat::Jpeg;
        source.bit_depth = 8;
        let report = preflight_source(&source);
        assert!(report.can_convert());
        assert!(report.has_code(SourcePreflightCode::JpegLossySource));
    }

    #[test]
    fn png_alpha_requires_explicit_flattening() {
        let mut source = rgb_tiff();
        source.format = SourceImageFormat::Png;
        source.has_alpha = true;
        let blocked = preflight_source(&source);
        assert!(!blocked.can_convert());
        assert!(blocked.has_code(SourcePreflightCode::AlphaFlatteningRequired));

        source.alpha_flattening = Some(AlphaFlatteningPolicy::White);
        assert!(preflight_source(&source).can_convert());
    }

    #[test]
    fn unsupported_precision_blocks_conversion() {
        let mut source = rgb_tiff();
        source.bit_depth = 12;
        let report = preflight_source(&source);
        assert!(!report.can_convert());
        assert!(report.has_code(SourcePreflightCode::UnsupportedBitDepth));
    }

    #[test]
    fn initial_multichannel_source_conversion_is_explicitly_blocked() {
        let mut source = rgb_tiff();
        source.color_model = SourceColorModel::Multichannel;
        let report = preflight_source(&source);
        assert!(!report.can_convert());
        assert!(report.has_code(SourcePreflightCode::UnsupportedSourceColorModel));
    }
}
