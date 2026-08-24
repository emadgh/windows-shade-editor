use crate::conversion_workflow::ConversionSaveGate;
use crate::design_source::{DesignSourceColorModel, DesignSourceDescriptor, SourceLossiness};
use crate::model::IccProfileIdentity;
use crate::source_profile_fallback::is_srgb_fallback_identity;
use crate::source_transparency::SourceTransparencyPolicy;
use crate::tiff_io::ColorModel;

pub use crate::design_source::{SourceImageFormat, TransparencyState};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceProfileState {
    Embedded(IccProfileIdentity),
    Assigned(IccProfileIdentity),
    Missing,
    Invalid(String),
}

impl SourceProfileState {
    pub fn identity(&self) -> Option<&IccProfileIdentity> {
        match self {
            Self::Embedded(identity) | Self::Assigned(identity) => Some(identity),
            Self::Missing | Self::Invalid(_) => None,
        }
    }

    pub fn is_ready_for_conversion(&self) -> bool {
        self.identity()
            .is_some_and(|identity| !identity.sha256.trim().is_empty())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PreflightSeverity {
    Info,
    Warning,
    Blocking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreflightCode {
    RgbNotProductionSeparated,
    JpegLossySource,
    MissingSourceProfile,
    InvalidSourceProfile,
    UnsavedSourceProject,
    NoSourceFaces,
    UnresolvedTransparency,
    UnsupportedSourceColorModel,
    UnsupportedBitDepth,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreflightFinding {
    pub code: PreflightCode,
    pub severity: PreflightSeverity,
    pub title: &'static str,
    pub detail: String,
    pub action: Option<&'static str>,
}

/// Legacy-compatible preflight input. Runtime TIFF callers can keep constructing
/// this directly while format-neutral design-source callers should use
/// `build_conversion_preflight_for_source` so JPEG coding and PNG alpha semantics
/// are taken from the shared source descriptor.
#[derive(Clone, Debug)]
pub struct ConversionPreflightInput {
    pub format: SourceImageFormat,
    pub color_model: ColorModel,
    pub bit_depth: u8,
    pub profile: SourceProfileState,
    pub save_gate: ConversionSaveGate,
    pub transparency: TransparencyState,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConversionPreflightReport {
    pub findings: Vec<PreflightFinding>,
}

impl ConversionPreflightReport {
    pub fn can_convert(&self) -> bool {
        !self
            .findings
            .iter()
            .any(|finding| finding.severity == PreflightSeverity::Blocking)
    }

    pub fn highest_severity(&self) -> Option<PreflightSeverity> {
        self.findings.iter().map(|finding| finding.severity).max()
    }

    pub fn contains(&self, code: PreflightCode) -> bool {
        self.findings.iter().any(|finding| finding.code == code)
    }
}

fn tiff_color_model(model: DesignSourceColorModel) -> ColorModel {
    match model {
        DesignSourceColorModel::Gray => ColorModel::Gray,
        DesignSourceColorModel::Rgb => ColorModel::Rgb,
        DesignSourceColorModel::Cmyk => ColorModel::Cmyk,
        DesignSourceColorModel::Other => ColorModel::Other,
    }
}

/// Build preflight from the shared TIFF/PNG/JPEG source descriptor. This path is
/// the contract for future runtime import wiring; it keeps JPEG lossiness tied to
/// the actual coding process instead of the file extension alone.
pub fn build_conversion_preflight_for_source(
    source: &DesignSourceDescriptor<'_>,
    profile: SourceProfileState,
    save_gate: ConversionSaveGate,
) -> ConversionPreflightReport {
    build_conversion_preflight_for_source_with_policy(source, profile, save_gate, None)
}

pub fn build_conversion_preflight_for_source_with_policy(
    source: &DesignSourceDescriptor<'_>,
    profile: SourceProfileState,
    save_gate: ConversionSaveGate,
    transparency_policy: Option<&SourceTransparencyPolicy>,
) -> ConversionPreflightReport {
    let input = ConversionPreflightInput {
        format: source.format,
        color_model: tiff_color_model(source.color_model),
        bit_depth: source.bit_depth,
        profile,
        save_gate,
        transparency: if source.transparency == TransparencyState::PresentUnresolved
            && transparency_policy.is_some()
        {
            TransparencyState::Flattened
        } else {
            source.transparency
        },
    };
    let mut report = build_conversion_preflight(&input);
    if source.format == SourceImageFormat::Jpeg && source.lossiness == SourceLossiness::Lossless {
        report
            .findings
            .retain(|finding| finding.code != PreflightCode::JpegLossySource);
    }
    report
}

/// Build the production-conversion preflight without mutating project or source data.
///
/// This is intentionally UI-independent. The conversion dialog/status bar may render
/// these findings, but the rules live here so future queue/batch conversion uses the
/// same safety gate.
pub fn build_conversion_preflight(input: &ConversionPreflightInput) -> ConversionPreflightReport {
    let mut findings = Vec::new();

    match input.color_model {
        ColorModel::Rgb => findings.push(PreflightFinding {
            code: PreflightCode::RgbNotProductionSeparated,
            severity: PreflightSeverity::Warning,
            title: "RGB source — not production separated",
            detail: "Convert this artwork to the target CMYK/Multichannel printing space before Shade Editor production output.".to_owned(),
            action: Some("Convert Color..."),
        }),
        ColorModel::Cmyk => {}
        ColorModel::Gray | ColorModel::Other => findings.push(PreflightFinding {
            code: PreflightCode::UnsupportedSourceColorModel,
            severity: PreflightSeverity::Blocking,
            title: "Unsupported source color model",
            detail: format!(
                "{} source conversion is not enabled by the current production conversion contract.",
                input.color_model.title()
            ),
            action: None,
        }),
    }

    if input.format == SourceImageFormat::Jpeg {
        findings.push(PreflightFinding {
            code: PreflightCode::JpegLossySource,
            severity: PreflightSeverity::Warning,
            title: "JPEG source — lossy image format",
            detail: "JPEG can be used as design artwork, but compression artifacts may be carried into the production separation.".to_owned(),
            action: None,
        });
    }

    if !matches!(input.bit_depth, 8 | 16) {
        findings.push(PreflightFinding {
            code: PreflightCode::UnsupportedBitDepth,
            severity: PreflightSeverity::Blocking,
            title: "Unsupported source bit depth",
            detail: format!(
                "{}-bit source data is not supported for production color conversion.",
                input.bit_depth
            ),
            action: None,
        });
    }

    match &input.profile {
        SourceProfileState::Missing if input.color_model == ColorModel::Rgb => findings.push(PreflightFinding {
            code: PreflightCode::MissingSourceProfile,
            severity: PreflightSeverity::Warning,
            title: "No Source ICC — sRGB fallback will be used",
            detail: "This RGB artwork has no assigned or embedded Source ICC. Conversion is allowed and will interpret its RGB samples as sRGB. Assign a Source ICC if the artwork uses another RGB encoding.".to_owned(),
            action: Some("Assign Source Profile..."),
        }),
        SourceProfileState::Missing => findings.push(PreflightFinding {
            code: PreflightCode::MissingSourceProfile,
            severity: PreflightSeverity::Blocking,
            title: "Source ICC profile required",
            detail: "This source color model has no safe default production interpretation. Assign or confirm its Source ICC before conversion.".to_owned(),
            action: Some("Assign Source Profile..."),
        }),
        SourceProfileState::Invalid(reason) => findings.push(PreflightFinding {
            code: PreflightCode::InvalidSourceProfile,
            severity: PreflightSeverity::Blocking,
            title: "Invalid source ICC profile",
            detail: reason.clone(),
            action: Some("Assign Source Profile..."),
        }),
        SourceProfileState::Embedded(identity) if is_srgb_fallback_identity(identity) => findings.push(PreflightFinding {
            code: PreflightCode::MissingSourceProfile,
            severity: PreflightSeverity::Warning,
            title: "No Source ICC — using reproducible sRGB fallback",
            detail: "This RGB artwork is untagged. Shade Editor will interpret it as sRGB for this conversion and records the exact fallback ICC identity in the recipe. Assign a Source ICC if another RGB encoding is intended.".to_owned(),
            action: Some("Assign Source Profile..."),
        }),
        SourceProfileState::Embedded(identity) | SourceProfileState::Assigned(identity)
            if identity.sha256.trim().is_empty() =>
        {
            findings.push(PreflightFinding {
                code: PreflightCode::InvalidSourceProfile,
                severity: PreflightSeverity::Blocking,
                title: "Source ICC identity is incomplete",
                detail: "The selected source ICC profile has no stable SHA-256 identity and cannot be captured in a reproducible conversion recipe.".to_owned(),
                action: Some("Reassign Source Profile..."),
            });
        }
        SourceProfileState::Embedded(_) | SourceProfileState::Assigned(_) => {}
    }

    match input.save_gate {
        ConversionSaveGate::Ready => {}
        ConversionSaveGate::NoSourceFaces => findings.push(PreflightFinding {
            code: PreflightCode::NoSourceFaces,
            severity: PreflightSeverity::Blocking,
            title: "No source Face to convert",
            detail: "Add a source Face before starting production color conversion.".to_owned(),
            action: None,
        }),
        ConversionSaveGate::SaveAsRequired => findings.push(PreflightFinding {
            code: PreflightCode::UnsavedSourceProject,
            severity: PreflightSeverity::Blocking,
            title: "Save Source project before conversion",
            detail: "This Source project has never been saved. Conversion must capture a reproducible saved .shade state.".to_owned(),
            action: Some("Save Source Project As..."),
        }),
        ConversionSaveGate::SaveRequired => findings.push(PreflightFinding {
            code: PreflightCode::UnsavedSourceProject,
            severity: PreflightSeverity::Blocking,
            title: "Save source changes before conversion",
            detail: "Current adjustments differ from the saved Source project. Save them before conversion so the output can be reproduced later.".to_owned(),
            action: Some("Save & Continue"),
        }),
    }

    if input.transparency == TransparencyState::PresentUnresolved {
        findings.push(PreflightFinding {
            code: PreflightCode::UnresolvedTransparency,
            severity: PreflightSeverity::Blocking,
            title: "Resolve source transparency",
            detail: "Transparency/alpha is not a printing ink. Flatten it against an explicit background before production conversion.".to_owned(),
            action: Some("Choose Flatten Background..."),
        });
    }

    ConversionPreflightReport { findings }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> SourceProfileState {
        SourceProfileState::Embedded(IccProfileIdentity {
            description: "Adobe RGB (1998)".to_owned(),
            sha256: "abc123".to_owned(),
        })
    }

    fn ready_rgb() -> ConversionPreflightInput {
        ConversionPreflightInput {
            format: SourceImageFormat::Tiff,
            color_model: ColorModel::Rgb,
            bit_depth: 16,
            profile: profile(),
            save_gate: ConversionSaveGate::Ready,
            transparency: TransparencyState::None,
        }
    }

    #[test]
    fn rgb_warning_is_informational_for_gate_but_conversion_can_continue() {
        let report = build_conversion_preflight(&ready_rgb());
        assert!(report.contains(PreflightCode::RgbNotProductionSeparated));
        assert!(report.can_convert());
        assert_eq!(report.highest_severity(), Some(PreflightSeverity::Warning));
    }

    #[test]
    fn missing_rgb_profile_warns_but_does_not_block_conversion() {
        let mut input = ready_rgb();
        input.profile = SourceProfileState::Missing;
        let report = build_conversion_preflight(&input);
        assert!(report.contains(PreflightCode::MissingSourceProfile));
        assert!(report.can_convert());
        assert_eq!(report.highest_severity(), Some(PreflightSeverity::Warning));
    }

    #[test]
    fn missing_cmyk_profile_remains_blocking() {
        let mut input = ready_rgb();
        input.color_model = ColorModel::Cmyk;
        input.profile = SourceProfileState::Missing;
        let report = build_conversion_preflight(&input);
        assert!(report.contains(PreflightCode::MissingSourceProfile));
        assert!(!report.can_convert());
    }

    #[test]
    fn explicit_rgb_fallback_identity_is_reported_as_warning() {
        let mut input = ready_rgb();
        input.profile = SourceProfileState::Embedded(
            crate::source_profile_fallback::srgb_fallback_identity().unwrap(),
        );
        let report = build_conversion_preflight(&input);
        assert!(report.contains(PreflightCode::MissingSourceProfile));
        assert!(report.can_convert());
    }

    #[test]
    fn dirty_source_blocks_until_saved() {
        let mut input = ready_rgb();
        input.save_gate = ConversionSaveGate::SaveRequired;
        let report = build_conversion_preflight(&input);
        assert!(report.contains(PreflightCode::UnsavedSourceProject));
        assert!(!report.can_convert());
        assert_eq!(
            report
                .findings
                .iter()
                .find(|finding| finding.code == PreflightCode::UnsavedSourceProject)
                .and_then(|finding| finding.action),
            Some("Save & Continue")
        );
    }

    #[test]
    fn source_descriptor_routes_png_alpha_to_blocking_preflight() {
        let source = DesignSourceDescriptor::new(
            SourceImageFormat::Png,
            DesignSourceColorModel::Rgb,
            16,
            3,
            None,
            TransparencyState::PresentUnresolved,
            SourceLossiness::Lossless,
        );
        let report = build_conversion_preflight_for_source(
            &source,
            profile(),
            ConversionSaveGate::Ready,
        );
        assert!(report.contains(PreflightCode::UnresolvedTransparency));
        assert!(!report.can_convert());
    }

    #[test]
    fn explicit_flatten_policy_resolves_only_the_transparency_blocker() {
        let source = DesignSourceDescriptor::new(
            SourceImageFormat::Png,
            DesignSourceColorModel::Rgb,
            16,
            3,
            None,
            TransparencyState::PresentUnresolved,
            SourceLossiness::Lossless,
        );
        let policy = SourceTransparencyPolicy::FlattenSolidRgb16 {
            background_rgb: [u16::MAX, u16::MAX, u16::MAX],
        };
        let report = build_conversion_preflight_for_source_with_policy(
            &source,
            profile(),
            ConversionSaveGate::Ready,
            Some(&policy),
        );
        assert!(!report.contains(PreflightCode::UnresolvedTransparency));
        assert!(report.contains(PreflightCode::RgbNotProductionSeparated));
        assert!(report.can_convert());
    }

    #[test]
    fn source_descriptor_warns_dct_jpeg_as_lossy() {
        let source = DesignSourceDescriptor::new(
            SourceImageFormat::Jpeg,
            DesignSourceColorModel::Rgb,
            8,
            3,
            None,
            TransparencyState::None,
            SourceLossiness::Lossy,
        );
        let report = build_conversion_preflight_for_source(
            &source,
            profile(),
            ConversionSaveGate::Ready,
        );
        assert!(report.contains(PreflightCode::JpegLossySource));
        assert!(report.can_convert());
    }

    #[test]
    fn source_descriptor_does_not_falsely_warn_lossless_jpeg() {
        let source = DesignSourceDescriptor::new(
            SourceImageFormat::Jpeg,
            DesignSourceColorModel::Rgb,
            8,
            3,
            None,
            TransparencyState::None,
            SourceLossiness::Lossless,
        );
        let report = build_conversion_preflight_for_source(
            &source,
            profile(),
            ConversionSaveGate::Ready,
        );
        assert!(!report.contains(PreflightCode::JpegLossySource));
        assert!(report.can_convert());
    }

    #[test]
    fn legacy_or_incomplete_profile_identity_is_not_reproducible() {
        let mut input = ready_rgb();
        input.profile = SourceProfileState::Assigned(IccProfileIdentity {
            description: "sRGB".to_owned(),
            sha256: String::new(),
        });
        let report = build_conversion_preflight(&input);
        assert!(report.contains(PreflightCode::InvalidSourceProfile));
        assert!(!report.can_convert());
    }
}
