use crate::model::{ShadeProject, apply_curve, apply_levels};
use crate::tiff_io::{ColorModel, PreviewFace};

pub fn adjusted_planes(face: &PreviewFace, project: &ShadeProject) -> Vec<Vec<u16>> {
    let channel_count = face.channels.len();
    if channel_count == 0 {
        return Vec::new();
    }
    let pixel_count = face.channels[0].len();
    let names = &face.metadata.channel_names;

    let mut prepared = (0..channel_count)
        .map(|_| vec![0.0f32; pixel_count])
        .collect::<Vec<_>>();

    for channel in 0..channel_count {
        let adjustment = project.adjustments.get(&names[channel]);
        for pixel in 0..pixel_count {
            let raw = face.channels[channel][pixel] as f32 / 65535.0;
            prepared[channel][pixel] = if let Some(adjustment) = adjustment {
                if adjustment.enabled {
                    apply_curve(apply_levels(raw, adjustment.levels), adjustment.curve)
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
                    mixed
                }
            } else {
                prepared[out_channel][pixel]
            };
            output[out_channel][pixel] = (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
        }
    }

    output
}

/// Build a display preview. This is not a press proof, but it must preserve the
/// image geometry and base color model. v0.1 incorrectly treated every 4+
/// channel file as CMYK, which corrupted RGB+spot previews.
pub fn rgba_from_planes(
    face: &PreviewFace,
    planes: &[Vec<u16>],
    solo_channel: Option<usize>,
) -> Vec<u8> {
    let width = face.width;
    let height = face.height;
    let pixel_count = width.saturating_mul(height);
    let mut rgba = Vec::with_capacity(pixel_count.saturating_mul(4));

    if let Some(channel) = solo_channel.filter(|index| *index < planes.len()) {
        let invert = face.metadata.color_model == ColorModel::Cmyk
            && channel < face.metadata.base_channel_count;
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
        let coverage = 1.0 - plane[pixel] as f32 / 65535.0;
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
                // Photoshop Solidity affects composite/on-screen simulation only,
                // not the actual separation. Honor the stored value exactly.
                blend_tint(rgb, tint, (coverage * info.solidity).clamp(0.0, 1.0));
            }
            Some(_) => {
                // Known Alpha/protected display channels are not printing inks;
                // do not contaminate the composite preview with them.
            }
            None => {
                // Files without DisplayInfo cannot tell us Spot vs Alpha here.
                // Retain a deterministic engineering tint so extra separations
                // remain visible during diagnosis.
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
