use lcms2::{
    ColorSpaceSignature, ColorSpaceSignatureExt, Flags, Intent, PixelFormat, Profile,
    ProfileClassSignature, Transform,
};

use crate::icc_conversion::{IccSourceModel, RuntimeIccProfile};
use crate::nchannel_icc::nchannel_pixel_format;

enum DeviceLinkTransform<const N: usize> {
    Rgb(Transform<[u16; 3], [u16; N]>),
    Cmyk(Transform<[u16; 4], [u16; N]>),
}

/// Direct production transform backed by one ICC DeviceLink profile.
///
/// A DeviceLink already fixes the source-to-device mapping, rendering behavior,
/// and black-generation policy. The separately captured Source ICC is therefore
/// revalidated for source provenance but is deliberately not inserted into this
/// LittleCMS chain.
pub struct ProductionDeviceLinkTransform<const N: usize> {
    transform: DeviceLinkTransform<N>,
    source_model: IccSourceModel,
}

impl<const N: usize> ProductionDeviceLinkTransform<N> {
    pub fn new(
        source_model: IccSourceModel,
        device_link: RuntimeIccProfile<'_>,
    ) -> Result<Self, String> {
        let link = open_profile(device_link)?;
        if link.device_class() != ProfileClassSignature::LinkClass {
            return Err(
                "Direct DeviceLink conversion requires an ICC LinkClass profile.".to_owned(),
            );
        }

        let expected_source = match source_model {
            IccSourceModel::Rgb => ColorSpaceSignature::RgbData,
            IccSourceModel::Cmyk => ColorSpaceSignature::CmykData,
        };
        if link.color_space() != expected_source {
            return Err(format!(
                "DeviceLink input declares {} channels but {:?} source data requires {}.",
                link.color_space().channels(),
                source_model,
                expected_source.channels()
            ));
        }
        if link.pcs().channels() != N as u32 {
            return Err(format!(
                "DeviceLink output declares {} channels but the captured target requires {N}.",
                link.pcs().channels()
            ));
        }
        if N == 4 && link.pcs() != ColorSpaceSignature::CmykData {
            return Err(
                "Four-channel DeviceLink output must declare CMYK output space.".to_owned(),
            );
        }

        let output_format = output_pixel_format::<N>()?;
        let transform = match source_model {
            IccSourceModel::Rgb => {
                let result: lcms2::LCMSResult<Transform<[u16; 3], [u16; N]>> =
                    Transform::new_multiprofile(
                        &[&link],
                        PixelFormat::RGB_16,
                        output_format,
                        Intent::Perceptual,
                        Flags::default(),
                    );
                result.map(DeviceLinkTransform::Rgb).map_err(|err| {
                    format!("Cannot create direct RGB-to-{N}C DeviceLink transform: {err}")
                })?
            }
            IccSourceModel::Cmyk => {
                let result: lcms2::LCMSResult<Transform<[u16; 4], [u16; N]>> =
                    Transform::new_multiprofile(
                        &[&link],
                        PixelFormat::CMYK_16,
                        output_format,
                        Intent::Perceptual,
                        Flags::default(),
                    );
                result.map(DeviceLinkTransform::Cmyk).map_err(|err| {
                    format!("Cannot create direct CMYK-to-{N}C DeviceLink transform: {err}")
                })?
            }
        };

        Ok(Self {
            transform,
            source_model,
        })
    }

    pub fn source_model(&self) -> IccSourceModel {
        self.source_model
    }

    pub fn channel_count(&self) -> usize {
        N
    }

    pub fn transform_rgb_chunk(
        &self,
        source: &[[u16; 3]],
        destination: &mut [[u16; N]],
    ) -> Result<(), String> {
        validate_lengths(source.len(), destination.len())?;
        let DeviceLinkTransform::Rgb(transform) = &self.transform else {
            return Err(format!(
                "This {N}-channel DeviceLink transform was not created for RGB source data."
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
        let DeviceLinkTransform::Cmyk(transform) = &self.transform else {
            return Err(format!(
                "This {N}-channel DeviceLink transform was not created for CMYK source data."
            ));
        };
        transform.transform_pixels(source, destination);
        Ok(())
    }
}

fn open_profile(profile: RuntimeIccProfile<'_>) -> Result<Profile, String> {
    match profile {
        RuntimeIccProfile::Embedded(bytes) => Profile::new_icc(bytes)
            .map_err(|err| format!("Cannot open DeviceLink ICC from captured bytes: {err}")),
        RuntimeIccProfile::File(path) => Profile::new_file(path)
            .map_err(|err| format!("Cannot open DeviceLink ICC {}: {err}", path.display())),
    }
}

fn output_pixel_format<const N: usize>() -> Result<PixelFormat, String> {
    if N == 4 {
        Ok(PixelFormat::CMYK_16)
    } else {
        nchannel_pixel_format::<N>()
    }
}

fn validate_lengths(source_len: usize, destination_len: usize) -> Result<(), String> {
    if source_len != destination_len {
        return Err(format!(
            "DeviceLink chunk length mismatch: {source_len} source pixels, {destination_len} destination pixels."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmyk_ink_limit_link() -> Vec<u8> {
        Profile::ink_limiting(ColorSpaceSignature::CmykData, 240.0)
            .expect("create deterministic LittleCMS CMYK DeviceLink")
            .icc()
            .expect("serialize deterministic DeviceLink")
    }

    #[test]
    fn real_cmyk_devicelink_fixture_is_deterministic_and_ink_limited() {
        let bytes = cmyk_ink_limit_link();
        let transform = ProductionDeviceLinkTransform::<4>::new(
            IccSourceModel::Cmyk,
            RuntimeIccProfile::Embedded(&bytes),
        )
        .expect("open direct CMYK DeviceLink");
        let source = [
            [0u16, 0, 0, 0],
            [u16::MAX, u16::MAX, u16::MAX, u16::MAX],
            [52_000, 41_000, 30_000, 22_000],
        ];
        let mut first = [[0u16; 4]; 3];
        let mut second = [[0u16; 4]; 3];
        transform.transform_cmyk_chunk(&source, &mut first).unwrap();
        transform
            .transform_cmyk_chunk(&source, &mut second)
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(first[0], [0, 0, 0, 0]);
        let maximum_total = 2.40 * f64::from(u16::MAX);
        for pixel in first {
            assert!(pixel.into_iter().map(u64::from).sum::<u64>() as f64 <= maximum_total + 4.0);
        }
    }

    #[test]
    fn device_link_input_model_mismatch_is_rejected() {
        let bytes = cmyk_ink_limit_link();
        let error = ProductionDeviceLinkTransform::<4>::new(
            IccSourceModel::Rgb,
            RuntimeIccProfile::Embedded(&bytes),
        )
        .err()
        .expect("CMYK link must reject RGB input");
        assert!(error.contains("DeviceLink input"));
    }

    #[test]
    fn normal_icc_profile_is_not_accepted_as_a_device_link() {
        let bytes = Profile::new_srgb().icc().unwrap();
        let error = ProductionDeviceLinkTransform::<4>::new(
            IccSourceModel::Rgb,
            RuntimeIccProfile::Embedded(&bytes),
        )
        .err()
        .expect("Display ICC must not execute as DeviceLink");
        assert!(error.contains("LinkClass"));
    }
}
