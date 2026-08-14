from pathlib import Path
import re

ROOT = Path('.')

def read(path):
    return (ROOT / path).read_text(encoding='utf-8')

def write(path, text):
    (ROOT / path).write_text(text, encoding='utf-8', newline='\n')

def replace_once(text, old, new, label):
    count = text.count(old)
    if count != 1:
        raise SystemExit(f'{label}: expected exactly 1 match, found {count}')
    return text.replace(old, new, 1)

# Version bump.
cargo = read('Cargo.toml')
cargo = replace_once(cargo, 'version = "0.15.0"', 'version = "0.15.1"', 'Cargo.toml version')
write('Cargo.toml', cargo)

lock = read('Cargo.lock')
lock, count = re.subn(
    r'(\[\[package\]\]\nname = "windows-shade-editor"\nversion = ")0\.15\.0(")',
    r'\g<1>0.15.1\2',
    lock,
    count=1,
)
if count != 1:
    raise SystemExit(f'Cargo.lock version: expected 1 match, found {count}')
write('Cargo.lock', lock)

# Central Spot polarity policy and preview normalization.
tiff = read('src/tiff_io.rs')
marker = '''#[derive(Clone, Debug)]
pub struct DecodedImage {'''
helpers = '''/// Normalize Photoshop Spot separations to the same working-space polarity as
/// CMYK ink coverage: 0 = no ink, 65535 = full ink. Keep this as a compile-time
/// switch for now so the legacy raw-Spot behavior can be restored easily or
/// exposed as a user setting later without changing the conversion contract.
pub const NORMALIZE_PHOTOSHOP_SPOT_POLARITY: bool = true;

pub fn is_photoshop_spot_channel(metadata: &TiffMetadata, channel: usize) -> bool {
    channel >= metadata.base_channel_count
        && metadata
            .channel_display_info
            .get(channel)
            .and_then(|value| *value)
            .is_some_and(PhotoshopChannelDisplay::is_spot)
}

fn map_spot_polarity(enabled: bool, is_spot: bool, sample: u16) -> u16 {
    if enabled && is_spot {
        u16::MAX - sample
    } else {
        sample
    }
}

/// Convert a raw TIFF sample into Shade Editor's adjustment working space.
pub fn working_sample_from_tiff(metadata: &TiffMetadata, channel: usize, sample: u16) -> u16 {
    map_spot_polarity(
        NORMALIZE_PHOTOSHOP_SPOT_POLARITY,
        is_photoshop_spot_channel(metadata, channel),
        sample,
    )
}

/// Convert a working-space sample back to the TIFF/Photoshop channel polarity.
pub fn tiff_sample_from_working(metadata: &TiffMetadata, channel: usize, sample: u16) -> u16 {
    map_spot_polarity(
        NORMALIZE_PHOTOSHOP_SPOT_POLARITY,
        is_photoshop_spot_channel(metadata, channel),
        sample,
    )
}

/// Coverage used to tint an extra separation in the composite preview. Unknown
/// extras keep the historical mask-style interpretation; declared Photoshop
/// Spot channels use normalized ink coverage when the policy is enabled.
pub fn extra_channel_preview_coverage(
    metadata: &TiffMetadata,
    channel: usize,
    working_sample: u16,
) -> f32 {
    let value = working_sample as f32 / u16::MAX as f32;
    if is_photoshop_spot_channel(metadata, channel) && NORMALIZE_PHOTOSHOP_SPOT_POLARITY {
        value
    } else {
        1.0 - value
    }
}

/// Solo preview should render ink coverage as white at zero and black at full.
pub fn solo_channel_uses_ink_coverage(metadata: &TiffMetadata, channel: usize) -> bool {
    (metadata.color_model == ColorModel::Cmyk && channel < metadata.base_channel_count)
        || (NORMALIZE_PHOTOSHOP_SPOT_POLARITY
            && is_photoshop_spot_channel(metadata, channel))
}

#[derive(Clone, Debug)]
pub struct DecodedImage {'''
tiff = replace_once(tiff, marker, helpers, 'tiff polarity helper insertion')
tiff = replace_once(
    tiff,
    'channels[channel][destination] = samples[source_base + channel];',
    'channels[channel][destination] = working_sample_from_tiff(\n                            &info.metadata,\n                            channel,\n                            samples[source_base + channel],\n                        );',
    'preview sample normalization',
)

