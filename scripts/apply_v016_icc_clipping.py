from pathlib import Path

ROOT = Path('.')


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding='utf-8')


def write(path: str, text: str) -> None:
    (ROOT / path).write_text(text, encoding='utf-8', newline='\n')


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f'{label}: expected exactly 1 match, found {count}')
    return text.replace(old, new, 1)


# Version and dependency. Cargo will refresh Cargo.lock in CI before the locked test pass.
cargo = read('Cargo.toml')
cargo = replace_once(cargo, 'version = "0.15.2"', 'version = "0.16.0"', 'Cargo version')
cargo = replace_once(
    cargo,
    'fontdue = "0.9.3"\n',
    'fontdue = "0.9.3"\nlcms2 = { version = "6.1.1", features = ["static"] }\n',
    'lcms dependency',
)
write('Cargo.toml', cargo)


color_management = r'''use lcms2::{
    ColorSpaceSignature, InfoType, Intent, Locale, PixelFormat, Profile, Transform,
};
use serde::{Deserialize, Serialize};

use crate::tiff_io::{ColorModel, TiffMetadata};

/// Rendering intent used only by the preview color-management stage.
/// Export never consumes this setting and always preserves the source ICC bytes.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PreviewRenderingIntent {
    Perceptual,
    RelativeColorimetric,
    Saturation,
    AbsoluteColorimetric,
}

impl Default for PreviewRenderingIntent {
    fn default() -> Self {
        Self::Perceptual
    }
}

impl PreviewRenderingIntent {
    pub fn label(self) -> &'static str {
        match self {
            Self::Perceptual => "Perceptual",
            Self::RelativeColorimetric => "Relative colorimetric",
            Self::Saturation => "Saturation",
            Self::AbsoluteColorimetric => "Absolute colorimetric",
        }
    }

    fn lcms(self) -> Intent {
        match self {
            Self::Perceptual => Intent::Perceptual,
            Self::RelativeColorimetric => Intent::RelativeColorimetric,
            Self::Saturation => Intent::Saturation,
            Self::AbsoluteColorimetric => Intent::AbsoluteColorimetric,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PreviewColorConfig {
    pub enabled: bool,
    pub intent: PreviewRenderingIntent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewColorStatus {
    Pending,
    Disabled,
    NoEmbeddedProfile,
    Applied {
        description: String,
        intent: PreviewRenderingIntent,
    },
    Fallback {
        reason: String,
    },
}

impl Default for PreviewColorStatus {
    fn default() -> Self {
        Self::Pending
    }
}

impl PreviewColorStatus {
    pub fn short_label(&self) -> &'static str {
        match self {
            Self::Pending => "ICC: pending",
            Self::Disabled => "ICC: off",
            Self::NoEmbeddedProfile => "ICC: none",
            Self::Applied { .. } => "ICC: managed",
            Self::Fallback { .. } => "ICC: fallback",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::Pending => "Preview color management has not rendered this Face yet.".to_owned(),
            Self::Disabled => "ICC-aware preview is disabled in Settings. TIFF data and metadata are unchanged.".to_owned(),
            Self::NoEmbeddedProfile => "This TIFF has no embedded ICC profile; Shade Editor is using its unmanaged display fallback.".to_owned(),
            Self::Applied { description, intent } => format!(
                "Embedded ICC '{}' → sRGB display preview · {} intent. Preview-only; TIFF samples and metadata are unchanged.",
                description,
                intent.label(),
            ),
            Self::Fallback { reason } => format!(
                "Embedded ICC could not be used for this preview ({reason}). Shade Editor fell back to the unmanaged display conversion; TIFF data is unchanged."
            ),
        }
    }

    pub fn is_managed(&self) -> bool {
        matches!(self, Self::Applied { .. })
    }

    pub fn is_problem(&self) -> bool {
        matches!(self, Self::Fallback { .. })
    }
}

enum BaseTransform {
    Rgb(Transform<[u16; 3], [u8; 3]>),
    Cmyk(Transform<[u16; 4], [u8; 3]>),
    Gray(Transform<[u16; 1], [u8; 3]>),
}

/// Preview-only ICC transform. This module intentionally owns all LittleCMS use
/// so a future printer/RIP soft-proof can be added here (via a proofing transform)
/// without letting color management leak into the production TIFF export path.
pub struct PreviewColorTransform {
    transform: Option<BaseTransform>,
    status: PreviewColorStatus,
}

impl PreviewColorTransform {
    pub fn new(metadata: &TiffMetadata, config: PreviewColorConfig) -> Self {
        if !config.enabled {
            return Self {
                transform: None,
                status: PreviewColorStatus::Disabled,
            };
        }
        let Some(icc) = metadata.icc_profile.as_deref() else {
            return Self {
                transform: None,
                status: PreviewColorStatus::NoEmbeddedProfile,
            };
        };
        let source = match Profile::new_icc(icc) {
            Ok(profile) => profile,
            Err(err) => return Self::fallback(format!("invalid embedded profile: {err}")),
        };
        let expected = match metadata.color_model {
            ColorModel::Rgb => Some(ColorSpaceSignature::RgbData),
            ColorModel::Cmyk => Some(ColorSpaceSignature::CmykData),
            ColorModel::Gray => Some(ColorSpaceSignature::GrayData),
            ColorModel::Other => None,
        };
        let Some(expected) = expected else {
            return Self::fallback("unsupported TIFF base color model".to_owned());
        };
        let actual = source.color_space();
        if actual != expected {
            return Self::fallback(format!(
                "profile color space {:?} does not match TIFF {}",
                actual,
                metadata.color_model.title(),
            ));
        }

        let description = source
            .info(InfoType::Description, Locale::none())
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Embedded profile".to_owned());
        let destination = Profile::new_srgb();
        let intent = config.intent.lcms();
        let transform = match metadata.color_model {
            ColorModel::Rgb => Transform::new(
                &source,
                PixelFormat::RGB_16,
                &destination,
                PixelFormat::RGB_8,
                intent,
            )
            .map(BaseTransform::Rgb),
            ColorModel::Cmyk => Transform::new(
                &source,
                PixelFormat::CMYK_16,
                &destination,
                PixelFormat::RGB_8,
                intent,
            )
            .map(BaseTransform::Cmyk),
            ColorModel::Gray => Transform::new(
                &source,
                PixelFormat::GRAY_16,
                &destination,
                PixelFormat::RGB_8,
                intent,
            )
            .map(BaseTransform::Gray),
            ColorModel::Other => unreachable!(),
        };
        match transform {
            Ok(transform) => Self {
                transform: Some(transform),
                status: PreviewColorStatus::Applied {
                    description,
                    intent: config.intent,
                },
            },
            Err(err) => Self::fallback(format!("cannot create ICC transform: {err}")),
        }
    }

    fn fallback(reason: String) -> Self {
        Self {
            transform: None,
            status: PreviewColorStatus::Fallback { reason },
        }
    }

    pub fn status(&self) -> &PreviewColorStatus {
        &self.status
    }

    /// Convert only the base RGB/CMYK/Gray channels. Spot separations are kept
    /// outside the ICC transform and composited later from Photoshop DisplayInfo.
    pub fn base_rgb8(&self, planes: &[Vec<u16>], pixel_count: usize) -> Option<Vec<[u8; 3]>> {
        let transform = self.transform.as_ref()?;
        match transform {
            BaseTransform::Rgb(transform) => {
                if planes.len() < 3 {
                    return None;
                }
                let mut src = Vec::with_capacity(pixel_count);
                for pixel in 0..pixel_count {
                    src.push([
                        *planes[0].get(pixel)?,
                        *planes[1].get(pixel)?,
                        *planes[2].get(pixel)?,
                    ]);
                }
                let mut dst = vec![[0u8; 3]; pixel_count];
                transform.transform_pixels(&src, &mut dst);
                Some(dst)
            }
            BaseTransform::Cmyk(transform) => {
                if planes.len() < 4 {
                    return None;
                }
                let mut src = Vec::with_capacity(pixel_count);
                for pixel in 0..pixel_count {
                    src.push([
                        *planes[0].get(pixel)?,
                        *planes[1].get(pixel)?,
                        *planes[2].get(pixel)?,
                        *planes[3].get(pixel)?,
                    ]);
                }
                let mut dst = vec![[0u8; 3]; pixel_count];
                transform.transform_pixels(&src, &mut dst);
                Some(dst)
            }
            BaseTransform::Gray(transform) => {
                if planes.is_empty() {
                    return None;
                }
                let mut src = Vec::with_capacity(pixel_count);
                for pixel in 0..pixel_count {
                    src.push([*planes[0].get(pixel)?]);
                }
                let mut dst = vec![[0u8; 3]; pixel_count];
                transform.transform_pixels(&src, &mut dst);
                Some(dst)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb_metadata(icc_profile: Option<Vec<u8>>) -> TiffMetadata {
        TiffMetadata {
            width: 1,
            height: 1,
            bit_depth: 16,
            samples_per_pixel: 3,
            base_channel_count: 3,
            color_model: ColorModel::Rgb,
            channel_names: vec!["Red".into(), "Green".into(), "Blue".into()],
            channel_display_info: vec![None; 3],
            compression: None,
            predictor: None,
            orientation: None,
            icc_profile,
            photoshop_resources: None,
            photoshop_image_source_data: None,
        }
    }

    #[test]
    fn preview_intent_defaults_to_perceptual() {
        assert_eq!(PreviewRenderingIntent::default(), PreviewRenderingIntent::Perceptual);
    }

    #[test]
    fn disabled_and_missing_profiles_never_create_a_transform() {
        let no_profile = rgb_metadata(None);
        let disabled = PreviewColorTransform::new(
            &no_profile,
            PreviewColorConfig {
                enabled: false,
                intent: PreviewRenderingIntent::Perceptual,
            },
        );
        assert_eq!(*disabled.status(), PreviewColorStatus::Disabled);
        assert!(disabled.base_rgb8(&[vec![0], vec![0], vec![0]], 1).is_none());

        let missing = PreviewColorTransform::new(
            &no_profile,
            PreviewColorConfig {
                enabled: true,
                intent: PreviewRenderingIntent::Perceptual,
            },
        );
        assert_eq!(*missing.status(), PreviewColorStatus::NoEmbeddedProfile);
    }

    #[test]
    fn embedded_srgb_profile_is_used_for_rgb_preview() {
        let icc = Profile::new_srgb().icc().expect("serialize sRGB profile");
        let metadata = rgb_metadata(Some(icc));
        let transform = PreviewColorTransform::new(
            &metadata,
            PreviewColorConfig {
                enabled: true,
                intent: PreviewRenderingIntent::RelativeColorimetric,
            },
        );
        assert!(transform.status().is_managed(), "{:?}", transform.status());
        let output = transform
            .base_rgb8(&[vec![65535], vec![32768], vec![0]], 1)
            .expect("managed RGB output");
        assert_eq!(output.len(), 1);
        assert!(output[0][0] > 245);
        assert!((120..=136).contains(&output[0][1]));
        assert!(output[0][2] < 10);
    }

    #[test]
    fn profile_color_space_mismatch_falls_back_safely() {
        let icc = Profile::new_srgb().icc().expect("serialize sRGB profile");
        let mut metadata = rgb_metadata(Some(icc));
        metadata.color_model = ColorModel::Cmyk;
        metadata.samples_per_pixel = 4;
        metadata.base_channel_count = 4;
        metadata.channel_names.push("Black".into());
        metadata.channel_display_info.push(None);
        let transform = PreviewColorTransform::new(
            &metadata,
            PreviewColorConfig {
                enabled: true,
                intent: PreviewRenderingIntent::Perceptual,
            },
        );
        assert!(transform.status().is_problem());
    }
}
'''
write('src/color_management.rs', color_management)


