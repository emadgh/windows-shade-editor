use std::path::Path;

use lcms2::{
    ColorSpaceSignature, Flags, Intent, PixelFormat, Profile, ProfileClassSignature, Transform,
};

use crate::color_conversion::ConversionRenderingIntent;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IccSourceModel {
    Rgb,
    Cmyk,
}

pub enum RuntimeIccProfile<'a> {
    Embedded(&'a [u8]),
    File(&'a Path),
}

impl RuntimeIccProfile<'_> {
    pub(crate) fn open(&self, role: &str) -> Result<Profile, String> {
        match self {
            Self::Embedded(bytes) => Profile::new_icc(bytes).map_err(|err| {
                format!("Cannot open {role} ICC profile from embedded bytes: {err}")
            }),
            Self::File(path) => Profile::new_file(path)
                .map_err(|err| format!("Cannot open {role} ICC profile {}: {err}", path.display())),
        }
    }
}

enum ProductionTransform {
    RgbToCmyk(Transform<[u16; 3], [u16; 4]>),
    CmykToCmyk(Transform<[u16; 4], [u16; 4]>),
}

/// Production ICC transform used by Color Conversion, not by display preview.
///
/// This first engine slice deliberately targets 4-channel CMYK only. The API is
/// chunk-oriented so export/conversion workers can stream large TIFFs without
/// allocating an additional full-resolution image. N-channel output is added by
/// extending this production engine, not by reusing preview RGB transforms.
pub struct ProductionCmykTransform {
    transform: ProductionTransform,
    source_model: IccSourceModel,
    black_point_compensation: bool,
    rendering_intent: ConversionRenderingIntent,
}

impl ProductionCmykTransform {
    pub fn new(
        source_model: IccSourceModel,
        source_profile: RuntimeIccProfile<'_>,
        target_profile: RuntimeIccProfile<'_>,
        rendering_intent: ConversionRenderingIntent,
        black_point_compensation: bool,
    ) -> Result<Self, String> {
        let source = source_profile.open("source")?;
        let target = target_profile.open("target")?;

        let expected_source_space = match source_model {
            IccSourceModel::Rgb => ColorSpaceSignature::RgbData,
            IccSourceModel::Cmyk => ColorSpaceSignature::CmykData,
        };
        if source.color_space() != expected_source_space {
            return Err(format!(
                "Source ICC color space does not match requested {:?} source model.",
                source_model
            ));
        }
        if target.color_space() != ColorSpaceSignature::CmykData {
            return Err("Target ICC profile is not a CMYK profile.".to_owned());
        }
        if target.device_class() != ProfileClassSignature::OutputClass {
            return Err("Target CMYK profile must be an output/printer profile.".to_owned());
        }

        let intent = to_lcms_intent(rendering_intent);
        let transform = match source_model {
            IccSourceModel::Rgb => {
                let result = if black_point_compensation {
                    Transform::new_flags(
                        &source,
                        PixelFormat::RGB_16,
                        &target,
                        PixelFormat::CMYK_16,
                        intent,
                        Flags::BLACKPOINT_COMPENSATION,
                    )
                } else {
                    Transform::new(
                        &source,
                        PixelFormat::RGB_16,
                        &target,
                        PixelFormat::CMYK_16,
                        intent,
                    )
                };
                result.map(ProductionTransform::RgbToCmyk).map_err(|err| {
                    format!("Cannot create production RGB→CMYK ICC transform: {err}")
                })?
            }
            IccSourceModel::Cmyk => {
                let result = if black_point_compensation {
                    Transform::new_flags(
                        &source,
                        PixelFormat::CMYK_16,
                        &target,
                        PixelFormat::CMYK_16,
                        intent,
                        Flags::BLACKPOINT_COMPENSATION,
                    )
                } else {
                    Transform::new(
                        &source,
                        PixelFormat::CMYK_16,
                        &target,
                        PixelFormat::CMYK_16,
                        intent,
                    )
                };
                result.map(ProductionTransform::CmykToCmyk).map_err(|err| {
                    format!("Cannot create production CMYK→CMYK ICC transform: {err}")
                })?
            }
        };

        Ok(Self {
            transform,
            source_model,
            black_point_compensation,
            rendering_intent,
        })
    }

    pub fn source_model(&self) -> IccSourceModel {
        self.source_model
    }

    pub fn black_point_compensation(&self) -> bool {
        self.black_point_compensation
    }

    pub fn rendering_intent(&self) -> ConversionRenderingIntent {
        self.rendering_intent
    }

    /// Convert one RGB chunk into CMYK. `source` and `destination` must contain
    /// the same number of pixels. Callers own chunk sizing and streaming policy.
    pub fn transform_rgb_chunk(
        &self,
        source: &[[u16; 3]],
        destination: &mut [[u16; 4]],
    ) -> Result<(), String> {
        validate_chunk_lengths(source.len(), destination.len())?;
        let ProductionTransform::RgbToCmyk(transform) = &self.transform else {
            return Err(
                "This production transform was not created for RGB source data.".to_owned(),
            );
        };
        transform.transform_pixels(source, destination);
        Ok(())
    }

    /// Convert one CMYK chunk into another characterized CMYK output space.
    /// This path is useful for explicit device-to-output conversion and shares
    /// the same bounded-memory contract as RGB→CMYK.
    pub fn transform_cmyk_chunk(
        &self,
        source: &[[u16; 4]],
        destination: &mut [[u16; 4]],
    ) -> Result<(), String> {
        validate_chunk_lengths(source.len(), destination.len())?;
        let ProductionTransform::CmykToCmyk(transform) = &self.transform else {
            return Err(
                "This production transform was not created for CMYK source data.".to_owned(),
            );
        };
        transform.transform_pixels(source, destination);
        Ok(())
    }
}

pub(crate) fn validate_chunk_lengths(
    source_len: usize,
    destination_len: usize,
) -> Result<(), String> {
    if source_len != destination_len {
        return Err(format!(
            "ICC conversion chunk length mismatch: {source_len} source pixels, {destination_len} destination pixels."
        ));
    }
    Ok(())
}

pub(crate) fn to_lcms_intent(intent: ConversionRenderingIntent) -> Intent {
    match intent {
        ConversionRenderingIntent::Perceptual => Intent::Perceptual,
        ConversionRenderingIntent::RelativeColorimetric => Intent::RelativeColorimetric,
        ConversionRenderingIntent::Saturation => Intent::Saturation,
        ConversionRenderingIntent::AbsoluteColorimetric => Intent::AbsoluteColorimetric,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_lengths_must_match() {
        assert!(validate_chunk_lengths(1024, 1024).is_ok());
        let error = validate_chunk_lengths(1024, 1000).expect_err("mismatch must fail");
        assert!(error.contains("1024 source pixels"));
    }

    #[test]
    fn conversion_intents_map_to_littlecms_intents() {
        assert_eq!(
            to_lcms_intent(ConversionRenderingIntent::Perceptual),
            Intent::Perceptual
        );
        assert_eq!(
            to_lcms_intent(ConversionRenderingIntent::RelativeColorimetric),
            Intent::RelativeColorimetric
        );
        assert_eq!(
            to_lcms_intent(ConversionRenderingIntent::Saturation),
            Intent::Saturation
        );
        assert_eq!(
            to_lcms_intent(ConversionRenderingIntent::AbsoluteColorimetric),
            Intent::AbsoluteColorimetric
        );
    }
}
