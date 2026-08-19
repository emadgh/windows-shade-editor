use lcms2::{
    ColorSpaceSignatureExt, Flags, Intent, PixelFormat, Profile, ProfileClassSignature, Transform,
};

use crate::color_conversion::ConversionRenderingIntent;
use crate::icc_conversion::{IccSourceModel, RuntimeIccProfile};

enum NChannelTransform<const N: usize> {
    Rgb(Transform<[u16; 3], [u16; N]>),
    Cmyk(Transform<[u16; 4], [u16; N]>),
}

/// Production ICC transform for characterized 5..=12 channel targets.
/// Keeping N in the Rust type prevents topology mismatches at call sites.
pub struct ProductionNChannelTransform<const N: usize> {
    transform: NChannelTransform<N>,
    source_model: IccSourceModel,
    rendering_intent: ConversionRenderingIntent,
    black_point_compensation: bool,
}

impl<const N: usize> ProductionNChannelTransform<N> {
    pub fn new(
        source_model: IccSourceModel,
        source_profile: RuntimeIccProfile<'_>,
        target_profile: RuntimeIccProfile<'_>,
        rendering_intent: ConversionRenderingIntent,
        black_point_compensation: bool,
    ) -> Result<Self, String> {
        let output_format = nchannel_pixel_format::<N>()?;
        let source = open_profile(source_profile, "source")?;
        let target = open_profile(target_profile, "target")?;

        let expected_source_channels = match source_model {
            IccSourceModel::Rgb => 3,
            IccSourceModel::Cmyk => 4,
        };
        if source.color_space().channels() != expected_source_channels {
            return Err(format!(
                "Source ICC declares {} channels but {:?} source data requires {expected_source_channels}.",
                source.color_space().channels(),
                source_model
            ));
        }
        if target.device_class() != ProfileClassSignature::OutputClass {
            return Err("N-channel target profile must be an output/printer profile.".to_owned());
        }
        if target.color_space().channels() != N as u32 {
            return Err(format!(
                "Target ICC declares {} channels but this transform requires {N}.",
                target.color_space().channels()
            ));
        }

        let intent = to_lcms_intent(rendering_intent);
        let transform = match source_model {
            IccSourceModel::Rgb => {
                let result: lcms2::LCMSResult<Transform<[u16; 3], [u16; N]>> =
                    if black_point_compensation {
                        Transform::new_flags(
                            &source,
                            PixelFormat::RGB_16,
                            &target,
                            output_format,
                            intent,
                            Flags::BLACKPOINT_COMPENSATION,
                        )
                    } else {
                        Transform::new(&source, PixelFormat::RGB_16, &target, output_format, intent)
                    };
                result.map(NChannelTransform::Rgb).map_err(|err| {
                    format!("Cannot create production RGB→{N}C ICC transform: {err}")
                })?
            }
            IccSourceModel::Cmyk => {
                let result: lcms2::LCMSResult<Transform<[u16; 4], [u16; N]>> =
                    if black_point_compensation {
                        Transform::new_flags(
                            &source,
                            PixelFormat::CMYK_16,
                            &target,
                            output_format,
                            intent,
                            Flags::BLACKPOINT_COMPENSATION,
                        )
                    } else {
                        Transform::new(
                            &source,
                            PixelFormat::CMYK_16,
                            &target,
                            output_format,
                            intent,
                        )
                    };
                result.map(NChannelTransform::Cmyk).map_err(|err| {
                    format!("Cannot create production CMYK→{N}C ICC transform: {err}")
                })?
            }
        };

        Ok(Self {
            transform,
            source_model,
            rendering_intent,
            black_point_compensation,
        })
    }

    pub fn channel_count(&self) -> usize {
        N
    }

    pub fn source_model(&self) -> IccSourceModel {
        self.source_model
    }

    pub fn rendering_intent(&self) -> ConversionRenderingIntent {
        self.rendering_intent
    }

    pub fn black_point_compensation(&self) -> bool {
        self.black_point_compensation
    }