# Replace render module as one coherent unit: adjustment statistics + display color management.
render = r'''use crate::color_management::PreviewColorTransform;
use crate::model::{ShadeProject, apply_curve, apply_levels};
use crate::tiff_io::{self, ColorModel, PreviewFace};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChannelClippingStats {
    pub sample_count: u64,
    pub levels_black_count: u64,
    pub levels_white_count: u64,
    pub curve_black_count: u64,
    pub curve_white_count: u64,
}

impl ChannelClippingStats {
    fn percent(count: u64, total: u64) -> f32 {
        if total == 0 {
            0.0
        } else {
            count as f32 * 100.0 / total as f32
        }
    }

    pub fn levels_black_percent(self) -> f32 {
        Self::percent(self.levels_black_count, self.sample_count)
    }

    pub fn levels_white_percent(self) -> f32 {
        Self::percent(self.levels_white_count, self.sample_count)
    }

    pub fn curve_black_percent(self) -> f32 {
        Self::percent(self.curve_black_count, self.sample_count)
    }

    pub fn curve_white_percent(self) -> f32 {
        Self::percent(self.curve_white_count, self.sample_count)
    }

    pub fn max_percent(self) -> f32 {
        self.levels_black_percent()
            .max(self.levels_white_percent())
            .max(self.curve_black_percent())
            .max(self.curve_white_percent())
    }
}

pub fn adjusted_planes(face: &PreviewFace, project: &ShadeProject) -> Vec<Vec<u16>> {
    adjusted_planes_with_stats(face, project).0
}

/// Apply the production adjustment order to preview samples and collect clipping
/// estimates from the same downsampled working-space data. These statistics are
/// diagnostic only; export still processes the full-resolution TIFF separately.
pub fn adjusted_planes_with_stats(
    face: &PreviewFace,
    project: &ShadeProject,
) -> (Vec<Vec<u16>>, Vec<ChannelClippingStats>) {
    let channel_count = face.channels.len();
    if channel_count == 0 {
        return (Vec::new(), Vec::new());
    }
    let pixel_count = face.channels[0].len();
    let names = &face.metadata.channel_names;
    let mut stats = vec![
        ChannelClippingStats {
            sample_count: pixel_count as u64,
            ..ChannelClippingStats::default()
        };
        channel_count
    ];

    let mut prepared = (0..channel_count)
        .map(|_| vec![0.0f32; pixel_count])
        .collect::<Vec<_>>();

    for channel in 0..channel_count {
        let adjustment = project.adjustments.get(&names[channel]);
        for pixel in 0..pixel_count {
            let raw = face.channels[channel][pixel] as f32 / 65535.0;
            prepared[channel][pixel] = if let Some(adjustment) = adjustment {
                if adjustment.enabled {
                    let black = adjustment.levels.input_black.clamp(0.0, 0.9999);
                    let white = adjustment.levels.input_white.clamp(black + 0.0001, 1.0);
                    if black > 0.0 && raw <= black {
                        stats[channel].levels_black_count += 1;
                    }
                    if white < 1.0 && raw >= white {
                        stats[channel].levels_white_count += 1;
                    }
                    apply_levels(raw, adjustment.levels)
                } else {
                    raw
                }
            } else {
                raw
            };
        }
    }

    let mut output = (0..channel_count)
        .map(|_| vec![0u16; pixel_count])
        .collect::<Vec<_>>();

    for out_channel in 0..channel_count {
        let name = &names[out_channel];
        let adjustment = project.adjustments.get(name);
        for pixel in 0..pixel_count {
            let value = if let Some(adjustment) = adjustment {
                if !adjustment.enabled {
                    prepared[out_channel][pixel]
                } else {
                    let mut mixed = adjustment.mixer.constant;
                    for source in 0..channel_count {
                        let coefficient = adjustment
                            .mixer
                            .coefficients
                            .get(&names[source])
                            .copied()
                            .unwrap_or(if source == out_channel { 1.0 } else { 0.0 });
                        mixed += prepared[source][pixel] * coefficient;
                    }
                    let curve_black = adjustment.curve.input_black.clamp(0.0, 1.0);
                    let curve_white = adjustment
                        .curve
                        .input_white
                        .clamp(curve_black + 1.0 / 65535.0, 1.0);
                    if mixed < 0.0 || (curve_black > 0.0 && mixed <= curve_black) {
                        stats[out_channel].curve_black_count += 1;
                    }
                    if mixed > 1.0 || (curve_white < 1.0 && mixed >= curve_white) {
                        stats[out_channel].curve_white_count += 1;
                    }
                    apply_curve(mixed, adjustment.curve)
                }
            } else {
                prepared[out_channel][pixel]
            };
            output[out_channel][pixel] = (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
        }
    }

    (output, stats)
}

pub fn rgba_from_planes(
    face: &PreviewFace,
    planes: &[Vec<u16>],
    solo_channel: Option<usize>,
) -> Vec<u8> {
    rgba_from_planes_impl(face, planes, solo_channel, None)
}

pub fn rgba_from_planes_with_color(
    face: &PreviewFace,
    planes: &[Vec<u16>],
    solo_channel: Option<usize>,
    color: &PreviewColorTransform,
) -> Vec<u8> {
    rgba_from_planes_impl(face, planes, solo_channel, Some(color))
}

fn rgba_from_planes_impl(
    face: &PreviewFace,
    planes: &[Vec<u16>],
    solo_channel: Option<usize>,
    color: Option<&PreviewColorTransform>,
) -> Vec<u8> {
    let width = face.width;
    let height = face.height;
    let pixel_count = width.saturating_mul(height);
    let mut rgba = Vec::with_capacity(pixel_count.saturating_mul(4));

    // Solo is an engineering separation view, not a colorimetric composite.
    if let Some(channel) = solo_channel.filter(|index| *index < planes.len()) {
        let invert = tiff_io::solo_channel_uses_ink_coverage(&face.metadata, channel);
        for value in &planes[channel] {
            let byte = (*value >> 8) as u8;
            let gray = if invert {
                255u8.saturating_sub(byte)
            } else {
                byte
            };
            rgba.extend_from_slice(&[gray, gray, gray, 255]);
        }
        return rgba;
    }

    if let Some(base_rgb) = color.and_then(|color| color.base_rgb8(planes, pixel_count)) {
        for (pixel, base) in base_rgb.into_iter().enumerate() {
            let mut rgb = [
                base[0] as f32 / 255.0,
                base[1] as f32 / 255.0,
                base[2] as f32 / 255.0,
            ];
            // Spot channels deliberately stay outside the base ICC transform.
            composite_extra_channels(face, planes, pixel, &mut rgb);
            push_rgb(&mut rgba, rgb);
        }
        return rgba;
    }

    match face.metadata.color_model {
        ColorModel::Rgb if planes.len() >= 3 => {
            for pixel in 0..pixel_count {
                let mut rgb = [
                    planes[0][pixel] as f32 / 65535.0,
                    planes[1][pixel] as f32 / 65535.0,
                    planes[2][pixel] as f32 / 65535.0,
                ];
                composite_extra_channels(face, planes, pixel, &mut rgb);
                push_rgb(&mut rgba, rgb);
            }
        }
        ColorModel::Cmyk if planes.len() >= 4 => {
            for pixel in 0..pixel_count {
                let c = planes[0][pixel] as f32 / 65535.0;
                let m = planes[1][pixel] as f32 / 65535.0;
                let y = planes[2][pixel] as f32 / 65535.0;
                let k = planes[3][pixel] as f32 / 65535.0;
                let mut rgb = [
                    1.0 - (c + k).min(1.0),
                    1.0 - (m + k).min(1.0),
                    1.0 - (y + k).min(1.0),
                ];
                composite_extra_channels(face, planes, pixel, &mut rgb);
                push_rgb(&mut rgba, rgb);
            }
        }
        ColorModel::Gray if !planes.is_empty() => {
            for pixel in 0..pixel_count {
                let gray = planes[0][pixel] as f32 / 65535.0;
                let mut rgb = [gray, gray, gray];
                composite_extra_channels(face, planes, pixel, &mut rgb);
                push_rgb(&mut rgba, rgb);
            }
        }
        _ => {
            if let Some(first) = planes.first() {
                for value in first.iter().take(pixel_count) {
                    let gray = (*value >> 8) as u8;
                    rgba.extend_from_slice(&[gray, gray, gray, 255]);
                }
            }
        }
    }

    rgba
}

fn composite_extra_channels(
    face: &PreviewFace,
    planes: &[Vec<u16>],
    pixel: usize,
    rgb: &mut [f32; 3],
) {
    let first_extra = face.metadata.base_channel_count.min(planes.len());
    if first_extra >= planes.len() {
        return;
    }

    const DIAGNOSTIC_TINTS: [[f32; 3]; 8] = [
        [0.00, 0.90, 0.45],
        [0.95, 0.15, 0.20],
        [0.15, 0.55, 0.95],
        [0.95, 0.15, 0.90],
        [0.95, 0.75, 0.05],
        [0.05, 0.80, 0.85],
        [0.55, 0.25, 0.90],
        [0.95, 0.45, 0.05],
    ];

    for channel_index in first_extra..planes.len() {
        let plane = &planes[channel_index];
        if pixel >= plane.len() {
            continue;
        }
        let coverage =
            tiff_io::extra_channel_preview_coverage(&face.metadata, channel_index, plane[pixel]);
        if coverage <= 0.001 {
            continue;
        }
        let display = face
            .metadata
            .channel_display_info
            .get(channel_index)
            .and_then(|value| *value);
        match display {
            Some(info) if info.is_spot() => {
                let tint = info.rgb.unwrap_or(
                    DIAGNOSTIC_TINTS[(channel_index - first_extra) % DIAGNOSTIC_TINTS.len()],
                );
                blend_tint(rgb, tint, (coverage * info.solidity).clamp(0.0, 1.0));
            }
            Some(_) => {
                // Known Alpha/protected display channels are not printing inks.
            }
            None => {
                let tint = DIAGNOSTIC_TINTS[(channel_index - first_extra) % DIAGNOSTIC_TINTS.len()];
                blend_tint(rgb, tint, (coverage * 0.72).clamp(0.0, 0.72));
            }
        }
    }
}

fn blend_tint(rgb: &mut [f32; 3], tint: [f32; 3], strength: f32) {
    if strength <= 0.0 {
        return;
    }
    for component in 0..3 {
        rgb[component] = rgb[component] * (1.0 - strength) + tint[component] * strength;
    }
}

fn push_rgb(rgba: &mut Vec<u8>, rgb: [f32; 3]) {
    rgba.extend_from_slice(&[
        (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        255,
    ]);
}

pub fn histogram(values: &[u16]) -> [u32; 256] {
    let mut bins = [0u32; 256];
    for value in values {
        let index = usize::from(*value >> 8);
        bins[index] = bins[index].saturating_add(1);
    }
    bins
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ChannelAdjustment, Curve, Levels, MixerRow};
    use crate::tiff_io::TiffMetadata;
    use std::collections::BTreeMap;

    fn face(values: Vec<u16>) -> PreviewFace {
        PreviewFace {
            metadata: TiffMetadata {
                width: values.len() as u32,
                height: 1,
                bit_depth: 16,
                samples_per_pixel: 1,
                base_channel_count: 1,
                color_model: ColorModel::Gray,
                channel_names: vec!["Ink".to_owned()],
                channel_display_info: vec![None],
                compression: None,
                predictor: None,
                orientation: None,
                icc_profile: None,
                photoshop_resources: None,
                photoshop_image_source_data: None,
            },
            width: values.len(),
            height: 1,
            channels: vec![values.clone()],
            histograms: vec![histogram(&values)],
        }
    }

    fn project(adjustment: ChannelAdjustment) -> ShadeProject {
        let mut project = ShadeProject::default();
        project
            .adjustments
            .insert("Ink".to_owned(), adjustment);
        project
    }

    #[test]
    fn default_adjustment_reports_no_clipping() {
        let face = face(vec![0, 16384, 32768, 49152, 65535]);
        let (_, stats) = adjusted_planes_with_stats(&face, &project(ChannelAdjustment::default()));
        assert_eq!(stats[0].levels_black_count, 0);
        assert_eq!(stats[0].levels_white_count, 0);
        assert_eq!(stats[0].curve_black_count, 0);
        assert_eq!(stats[0].curve_white_count, 0);
    }

    #[test]
    fn levels_thresholds_report_shadow_and_highlight_clipping() {
        let face = face(vec![0, 10000, 30000, 50000, 65535]);
        let adjustment = ChannelAdjustment {
            levels: Levels {
                input_black: 0.20,
                input_white: 0.80,
                ..Levels::default()
            },
            ..ChannelAdjustment::default()
        };
        let (_, stats) = adjusted_planes_with_stats(&face, &project(adjustment));
        assert_eq!(stats[0].levels_black_count, 2);
        assert_eq!(stats[0].levels_white_count, 2);
        assert!((stats[0].levels_black_percent() - 40.0).abs() < 0.001);
    }

    #[test]
    fn mixer_overflow_is_reported_at_curve_stage() {
        let face = face(vec![32768, 65535]);
        let mut coefficients = BTreeMap::new();
        coefficients.insert("Ink".to_owned(), 2.0);
        let adjustment = ChannelAdjustment {
            levels: Levels::default(),
            curve: Curve::default(),
            mixer: MixerRow {
                coefficients,
                constant: 0.1,
            },
            enabled: true,
        };
        let (_, stats) = adjusted_planes_with_stats(&face, &project(adjustment));
        assert_eq!(stats[0].curve_white_count, 2);
    }
}
'''
write('src/render.rs', render)


