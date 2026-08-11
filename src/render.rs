use crate::model::{ShadeProject, apply_curve, apply_levels};
use crate::tiff_io::PreviewFace;

pub fn adjusted_planes(face: &PreviewFace, project: &ShadeProject) -> Vec<Vec<u16>> {
    let channel_count = face.channels.len();
    if channel_count == 0 { return Vec::new(); }
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
                        let coefficient = adjustment.mixer.coefficients
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

pub fn rgba_from_planes(
    planes: &[Vec<u16>],
    width: usize,
    height: usize,
    solo_channel: Option<usize>,
) -> Vec<u8> {
    let pixel_count = width.saturating_mul(height);
    let mut rgba = Vec::with_capacity(pixel_count.saturating_mul(4));

    if let Some(channel) = solo_channel.filter(|index| *index < planes.len()) {
        for value in &planes[channel] {
            let gray = 255u8.saturating_sub((*value >> 8) as u8);
            rgba.extend_from_slice(&[gray, gray, gray, 255]);
        }
        return rgba;
    }

    if planes.len() >= 4 {
        for pixel in 0..pixel_count {
            let c = planes[0][pixel] as f32 / 65535.0;
            let m = planes[1][pixel] as f32 / 65535.0;
            let y = planes[2][pixel] as f32 / 65535.0;
            let k = planes[3][pixel] as f32 / 65535.0;
            let mut r = 1.0 - (c + k).min(1.0);
            let mut g = 1.0 - (m + k).min(1.0);
            let mut b = 1.0 - (y + k).min(1.0);

            if planes.len() > 4 {
                let strongest_spot = planes[4..]
                    .iter()
                    .map(|plane| plane[pixel] as f32 / 65535.0)
                    .fold(0.0f32, f32::max);
                let neutral_preview = 1.0 - strongest_spot * 0.28;
                r *= neutral_preview;
                g *= neutral_preview;
                b *= neutral_preview;
            }

            rgba.extend_from_slice(&[
                (r.clamp(0.0, 1.0) * 255.0) as u8,
                (g.clamp(0.0, 1.0) * 255.0) as u8,
                (b.clamp(0.0, 1.0) * 255.0) as u8,
                255,
            ]);
        }
    } else if let Some(first) = planes.first() {
        for value in first {
            let gray = 255u8.saturating_sub((*value >> 8) as u8);
            rgba.extend_from_slice(&[gray, gray, gray, 255]);
        }
    }
    rgba
}

pub fn histogram(values: &[u16]) -> [u32; 256] {
    let mut bins = [0u32; 256];
    for value in values {
        let index = usize::from(*value >> 8);
        bins[index] = bins[index].saturating_add(1);
    }
    bins
}
