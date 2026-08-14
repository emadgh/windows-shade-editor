use crate::color_management::PreviewColorTransform;
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
        project.adjustments.insert("Ink".to_owned(), adjustment);
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
        let face = face(vec![0, 10000, 30000, 55000, 65535]);
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