# Settings are app-level and deliberately do not enter the .shade schema.
settings = read('src/settings_v6.rs')
settings = replace_once(
    settings,
    'use crate::export_batch::{ConflictPolicy, DEFAULT_EXPORT_TEMPLATE};\n',
    'use crate::color_management::PreviewRenderingIntent;\nuse crate::export_batch::{ConflictPolicy, DEFAULT_EXPORT_TEMPLATE};\n',
    'settings color-management import',
)
settings = replace_once(
    settings,
    '    pub max_preview_dimension: u32,\n    pub adjustment_tabs: bool,\n',
    '    pub max_preview_dimension: u32,\n    pub icc_preview: bool,\n    pub icc_rendering_intent: PreviewRenderingIntent,\n    pub show_clipping_warnings: bool,\n    pub adjustment_tabs: bool,\n',
    'settings fields',
)
settings = replace_once(
    settings,
    '            max_preview_dimension: 1800,\n            adjustment_tabs: false,\n',
    '            max_preview_dimension: 1800,\n            icc_preview: true,\n            icc_rendering_intent: PreviewRenderingIntent::Perceptual,\n            show_clipping_warnings: true,\n            adjustment_tabs: false,\n',
    'settings defaults',
)
settings = replace_once(
    settings,
    '    #[test]\n    fn compact_curve_controls_default_off() {\n',
    '''    #[test]\n    fn color_management_and_clipping_defaults_are_safe() {\n        let settings = AppSettings::default();\n        assert!(settings.icc_preview);\n        assert_eq!(\n            settings.icc_rendering_intent,\n            PreviewRenderingIntent::Perceptual\n        );\n        assert!(settings.show_clipping_warnings);\n    }\n\n    #[test]\n    fn compact_curve_controls_default_off() {\n''',
    'settings tests',
)
write('src/settings_v6.rs', settings)


