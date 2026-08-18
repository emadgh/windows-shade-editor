use lcms2::{
    CIExyY, ColorSpaceSignature, Flags, GlobalContext, PixelFormat, Profile, Transform,
};

use crate::color_conversion::ConversionRenderingIntent;
use crate::icc_conversion::{
    IccSourceModel, RuntimeIccProfile, to_lcms_intent, validate_chunk_lengths,
};

pub type PcsLabPixel = [f64; 3];

enum ProductionLabTransform {
    Rgb(Transform<[u16; 3], PcsLabPixel>),
    Cmyk(Transform<[u16; 4], PcsLabPixel>),
}

/// Production Source-ICC → CIE Lab transform for the Custom Optimizer path.
///
/// This is deliberately separate from display/soft-proof transforms. The Lab
/// destination is a LittleCMS v4 D50 Lab identity profile and `Lab_DBL` output,
/// so the Custom Optimizer receives physical CIE L*, a*, b* coordinates without
/// an application-defined integer encoding step.
pub struct ProductionPcsLabTransform {
    transform: ProductionLabTransform,
    source_model: IccSourceModel,
    rendering_intent: ConversionRenderingIntent,
    black_point_compensation: bool,
}

impl ProductionPcsLabTransform {
    pub fn new(
        source_model: IccSourceModel,
        source_profile: RuntimeIccProfile<'_>,
        rendering_intent: ConversionRenderingIntent,
        black_point_compensation: bool,
    ) -> Result<Self, String> {
        let source = source_profile.open("source")?;
        let expected_source_space = match source_model {
            IccSourceModel::Rgb => ColorSpaceSignature::RgbData,
            IccSourceModel::Cmyk => ColorSpaceSignature::CmykData,
        };
        if source.color_space() != expected_source_space {
            return Err(format!(
                "Source ICC color space does not match requested {:?} source model for production Lab conversion.",
                source_model
            ));
        }

        let lab = Profile::new_lab4_context(GlobalContext::new(), CIExyY::d50())
            .map_err(|err| format!("Cannot create production D50 Lab identity profile: {err}"))?;
        let intent = to_lcms_intent(rendering_intent);
        let transform = match source_model {
            IccSourceModel::Rgb => {
                let result = if black_point_compensation {
                    Transform::new_flags(
                        &source,
                        PixelFormat::RGB_16,
                        &lab,
                        PixelFormat::Lab_DBL,
                        intent,
                        Flags::BLACKPOINT_COMPENSATION,
                    )
                } else {
                    Transform::new(
                        &source,
                        PixelFormat::RGB_16,
                        &lab,
                        PixelFormat::Lab_DBL,
                        intent,
                    )
                };
                result
                    .map(ProductionLabTransform::Rgb)
                    .map_err(|err| format!("Cannot create production RGB→Lab ICC transform: {err}"))?
            }
            IccSourceModel::Cmyk => {
                let result = if black_point_compensation {
                    Transform::new_flags(
                        &source,
                        PixelFormat::CMYK_16,
                        &lab,
                        PixelFormat::Lab_DBL,
                        intent,
                        Flags::BLACKPOINT_COMPENSATION,
                    )
                } else {
                    Transform::new(
                        &source,
                        PixelFormat::CMYK_16,
                        &lab,
                        PixelFormat::Lab_DBL,
                        intent,
                    )
                };
                result
                    .map(ProductionLabTransform::Cmyk)
                    .map_err(|err| format!("Cannot create production CMYK→Lab ICC transform: {err}"))?
            }
        };

        Ok(Self {
            transform,
            source_model,
            rendering_intent,
            black_point_compensation,
        })
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
        destination: &mut [PcsLabPixel],
    ) -> Result<(), String> {
        validate_chunk_lengths(source.len(), destination.len())?;
        let ProductionLabTransform::Rgb(transform) = &self.transform else {
            return Err("This production Lab transform was not created for RGB source data.".to_owned());
        };
        transform.transform_pixels(source, destination);
        validate_lab_chunk(destination)
    }

    pub fn transform_cmyk_chunk(
        &self,
        source: &[[u16; 4]],
        destination: &mut [PcsLabPixel],
    ) -> Result<(), String> {
        validate_chunk_lengths(source.len(), destination.len())?;
        let ProductionLabTransform::Cmyk(transform) = &self.transform else {
            return Err("This production Lab transform was not created for CMYK source data.".to_owned());
        };
        transform.transform_pixels(source, destination);
        validate_lab_chunk(destination)
    }
}

fn validate_lab_chunk(pixels: &[PcsLabPixel]) -> Result<(), String> {
    for (index, [l, a, b]) in pixels.iter().copied().enumerate() {
        if !l.is_finite() || !a.is_finite() || !b.is_finite() {
            return Err(format!(
                "Production ICC→Lab transform produced non-finite Lab at pixel {index}."
            ));
        }
        if !(-1.0e-9..=100.0 + 1.0e-9).contains(&l) {
            return Err(format!(
                "Production ICC→Lab transform produced out-of-range L* {l} at pixel {index}."
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn srgb_bytes() -> Vec<u8> {
        Profile::new_srgb().icc().unwrap()
    }

    #[test]
    fn rgb_srgb_white_black_and_red_transform_to_finite_d50_lab() {
        let bytes = srgb_bytes();
        let transform = ProductionPcsLabTransform::new(
            IccSourceModel::Rgb,
            RuntimeIccProfile::Embedded(&bytes),
            ConversionRenderingIntent::RelativeColorimetric,
            false,
        )
        .unwrap();
        let source = [[u16::MAX; 3], [0; 3], [u16::MAX, 0, 0]];
        let mut output = [[0.0; 3]; 3];
        transform.transform_rgb_chunk(&source, &mut output).unwrap();

        assert!((output[0][0] - 100.0).abs() < 0.05);
        assert!(output[0][1].abs() < 0.1);
        assert!(output[0][2].abs() < 0.1);
        assert!(output[1][0].abs() < 0.05);
        assert!(output[2].iter().all(|value| value.is_finite()));
        assert!(output[2][0] > output[1][0]);
    }

    #[test]
    fn source_profile_model_mismatch_fails_before_transform_creation() {
        let bytes = srgb_bytes();
        let error = ProductionPcsLabTransform::new(
            IccSourceModel::Cmyk,
            RuntimeIccProfile::Embedded(&bytes),
            ConversionRenderingIntent::RelativeColorimetric,
            false,
        )
        .err()
        .expect("sRGB must not be accepted as CMYK source profile");
        assert!(error.contains("does not match"));
    }

    #[test]
    fn chunk_length_and_source_variant_are_strict() {
        let bytes = srgb_bytes();
        let transform = ProductionPcsLabTransform::new(
            IccSourceModel::Rgb,
            RuntimeIccProfile::Embedded(&bytes),
            ConversionRenderingIntent::RelativeColorimetric,
            false,
        )
        .unwrap();
        let error = transform
            .transform_rgb_chunk(&[[0; 3]; 2], &mut [[0.0; 3]; 1])
            .unwrap_err();
        assert!(error.contains("chunk length mismatch"));
        let error = transform
            .transform_cmyk_chunk(&[[0; 4]], &mut [[0.0; 3]])
            .unwrap_err();
        assert!(error.contains("not created for CMYK"));
    }
}