spot_tests = r'''

#[cfg(test)]
mod spot_polarity_tests {
    use super::*;

    fn metadata_with_spot() -> TiffMetadata {
        let mut display = vec![None; 5];
        display[4] = Some(PhotoshopChannelDisplay {
            rgb: Some([0.9, 0.2, 0.1]),
            solidity: 1.0,
            kind: 2,
        });
        TiffMetadata {
            width: 1,
            height: 1,
            bit_depth: 16,
            samples_per_pixel: 5,
            base_channel_count: 4,
            color_model: ColorModel::Cmyk,
            channel_names: vec![
                "Cyan".to_owned(),
                "Magenta".to_owned(),
                "Yellow".to_owned(),
                "Black".to_owned(),
                "Spot Red".to_owned(),
            ],
            channel_display_info: display,
            compression: None,
            predictor: None,
            orientation: None,
            icc_profile: None,
            photoshop_resources: None,
            photoshop_image_source_data: None,
        }
    }

    #[test]
    fn spot_polarity_policy_can_be_enabled_or_disabled() {
        let raw = 12_345u16;
        assert_eq!(map_spot_polarity(false, true, raw), raw);
        assert_eq!(map_spot_polarity(true, true, raw), u16::MAX - raw);
        assert_eq!(map_spot_polarity(true, false, raw), raw);
    }

    #[test]
    fn normalized_spot_round_trips_raw_tiff_samples() {
        let metadata = metadata_with_spot();
        for raw in [0u16, 1, 257, 12_345, 32_768, 65_534, u16::MAX] {
            let working = working_sample_from_tiff(&metadata, 4, raw);
            let restored = tiff_sample_from_working(&metadata, 4, working);
            assert_eq!(restored, raw);
        }
    }

    #[test]
    fn equivalent_yellow_and_spot_ink_have_same_normalized_values() {
        let metadata = metadata_with_spot();
        for yellow in [0u16, 8_192, 16_384, 32_768, 49_152, u16::MAX] {
            let spot_raw = u16::MAX - yellow;
            let yellow_working = map_spot_polarity(true, false, yellow);
            let spot_working = map_spot_polarity(true, true, spot_raw);
            assert_eq!(yellow_working, spot_working);
        }
        assert!(is_photoshop_spot_channel(&metadata, 4));
        assert!(!is_photoshop_spot_channel(&metadata, 2));
    }
}
'''
if 'mod spot_polarity_tests {' in tiff:
    raise SystemExit('spot polarity tests already exist')
tiff += spot_tests
write('src/tiff_io.rs', tiff)

# Render preview and histogram in the same working polarity.
render = read('src/render.rs')
render = replace_once(
    render,
    'use crate::tiff_io::{ColorModel, PreviewFace};',
    'use crate::tiff_io::{self, ColorModel, PreviewFace};',
    'render tiff_io import',
)
render = replace_once(
    render,
    '''        let invert = face.metadata.color_model == ColorModel::Cmyk
            && channel < face.metadata.base_channel_count;''',
    '''        let invert = tiff_io::solo_channel_uses_ink_coverage(&face.metadata, channel);''',
    'solo preview ink polarity',
)
render = replace_once(
    render,
    '        let coverage = 1.0 - plane[pixel] as f32 / 65535.0;',
    '''        let coverage =
            tiff_io::extra_channel_preview_coverage(&face.metadata, channel_index, plane[pixel]);''',
    'composite extra coverage',
)
write('src/render.rs', render)