app = read('src/app_main.rs')
app = replace_once(
    app,
    '#[path = "app_log.rs"]\nmod app_log;\n#[path = "dpi.rs"]\n',
    '#[path = "app_log.rs"]\nmod app_log;\n#[path = "color_management.rs"]\nmod color_management;\n#[path = "dpi.rs"]\n',
    'app color module',
)
app = replace_once(
    app,
    'use model::{ChannelAdjustment, ShadeProject, TEST_CODE_ALL_CHANNELS, TestCodePosition};\n',
    'use color_management::{PreviewColorConfig, PreviewColorStatus, PreviewRenderingIntent};\nuse model::{ChannelAdjustment, ShadeProject, TEST_CODE_ALL_CHANNELS, TestCodePosition};\n',
    'app color imports',
)
app = replace_once(
    app,
    '''    adjusted: Vec<Vec<u16>>,\n    texture: Option<egui::TextureHandle>,\n    original_texture: Option<egui::TextureHandle>,\n''',
    '''    adjusted: Vec<Vec<u16>>,\n    clipping: Vec<render::ChannelClippingStats>,\n    color_status: PreviewColorStatus,\n    texture: Option<egui::TextureHandle>,\n    original_texture: Option<egui::TextureHandle>,\n''',
    'runtime face diagnostics',
)
app = replace_once(
    app,
    '''    adjusted: Vec<Vec<u16>>,\n    rgba: Vec<u8>,\n    original_rgba: Vec<u8>,\n''',
    '''    adjusted: Vec<Vec<u16>>,\n    clipping: Vec<render::ChannelClippingStats>,\n    color_status: PreviewColorStatus,\n    rgba: Vec<u8>,\n    original_rgba: Vec<u8>,\n''',
    'render result diagnostics',
)
app = replace_once(
    app,
    '''            adjusted: Vec::new(),\n            texture: None,\n            original_texture: None,\n''',
    '''            adjusted: Vec::new(),\n            clipping: Vec::new(),\n            color_status: PreviewColorStatus::Pending,\n            texture: None,\n            original_texture: None,\n''',
    'runtime face defaults',
)
app = replace_once(
    app,
    '''            face.adjusted = result.adjusted;\n            let image = egui::ColorImage::from_rgba_unmultiplied(\n''',
    '''            face.adjusted = result.adjusted;\n            face.clipping = result.clipping;\n            face.color_status = result.color_status;\n            let image = egui::ColorImage::from_rgba_unmultiplied(\n''',
    'poll render diagnostics',
)
app = replace_once(
    app,
    '''        let preview = Arc::clone(&face.preview);\n        let project = self.project.clone();\n        let solo_channel = self.solo_channel;\n        let tx = self.render_tx.clone();\n        self.render_busy = Some((face_index, generation));\n        std::thread::spawn(move || {\n            let adjusted = render::adjusted_planes(&preview, &project);\n            let rgba = render::rgba_from_planes(&preview, &adjusted, solo_channel);\n            let original_rgba = render::rgba_from_planes(&preview, &preview.channels, solo_channel);\n            let _ = tx.send(RenderResult {\n                face_index,\n                generation,\n                adjusted,\n                rgba,\n                original_rgba,\n            });\n        });\n''',
    '''        let preview = Arc::clone(&face.preview);\n        let project = self.project.clone();\n        let solo_channel = self.solo_channel;\n        let color_config = PreviewColorConfig {\n            enabled: self.settings.icc_preview,\n            intent: self.settings.icc_rendering_intent,\n        };\n        let tx = self.render_tx.clone();\n        self.render_busy = Some((face_index, generation));\n        std::thread::spawn(move || {\n            let (adjusted, clipping) = render::adjusted_planes_with_stats(&preview, &project);\n            let color = color_management::PreviewColorTransform::new(&preview.metadata, color_config);\n            let rgba =\n                render::rgba_from_planes_with_color(&preview, &adjusted, solo_channel, &color);\n            let original_rgba = render::rgba_from_planes_with_color(\n                &preview,\n                &preview.channels,\n                solo_channel,\n                &color,\n            );\n            let color_status = color.status().clone();\n            let _ = tx.send(RenderResult {\n                face_index,\n                generation,\n                adjusted,\n                clipping,\n                color_status,\n                rgba,\n                original_rgba,\n            });\n        });\n''',
    'render worker',
)
app = replace_once(
    app,
    '''    fn mark_current_preview_dirty(&mut self) {\n        if let Some(face) = self.faces.get_mut(self.current_face) {\n            face.generation = face.generation.wrapping_add(1).max(1);\n        }\n    }\n''',
    '''    fn mark_current_preview_dirty(&mut self) {\n        if let Some(face) = self.faces.get_mut(self.current_face) {\n            face.generation = face.generation.wrapping_add(1).max(1);\n        }\n    }\n\n    /// Re-render textures for application-only display settings without marking\n    /// the .shade project dirty. ICC preview settings never alter project/TIFF data.\n    fn invalidate_display_previews(&mut self) {\n        for face in &mut self.faces {\n            face.generation = face.generation.wrapping_add(1).max(1);\n            face.color_status = PreviewColorStatus::Pending;\n        }\n        self.render_busy = None;\n    }\n''',
    'display invalidation helper',
)

