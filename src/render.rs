use crate::color_management::PreviewColorTransform;
use crate::model::{Curve, Levels, MASTER_ADJUSTMENT_KEY, ShadeProject, apply_curve, apply_levels};
use crate::runtime_preview::{RuntimeColorModel, RuntimePreviewSource};
use crate::tiff_io;
#[cfg(test)]
use crate::tiff_io::PreviewFace;

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

pub fn adjusted_planes<P: RuntimePreviewSource + ?Sized>(
    face: &P,
    project: &ShadeProject,
) -> Vec<Vec<u16>> {
    adjusted_planes_with_stats(face, project).0
}

/// Apply the production adjustment order to preview samples and collect clipping
/// estimates from the same downsampled working-space data. These statistics are
/// diagnostic only; export still processes the full-resolution TIFF separately.
pub fn adjusted_planes_with_stats<P: RuntimePreviewSource + ?Sized>(
    face: &P,
    project: &ShadeProject,
) -> (Vec<Vec<u16>>, Vec<ChannelClippingStats>) {
    let channel_count = face.channels().len();
    if channel_count == 0 {
        return (Vec::new(), Vec::new());
    }
    let pixel_count = face.channels()[0].len();
    let names = face.channel_names();
    let master = project
        .adjustments
        .get(MASTER_ADJUSTMENT_KEY)
        .filter(|adjustment| adjustment.enabled);
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

    // Levels stage: channel-specific Levels first, then the independent Master
    // Levels pass. The Master pass never writes back into any channel control.
    for channel in 0..channel_count {
        let adjustment = project.adjustments.get(&names[channel]);
        for pixel in 0..pixel_count {
            let raw = face.channels()[channel][pixel] as f32 / 65535.0;
            let mut value = raw;
            let mut clipped_black = false;
            let mut clipped_white = false;
            if let Some(adjustment) = adjustment.filter(|adjustment| adjustment.enabled) {
                let (black, white) = levels_clipping(value, adjustment.levels);
                clipped_black |= black;
                clipped_white |= white;
                value = apply_levels(value, adjustment.levels);
            }
            if let Some(master) = master {
                let (black, white) = levels_clipping(value, master.levels);
                clipped_black |= black;
                clipped_white |= white;
                value = apply_levels(value, master.levels);
            }
            prepared[channel][pixel] = value;
            if clipped_black {
                stats[channel].levels_black_count += 1;
            }
            if clipped_white {
                stats[channel].levels_white_count += 1;
            }
        }
    }

    let mut output = (0..channel_count)
        .map(|_| vec![0u16; pixel_count])
        .collect::<Vec<_>>();

    // Mixer remains channel-owned. Curve is then applied per-channel followed
    // by the independent Master Curve as a common finishing pass.
    for out_channel in 0..channel_count {
        let name = &names[out_channel];
        let adjustment = project.adjustments.get(name);
        for pixel in 0..pixel_count {
            let mut value =
                if let Some(adjustment) = adjustment.filter(|adjustment| adjustment.enabled) {
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
                    mixed
                } else {
                    prepared[out_channel][pixel]
                };

            let mut clipped_black = false;
            let mut clipped_white = false;
            if let Some(adjustment) = adjustment.filter(|adjustment| adjustment.enabled) {
                let (black, white) = curve_clipping(value, adjustment.curve);
                clipped_black |= black;
                clipped_white |= white;
                value = apply_curve(value, adjustment.curve);
            }
            if let Some(master) = master {
                let (black, white) = curve_clipping(value, master.curve);
                clipped_black |= black;
                clipped_white |= white;
                value = apply_curve(value, master.curve);
            }
            if clipped_black {
                stats[out_channel].curve_black_count += 1;
            }
            if clipped_white {
                stats[out_channel].curve_white_count += 1;
            }
            output[out_channel][pixel] = (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
        }
    }

    (output, stats)
}

fn levels_clipping(value: f32, levels: Levels) -> (bool, bool) {
    let black = levels.input_black.clamp(0.0, 0.9999);
    let white = levels.input_white.clamp(black + 0.0001, 1.0);
    (black > 0.0 && value <= black, white < 1.0 && value >= white)
}

fn curve_clipping(value: f32, curve: Curve) -> (bool, bool) {
    let black = curve.input_black.clamp(0.0, 1.0);
    let white = curve.input_white.clamp(black + 1.0 / 65535.0, 1.0);
    (
        value < 0.0 || (black > 0.0 && value <= black),
        value > 1.0 || (white < 1.0 && value >= white),
    )
}

pub fn rgba_from_planes<P: RuntimePreviewSource + ?Sized>(
    face: &P,
    planes: &[Vec<u16>],
    solo_channel: Option<usize>,
) -> Vec<u8> {
    rgba_from_planes_impl(face, planes, solo_channel, None)
}

pub fn rgba_from_planes_with_color<P: RuntimePreviewSource + ?Sized>(
    face: &P,
    planes: &[Vec<u16>],
    solo_channel: Option<usize>,
    color: &PreviewColorTransform,
) -> Vec<u8> {
    rgba_from_planes_impl(face, planes, solo_channel, Some(color))
}

fn rgba_from_planes_impl<P: RuntimePreviewSource + ?Sized>(
    face: &P,
    planes: &[Vec<u16>],
    solo_channel: Option<usize>,
    color: Option<&PreviewColorTransform>,
) -> Vec<u8> {
    let width = face.width();
    let height = face.height();
    let pixel_count = width.saturating_mul(height);
    let mut rgba = Vec::with_capacity(pixel_count.saturating_mul(4));

    // Solo is an engineering separation view, not a colorimetric composite.
    if let Some(channel) = solo_channel.filter(|index| *index < planes.len()) {
        let invert = face
            .tiff_metadata()
            .is_some_and(|metadata| tiff_io::solo_channel_uses_ink_coverage(metadata, channel));
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

    match face.color_model() {
        RuntimeColorModel::Rgb if planes.len() >= 3 => {
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
        RuntimeColorModel::Cmyk if planes.len() >= 4 => {
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
        RuntimeColorModel::Gray if !planes.is_empty() => {
            for pixel in 0..pixel_count {
                let gray = planes[0][pixel] as f32 / 65535.0;
                let mut rgb = [gray, gray, gray];
                composite_extra_channels(face, planes, pixel, &mut rgb);
                push_rgb(&mut rgba, rgb);
            }
        }
        RuntimeColorModel::Other if has_declared_spot_channels(face, planes.len()) => {
            // A Photoshop Multichannel/Separated TIFF may consist entirely of
            // printing inks and therefore has no RGB/CMYK base composite. Render
            // declared Spot channels over paper white using their DisplayInfo
            // color and Solidity rather than falling back to channel-1 grayscale.
            for pixel in 0..pixel_count {
                let mut rgb = [1.0, 1.0, 1.0];
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

fn has_declared_spot_channels<P: RuntimePreviewSource + ?Sized>(
    face: &P,
    channel_count: usize,
) -> bool {
    let Some(metadata) = face.tiff_metadata() else {
        return false;
    };
    metadata
        .channel_display_info
        .iter()
        .take(channel_count)
        .flatten()
        .any(|info| info.is_spot())
}

fn composite_extra_channels<P: RuntimePreviewSource + ?Sized>(
    face: &P,
    planes: &[Vec<u16>],
    pixel: usize,
    rgb: &mut [f32; 3],
) {
    let Some(metadata) = face.tiff_metadata() else {
        return;
    };
    let first_extra = metadata.base_channel_count.min(planes.len());

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

    for channel_index in 0..planes.len() {
        let display = metadata
            .channel_display_info
            .get(channel_index)
            .and_then(|value| *value);
        let is_extra = channel_index >= first_extra;
        let is_declared_spot = display.is_some_and(|info| info.is_spot());
        if !is_extra && !is_declared_spot {
            continue;
        }

        let plane = &planes[channel_index];
        if pixel >= plane.len() {
            continue;
        }
        let coverage = if is_extra {
            tiff_io::extra_channel_preview_coverage(metadata, channel_index, plane[pixel])
        } else {
            // A declared Spot that sits inside a Multichannel base range is not
            // polarity-normalized by tiff_io, so preserve Photoshop's raw Spot
            // convention here: white = no ink, black = full ink.
            1.0 - plane[pixel] as f32 / u16::MAX as f32
        };
        if coverage <= 0.001 {
            continue;
        }

        match display {
            Some(info) if info.is_spot() => {
                let tint = info
                    .rgb
                    .unwrap_or(DIAGNOSTIC_TINTS[channel_index % DIAGNOSTIC_TINTS.len()]);
                blend_tint(rgb, tint, (coverage * info.solidity).clamp(0.0, 1.0));
            }
            Some(_) => {
                // Known Alpha/protected display channels are not printing inks.
            }
            None if is_extra => {
                let tint = DIAGNOSTIC_TINTS[(channel_index - first_extra) % DIAGNOSTIC_TINTS.len()];
                blend_tint(rgb, tint, (coverage * 0.72).clamp(0.0, 0.72));
            }
            None => {}
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
    use crate::tiff_io::{ColorModel, PhotoshopChannelDisplay, TiffMetadata};
    use std::collections::BTreeMap;
    use windows_shade_editor::design_source_preview::DesignSourcePreview;
    use windows_shade_editor::png_source::{DecodedPngSource, PngSourceModel};

    fn face(values: Vec<u16>) -> PreviewFace {
        PreviewFace {
            metadata: TiffMetadata {
                width: values.len() as u32,
                height: 1,
                bit_depth: 16,
                samples_per_pixel: 1,
                base_channel_count: 1,
                color_model: ColorModel::Gray,
                non_cmyk_separated: false,
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

    #[test]
    fn multichannel_spot_preview_uses_display_color_and_solidity() {
        let plane = vec![u16::MAX];
        let face = PreviewFace {
            metadata: TiffMetadata {
                width: 1,
                height: 1,
                bit_depth: 16,
                samples_per_pixel: 1,
                base_channel_count: 0,
                color_model: ColorModel::Other,
                non_cmyk_separated: false,
                channel_names: vec!["Spot Blue".to_owned()],
                channel_display_info: vec![Some(PhotoshopChannelDisplay {
                    rgb: Some([0.0, 0.25, 1.0]),
                    solidity: 0.5,
                    kind: 2,
                })],
                compression: None,
                predictor: None,
                orientation: None,
                icc_profile: None,
                photoshop_resources: None,
                photoshop_image_source_data: None,
            },
            width: 1,
            height: 1,
            channels: vec![plane.clone()],
            histograms: vec![histogram(&plane)],
        };

        let rgba = rgba_from_planes(&face, &face.channels, None);
        assert_eq!(rgba, vec![128, 159, 255, 255]);
    }

    #[test]
    fn spot_declared_inside_multichannel_base_range_is_still_composited() {
        let plane = vec![0u16];
        let face = PreviewFace {
            metadata: TiffMetadata {
                width: 1,
                height: 1,
                bit_depth: 16,
                samples_per_pixel: 1,
                base_channel_count: 1,
                color_model: ColorModel::Other,
                non_cmyk_separated: false,
                channel_names: vec!["Spot Red".to_owned()],
                channel_display_info: vec![Some(PhotoshopChannelDisplay {
                    rgb: Some([1.0, 0.0, 0.0]),
                    solidity: 0.4,
                    kind: 2,
                })],
                compression: None,
                predictor: None,
                orientation: None,
                icc_profile: None,
                photoshop_resources: None,
                photoshop_image_source_data: None,
            },
            width: 1,
            height: 1,
            channels: vec![plane.clone()],
            histograms: vec![histogram(&plane)],
        };

        let rgba = rgba_from_planes(&face, &face.channels, None);
        assert_eq!(rgba, vec![255, 153, 153, 255]);
    }

    #[test]
    fn master_levels_and_curve_stack_without_overwriting_channel_controls() {
        let face = face(vec![32768]);
        let mut local = ChannelAdjustment::default();
        local.levels.gamma = 1.6;
        local.curve = Curve {
            midpoint_enabled: true,
            midpoint_input: 0.5,
            midpoint: 0.62,
            ..Curve::default()
        };
        let local_before = local.clone();
        let mut project = project(local);
        let mut master = ChannelAdjustment::default();
        master.levels.gamma = 0.82;
        master.curve = Curve {
            midpoint_enabled: true,
            midpoint_input: 0.5,
            midpoint: 0.44,
            ..Curve::default()
        };
        project
            .adjustments
            .insert(MASTER_ADJUSTMENT_KEY.to_owned(), master.clone());

        let (planes, _) = adjusted_planes_with_stats(&face, &project);
        let raw = 32768.0 / 65535.0;
        let after_local_levels = apply_levels(raw, local_before.levels);
        let after_master_levels = apply_levels(after_local_levels, master.levels);
        let after_local_curve = apply_curve(after_master_levels, local_before.curve);
        let expected = apply_curve(after_local_curve, master.curve);
        let actual = planes[0][0] as f32 / 65535.0;
        assert!((actual - expected).abs() < 2.0 / 65535.0);
        assert_eq!(project.adjustments.get("Ink").unwrap(), &local_before);
    }

    #[test]
    fn png_design_preview_uses_shared_adjustment_and_rgb_render_boundary() {
        let decoded = DecodedPngSource {
            width: 1,
            height: 1,
            bit_depth: 16,
            model: PngSourceModel::Rgb,
            samples: vec![u16::MAX, 32768, 0],
            alpha: Some(vec![12345]),
            icc_profile: None,
            declares_srgb: false,
        };
        let preview = DesignSourcePreview::from_png(&decoded, 512).expect("PNG preview");
        let project = ShadeProject::default();
        let (planes, stats) = adjusted_planes_with_stats(&preview, &project);
        assert_eq!(planes, preview.channels);
        assert_eq!(stats.len(), 3);
        assert_eq!(
            rgba_from_planes(&preview, &planes, None),
            vec![255, 128, 0, 255]
        );
        assert_eq!(preview.alpha.as_deref(), Some(&[12345][..]));
    }
}