# Export: normalize before Levels/Mixer/Curve and restore Photoshop polarity on write.
export = read('src/export_v6.rs')
export = replace_once(
    export,
    '''use crate::tiff_io::{
    ColorModel, StreamInfo, TiffMetadata, decode_full, for_each_decoded_region,
    for_each_decoded_strip, stream_info,
};''',
    '''use crate::tiff_io::{
    ColorModel, StreamInfo, TiffMetadata, decode_full, for_each_decoded_region,
    for_each_decoded_strip, stream_info, tiff_sample_from_working, working_sample_from_tiff,
};''',
    'export tiff helper imports',
)
export = replace_once(
    export,
    '                let raw = decoded.samples[base + channel] as f32 / 65535.0;',
    '''                let raw = working_sample_from_tiff(
                    &decoded.metadata,
                    channel,
                    decoded.samples[base + channel],
                ) as f32
                    / 65535.0;''',
    'full export working input',
)
export = replace_once(
    export,
    '                output[base + out_channel] = (value.clamp(0.0, 1.0) * 65535.0).round() as u16;',
    '''                let working = (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
                output[base + out_channel] =
                    tiff_sample_from_working(&decoded.metadata, out_channel, working);''',
    'full export TIFF polarity restore',
)
old_adjusted = '''fn adjusted_strip(
    input: &[u16],
    channels: usize,
    names: &[String],
    project: &ShadeProject,
) -> Vec<u16> {
    let pixel_count = input.len() / channels.max(1);
    let mut output = vec![0u16; pixel_count.saturating_mul(channels)];
    let mut prepared = vec![0.0f32; channels];
    for pixel in 0..pixel_count {
        let base = pixel * channels;
        for channel in 0..channels {
            let raw = input[base + channel] as f32 / 65535.0;
            prepared[channel] = match project.adjustments.get(&names[channel]) {
                Some(adjustment) if adjustment.enabled => {
                    apply_levels(raw, adjustment.levels)
                }
                _ => raw,
            };
        }
        for out_channel in 0..channels {
            let value = match project.adjustments.get(&names[out_channel]) {
                Some(adjustment) if adjustment.enabled => {
                    let mut mixed = adjustment.mixer.constant;
                    for source_channel in 0..channels {
                        let coefficient = adjustment
                            .mixer
                            .coefficients
                            .get(&names[source_channel])
                            .copied()
                            .unwrap_or(if source_channel == out_channel {
                                1.0
                            } else {
                                0.0
                            });
                        mixed += prepared[source_channel] * coefficient;
                    }
                    apply_curve(mixed, adjustment.curve)
                }
                _ => prepared[out_channel],
            };
            output[base + out_channel] = (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
        }
    }
    output
}'''
new_adjusted = '''fn adjusted_strip(
    input: &[u16],
    metadata: &TiffMetadata,
    project: &ShadeProject,
) -> Vec<u16> {
    let channels = metadata.samples_per_pixel;
    let names = &metadata.channel_names;
    let pixel_count = input.len() / channels.max(1);
    let mut output = vec![0u16; pixel_count.saturating_mul(channels)];
    let mut prepared = vec![0.0f32; channels];
    for pixel in 0..pixel_count {
        let base = pixel * channels;
        for channel in 0..channels {
            let raw = working_sample_from_tiff(metadata, channel, input[base + channel]) as f32
                / 65535.0;
            prepared[channel] = match project.adjustments.get(&names[channel]) {
                Some(adjustment) if adjustment.enabled => {
                    apply_levels(raw, adjustment.levels)
                }
                _ => raw,
            };
        }
        for out_channel in 0..channels {
            let value = match project.adjustments.get(&names[out_channel]) {
                Some(adjustment) if adjustment.enabled => {
                    let mut mixed = adjustment.mixer.constant;
                    for source_channel in 0..channels {
                        let coefficient = adjustment
                            .mixer
                            .coefficients
                            .get(&names[source_channel])
                            .copied()
                            .unwrap_or(if source_channel == out_channel {
                                1.0
                            } else {
                                0.0
                            });
                        mixed += prepared[source_channel] * coefficient;
                    }
                    apply_curve(mixed, adjustment.curve)
                }
                _ => prepared[out_channel],
            };
            let working = (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
            output[base + out_channel] =
                tiff_sample_from_working(metadata, out_channel, working);
        }
    }
    output
}'''
export = replace_once(export, old_adjusted, new_adjusted, 'streaming adjustment pipeline')