# Channel clipping diagnostics.
app = replace_once(
    app,
    '''        let adjusted_histograms = face\n            .adjusted\n            .iter()\n            .map(|values| render::histogram(values))\n            .collect::<Vec<_>>();\n        let base_count = face.preview.metadata.base_channel_count;\n''',
    '''        let adjusted_histograms = face\n            .adjusted\n            .iter()\n            .map(|values| render::histogram(values))\n            .collect::<Vec<_>>();\n        let clipping = face.clipping.clone();\n        let base_count = face.preview.metadata.base_channel_count;\n''',
    'channel clipping clone',
)
app = replace_once(
    app,
    '''            let hover = match display_info {\n                Some(info) if info.is_spot() => format!(\n                    "Photoshop Spot Channel · Solidity {:.0}% · click to select; click again to toggle solo preview.",\n                    info.solidity * 100.0\n                ),\n                Some(_) => "Photoshop Alpha/auxiliary channel · click to select; click again to toggle solo preview.".to_owned(),\n                None => "Extra TIFF channel (Spot/Alpha type not declared) · click to select; click again to toggle solo preview.".to_owned(),\n            };\n            let response = clickable_channel_row(\n                ui,\n                self.selected_channel == index,\n                is_solo,\n                &label,\n                accent,\n                32.0,\n            )\n            .on_hover_text(hover);\n''',
    '''            let mut hover = match display_info {\n                Some(info) if info.is_spot() => format!(\n                    "Photoshop Spot Channel · Solidity {:.0}% · click to select; click again to toggle solo preview.",\n                    info.solidity * 100.0\n                ),\n                Some(_) => "Photoshop Alpha/auxiliary channel · click to select; click again to toggle solo preview.".to_owned(),\n                None => "Extra TIFF channel (Spot/Alpha type not declared) · click to select; click again to toggle solo preview.".to_owned(),\n            };\n            let warning = if self.settings.show_clipping_warnings {\n                clipping.get(index).copied().and_then(clipping_warning_color)\n            } else {\n                None\n            };\n            if self.settings.show_clipping_warnings {\n                if let Some(stats) = clipping.get(index).copied() {\n                    hover.push_str(&format!("\\n{}", clipping_tooltip(stats)));\n                }\n            }\n            let response = clickable_channel_row(\n                ui,\n                self.selected_channel == index,\n                is_solo,\n                &label,\n                accent,\n                warning,\n                32.0,\n            )\n            .on_hover_text(hover);\n''',
    'channel clipping indicator',
)