    pub fn transform_rgb_chunk(
        &self,
        source: &[[u16; 3]],
        destination: &mut [[u16; N]],
    ) -> Result<(), String> {
        validate_lengths(source.len(), destination.len())?;
        let NChannelTransform::Rgb(transform) = &self.transform else {
            return Err(format!(
                "This {N}-channel transform was not created for RGB source data."
            ));
        };
        transform.transform_pixels(source, destination);
        Ok(())
    }

    pub fn transform_cmyk_chunk(
        &self,
        source: &[[u16; 4]],
        destination: &mut [[u16; N]],
    ) -> Result<(), String> {
        validate_lengths(source.len(), destination.len())?;
        let NChannelTransform::Cmyk(transform) = &self.transform else {
            return Err(format!(
                "This {N}-channel transform was not created for CMYK source data."
            ));
        };
        transform.transform_pixels(source, destination);
        Ok(())
    }
}

fn open_profile(profile: RuntimeIccProfile<'_>, role: &str) -> Result<Profile, String> {
    match profile {
        RuntimeIccProfile::Embedded(bytes) => Profile::new_icc(bytes)
            .map_err(|err| format!("Cannot open {role} ICC profile from embedded bytes: {err}")),
        RuntimeIccProfile::File(path) => Profile::new_file(path)
            .map_err(|err| format!("Cannot open {role} ICC profile {}: {err}", path.display())),
    }
}

pub(crate) fn nchannel_pixel_format<const N: usize>() -> Result<PixelFormat, String> {
    match N {
        5 => Ok(PixelFormat::CMYK5_16),
        6 => Ok(PixelFormat::CMYK6_16),
        7 => Ok(PixelFormat::CMYK7_16),
        8 => Ok(PixelFormat::CMYK8_16),
        9 => Ok(PixelFormat::CMYK9_16),
        10 => Ok(PixelFormat::CMYK10_16),
        11 => Ok(PixelFormat::CMYK11_16),
        12 => Ok(PixelFormat::CMYK12_16),
        _ => Err(format!(
            "Production ICC N-channel output currently supports 5..=12 channels; requested {N}."
        )),
    }
}

fn validate_lengths(source_len: usize, destination_len: usize) -> Result<(), String> {
    if source_len != destination_len {
        return Err(format!(
            "N-channel ICC chunk length mismatch: {source_len} source pixels, {destination_len} destination pixels."
        ));
    }
    Ok(())
}

fn to_lcms_intent(intent: ConversionRenderingIntent) -> Intent {
    match intent {
        ConversionRenderingIntent::Perceptual => Intent::Perceptual,
        ConversionRenderingIntent::RelativeColorimetric => Intent::RelativeColorimetric,
        ConversionRenderingIntent::Saturation => Intent::Saturation,
        ConversionRenderingIntent::AbsoluteColorimetric => Intent::AbsoluteColorimetric,
    }
}

pub type Production5ChannelTransform = ProductionNChannelTransform<5>;
pub type Production6ChannelTransform = ProductionNChannelTransform<6>;
pub type Production7ChannelTransform = ProductionNChannelTransform<7>;
pub type Production8ChannelTransform = ProductionNChannelTransform<8>;
pub type Production9ChannelTransform = ProductionNChannelTransform<9>;
pub type Production10ChannelTransform = ProductionNChannelTransform<10>;
pub type Production11ChannelTransform = ProductionNChannelTransform<11>;
pub type Production12ChannelTransform = ProductionNChannelTransform<12>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_channel_counts_map_to_distinct_formats() {
        assert_eq!(nchannel_pixel_format::<5>().unwrap(), PixelFormat::CMYK5_16);
        assert_eq!(nchannel_pixel_format::<7>().unwrap(), PixelFormat::CMYK7_16);
        assert_eq!(
            nchannel_pixel_format::<12>().unwrap(),
            PixelFormat::CMYK12_16
        );
    }

    #[test]
    fn unsupported_channel_counts_are_rejected_before_profile_transform() {
        let error =
            nchannel_pixel_format::<13>().expect_err("13C is not mapped by the pinned binding");
        assert!(error.contains("5..=12"));
    }

    #[test]
    fn chunk_lengths_are_strict() {
        assert!(validate_lengths(64, 64).is_ok());
        assert!(validate_lengths(64, 63).is_err());
    }
}