# Three production call sites.
for label, old, new in [
    ('u8 strip call', 'adjusted_strip(input, channels, names, project)', 'adjusted_strip(input, &stream.metadata, project)'),
    ('u16 strip call', 'adjusted_strip(input, channels, names, project)', 'adjusted_strip(input, &stream.metadata, project)'),
    ('region call', 'adjusted_strip(input, channels, names, project)', 'adjusted_strip(input, metadata, project)'),
]:
    export = replace_once(export, old, new, label)

# Update the existing pipeline test and add Spot export regression coverage.
export = replace_once(
    export,
    '        let output = adjusted_strip(&input, 2, &names, &project);',
    '''        let metadata = test_metadata(&names, 2, vec![None; 2]);
        let output = adjusted_strip(&input, &metadata, &project);''',
    'pipeline test adjusted_strip call',
)
insert_test_helper_marker = '''    #[test]
    fn adjustment_pipeline_is_levels_then_mixer_then_curve() {'''
test_helpers = '''    fn test_metadata(
        names: &[String],
        base_channel_count: usize,
        channel_display_info: Vec<Option<crate::tiff_io::PhotoshopChannelDisplay>>,
    ) -> TiffMetadata {
        TiffMetadata {
            width: 1,
            height: 1,
            bit_depth: 16,
            samples_per_pixel: names.len(),
            base_channel_count,
            color_model: ColorModel::Cmyk,
            channel_names: names.to_vec(),
            channel_display_info,
            compression: None,
            predictor: None,
            orientation: None,
            icc_profile: None,
            photoshop_resources: None,
            photoshop_image_source_data: None,
        }
    }

    #[test]
    fn adjustment_pipeline_is_levels_then_mixer_then_curve() {'''
export = replace_once(export, insert_test_helper_marker, test_helpers, 'export test metadata helper')

spot_export_test = '''

    #[test]
    fn spot_zero_working_coverage_exports_as_no_ink_with_photoshop_polarity() {
        let names = vec![
            "Cyan".to_owned(),
            "Magenta".to_owned(),
            "Yellow".to_owned(),
            "Black".to_owned(),
            "Spot Red".to_owned(),
        ];
        let mut display = vec![None; 5];
        display[4] = Some(crate::tiff_io::PhotoshopChannelDisplay {
            rgb: Some([0.9, 0.2, 0.1]),
            solidity: 1.0,
            kind: 2,
        });
        let metadata = test_metadata(&names, 4, display);
        let mut project = ShadeProject::default();
        project.ensure_channels(&names);
        project.adjustments.get_mut("Yellow").unwrap().levels.output_white = 0.0;
        project.adjustments.get_mut("Spot Red").unwrap().levels.output_white = 0.0;

        // Equivalent 50% ink: CMYK raw uses direct coverage; Photoshop Spot raw is inverted.
        let input = [0u16, 0, 32_768, 0, u16::MAX - 32_768];
        let output = adjusted_strip(&input, &metadata, &project);

        assert_eq!(output[2], 0, "Yellow 0 working coverage must export as no ink");
        if crate::tiff_io::NORMALIZE_PHOTOSHOP_SPOT_POLARITY {
            assert_eq!(
                output[4],
                u16::MAX,
                "Spot 0 working coverage must be restored to Photoshop's no-ink raw value"
            );
        } else {
            assert_eq!(output[4], 0, "legacy Spot polarity must remain available when disabled");
        }
    }
'''
marker_end_test = '''    fn apply_dynamic_u8_predictor(data: &mut [u8], width: usize, height: usize, channels: usize) {'''
export = replace_once(export, marker_end_test, spot_export_test + '\n    fn apply_dynamic_u8_predictor(data: &mut [u8], width: usize, height: usize, channels: usize) {', 'spot export regression test')
write('src/export_v6.rs', export)

print('v0.15.1 Spot polarity normalization patch applied')