# Adjustment summary for selected channel (shown in both Selected and All scope).
app = replace_once(
    app,
    '''        let active_adjusted_histogram = all_adjusted_histograms.get(self.selected_channel).copied();\n        let control_accent = self\n''',
    '''        let active_adjusted_histogram = all_adjusted_histograms.get(self.selected_channel).copied();\n        let active_clipping = face.clipping.get(self.selected_channel).copied();\n        let control_accent = self\n''',
    'active clipping stats',
)
app = replace_once(
    app,
    '''        });\n\n        let mut frame = egui::Frame::new().inner_margin(8).corner_radius(6);\n''',
    '''        });\n        if self.settings.show_clipping_warnings {\n            if let Some(stats) = active_clipping {\n                clipping_summary_ui(ui, stats);\n            }\n        }\n\n        let mut frame = egui::Frame::new().inner_margin(8).corner_radius(6);\n''',
    'adjustment clipping summary',
)

# Viewport ICC status badge.
app = replace_once(
    app,
    '''        let meta = face.preview.metadata.clone();\n        let dpi_info = face.dpi;\n        let texture = face.texture.clone();\n''',
    '''        let meta = face.preview.metadata.clone();\n        let dpi_info = face.dpi;\n        let color_status = face.color_status.clone();\n        let texture = face.texture.clone();\n''',
    'viewport color status clone',
)
app = replace_once(
    app,
    '''            ui.label(meta.color_model.title());\n            ui.label(format!("{} channels", meta.samples_per_pixel));\n        });\n''',
    '''            ui.label(meta.color_model.title());\n            ui.label(format!("{} channels", meta.samples_per_pixel));\n            let icc_response = if color_status.is_problem() {\n                ui.colored_label(egui::Color32::YELLOW, color_status.short_label())\n            } else if color_status.is_managed() {\n                ui.colored_label(egui::Color32::LIGHT_GREEN, color_status.short_label())\n            } else {\n                ui.label(color_status.short_label())\n            };\n            icc_response.on_hover_text(color_status.detail());\n        });\n''',
    'viewport ICC status',
)

# Settings UI. Capture display settings separately so app-only changes re-render but don't dirty project.
app = replace_once(
    app,
    '''        let mut open = self.show_settings;\n        let mut rebuild_previews_requested = false;\n        egui::Window::new("Settings")\n''',
    '''        let mut open = self.show_settings;\n        let mut rebuild_previews_requested = false;\n        let color_preview_before = (\n            self.settings.icc_preview,\n            self.settings.icc_rendering_intent,\n        );\n        egui::Window::new("Settings")\n''',
    'settings color before',
)
app = replace_once(
    app,
    '''                ui.small("The max dimension is used when TIFF previews are loaded. Use Rebuild previews to apply a changed value to Faces already open in this project.");\n                ui.separator();\n                ui.heading("Export & storage");\n''',
    '''                ui.small("The max dimension is used when TIFF previews are loaded. Use Rebuild previews to apply a changed value to Faces already open in this project.");\n                ui.separator();\n                ui.heading("Color management & clipping");\n                changed |= ui\n                    .checkbox(\n                        &mut self.settings.icc_preview,\n                        "ICC-aware preview (embedded profile → sRGB)",\n                    )\n                    .changed();\n                egui::ComboBox::from_label("Preview rendering intent")\n                    .selected_text(self.settings.icc_rendering_intent.label())\n                    .show_ui(ui, |ui| {\n                        for intent in [\n                            PreviewRenderingIntent::Perceptual,\n                            PreviewRenderingIntent::RelativeColorimetric,\n                            PreviewRenderingIntent::Saturation,\n                            PreviewRenderingIntent::AbsoluteColorimetric,\n                        ] {\n                            changed |= ui\n                                .selectable_value(\n                                    &mut self.settings.icc_rendering_intent,\n                                    intent,\n                                    intent.label(),\n                                )\n                                .changed();\n                        }\n                    });\n                changed |= ui\n                    .checkbox(\n                        &mut self.settings.show_clipping_warnings,\n                        "Show per-channel clipping warnings",\n                    )\n                    .changed();\n                ui.small("ICC is preview-only: it converts the TIFF base RGB/CMYK/Gray channels to sRGB for the screen, then composites declared Photoshop Spot channels. Alpha channels remain excluded. Exported samples, embedded ICC bytes and Photoshop resources are not changed by this setting.");\n                ui.small("Clipping percentages are estimates from the loaded preview samples. Yellow starts at 0.10%; red at 1.00%. Full-resolution export data is not sampled or modified for these warnings.");\n                ui.small("The color-management stage is isolated so a printer/RIP Soft Proof profile can be added later without changing the TIFF export pipeline.");\n                ui.separator();\n                ui.heading("Export & storage");\n''',
    'settings ICC section',
)
app = replace_once(
    app,
    '''        self.show_settings = open;\n        if rebuild_previews_requested {\n            self.rebuild_previews();\n        }\n''',
    '''        self.show_settings = open;\n        if color_preview_before\n            != (self.settings.icc_preview, self.settings.icc_rendering_intent)\n        {\n            self.invalidate_display_previews();\n        }\n        if rebuild_previews_requested {\n            self.rebuild_previews();\n        }\n''',
    'settings ICC invalidation',
)

# Extend channel row with a right-side warning dot.
app = replace_once(
    app,
    '''fn clickable_channel_row(\n    ui: &mut egui::Ui,\n    selected: bool,\n    solo: bool,\n    label: &str,\n    accent: egui::Color32,\n    height: f32,\n) -> egui::Response {\n''',
    '''fn clickable_channel_row(\n    ui: &mut egui::Ui,\n    selected: bool,\n    solo: bool,\n    label: &str,\n    accent: egui::Color32,\n    warning: Option<egui::Color32>,\n    height: f32,\n) -> egui::Response {\n''',
    'channel row signature',
)
app = replace_once(
    app,
    '''    ui.painter().text(\n        egui::pos2(rect.left() + 28.0, rect.center().y),\n        egui::Align2::LEFT_CENTER,\n        label,\n        egui::FontId::proportional(14.0),\n        accent,\n    );\n    response\n}\n''',
    '''    ui.painter().text(\n        egui::pos2(rect.left() + 28.0, rect.center().y),\n        egui::Align2::LEFT_CENTER,\n        label,\n        egui::FontId::proportional(14.0),\n        accent,\n    );\n    if let Some(color) = warning {\n        ui.painter().circle_filled(\n            egui::pos2(rect.right() - 10.0, rect.center().y),\n            4.5,\n            color,\n        );\n    }\n    response\n}\n''',
    'channel warning paint',
)

# Add clipping helpers before the row painter section.
app = replace_once(
    app,
    '''fn clickable_channel_row(\n''',
    '''fn clipping_warning_color(stats: render::ChannelClippingStats) -> Option<egui::Color32> {\n    let max = stats.max_percent();\n    if max >= 1.0 {\n        Some(egui::Color32::RED)\n    } else if max >= 0.10 {\n        Some(egui::Color32::YELLOW)\n    } else {\n        None\n    }\n}\n\nfn clipping_tooltip(stats: render::ChannelClippingStats) -> String {\n    format!(\n        "Preview clipping estimate · Levels: black ~{:.2}%, white ~{:.2}% · Curve: black ~{:.2}%, white ~{:.2}% · {} sampled pixels",\n        stats.levels_black_percent(),\n        stats.levels_white_percent(),\n        stats.curve_black_percent(),\n        stats.curve_white_percent(),\n        stats.sample_count,\n    )\n}\n\nfn clipping_summary_ui(ui: &mut egui::Ui, stats: render::ChannelClippingStats) {\n    let warning = clipping_warning_color(stats);\n    egui::Frame::new()\n        .inner_margin(6)\n        .corner_radius(4)\n        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)\n        .show(ui, |ui| {\n            ui.horizontal_wrapped(|ui| {\n                if let Some(color) = warning {\n                    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());\n                    ui.painter().circle_filled(rect.center(), 4.0, color);\n                }\n                ui.strong("Clipping estimate");\n                ui.label(format!(\n                    "Levels  Black ~{:.2}%  White ~{:.2}%",\n                    stats.levels_black_percent(),\n                    stats.levels_white_percent(),\n                ));\n                ui.separator();\n                ui.label(format!(\n                    "Curve  Black ~{:.2}%  White ~{:.2}%",\n                    stats.curve_black_percent(),\n                    stats.curve_white_percent(),\n                ));\n            });\n            ui.small(format!(\n                "Preview estimate from {} sampled pixels · yellow ≥0.10% · red ≥1.00%",\n                stats.sample_count,\n            ));\n        });\n    ui.add_space(5.0);\n}\n\nfn clickable_channel_row(\n''',
    'clipping UI helpers',
)
write('src/app_main.rs', app)


notes = read('RELEASE_NOTES.md')
section = '''# Shade Editor 0.16.0\n\n- Add ICC-aware preview color management using each TIFF's embedded RGB/CMYK/Gray profile and an sRGB display destination; rendering intent is selectable in Settings.\n- Keep ICC conversion strictly in the preview path. Export continues to use the original TIFF samples and preserve embedded ICC/Photoshop metadata without applying display transforms.\n- Composite declared Photoshop Spot separations after the base ICC transform using their existing DisplayInfo color/solidity; known Alpha channels remain excluded from the printing composite.\n- Add per-channel Levels and Curve clipping estimates from preview working-space samples, with yellow/red warning indicators in the channel list and detailed percentages in Adjustments.\n- Isolate color management behind a dedicated preview module so a future printer/RIP Soft Proof profile can use a proofing transform without changing production TIFF export.\n- Keep the adjustment order Levels → Mixer → Curve and keep `.shade` schema v9 unchanged.\n\n'''
if notes.startswith('# Shade Editor 0.16.0'):
    raise SystemExit('release notes already contain 0.16.0')
write('RELEASE_NOTES.md', section + notes)

print('Applied Shade Editor v0.16.0 ICC preview + clipping migration')
