use lcms2::{
    ColorSpaceSignature, ColorSpaceSignatureExt, Flags, PixelFormat, Profile,
    ProfileClassSignature, Transform,
};

use crate::icc_conversion::{IccSourceModel, RuntimeIccProfile};

enum DeviceLinkTransform<const N: usize> {
    Rgb(Transform<[u16; 3], [u16; N]>),
    Cmyk(Transform<[u16; 4], [u16; N]>),
}

/// Production transform for a precomputed ICC DeviceLink.
///
/// DeviceLink separation is fixed by the link profile itself. Shade Editor
/// therefore uses the rendering intent stored in the DeviceLink header and does
/// not expose runtime BPC/Black-generation/ink-bias controls for this engine.
pub struct ProductionDeviceLinkTransform<const N: usize> {
    transform: DeviceLinkTransform<N>,
    source_model: IccSourceModel,
    link_intent: lcms2::Intent,
}

impl<const N: usize> ProductionDeviceLinkTransform<N> {
    pub fn new(
        source_model: IccSourceModel,
        device_link: RuntimeIccProfile<'_>,
    ) -> Result<Self, String> {
        let link = open_profile(device_link)?;
        if link.device_class() != ProfileClassSignature::LinkClass {
            return Err("Selected profile is not an ICC DeviceLink profile.".to_owned());
        }

        let expected_input = match source_model {
            IccSourceModel::Rgb => ColorSpaceSignature::RgbData,
            IccSourceModel::Cmyk => ColorSpaceSignature::CmykData,
        };
        if link.color_space() != expected_input {
            return Err(format!(
                "DeviceLink input color space does not match requested {:?} source model.",
                source_model
            ));
        }

        let output_space = link.pcs();
        if output_space.channels() != N as u32 {
            return Err(format!(
                "DeviceLink output declares {} channels but this transform requires {N}.",
                output_space.channels()
            ));
        }
        if N == 4 && output_space != ColorSpaceSignature::CmykData {
            return Err(
                "Four-channel DeviceLink output must be CMYK for the current production path."
                    .to_owned(),
            );
        }

        let output_format = device_link_output_format::<N>()?;
        let intent = link.header_rendering_intent();
        let transform = match source_model {
            IccSourceModel::Rgb => {
                let result: lcms2::LCMSResult<Transform<[u16; 3], [u16; N]>> =
                    Transform::new_multiprofile(
                        &[&link],
                        PixelFormat::RGB_16,
                        output_format,
                        intent,
                        Flags::default(),
                    );
                result
                    .map(DeviceLinkTransform::Rgb)
                    .map_err(|err| format!("Cannot create RGB→{N}C DeviceLink transform: {err}"))?
            }
            IccSourceModel::Cmyk => {
                let result: lcms2::LCMSResult<Transform<[u16; 4], [u16; N]>> =
                    Transform::new_multiprofile(
                        &[&link],
                        PixelFormat::CMYK_16,
                        output_format,
                        intent,
                        Flags::default(),
                    );
                result
                    .map(DeviceLinkTransform::Cmyk)
                    .map_err(|err| format!("Cannot create CMYK→{N}C DeviceLink transform: {err}"))?
            }
        };

        Ok(Self {
            transform,
            source_model,
            link_intent: intent,
        })
    }

    pub fn source_model(&self) -> IccSourceModel {
        self.source_model
    }

    pub fn channel_count(&self) -> usize {
        N
    }

    pub fn link_intent(&self) -> lcms2::Intent {
        self.link_intent
    }

    pub fn transform_rgb_chunk(
        &self,
        source: &[[u16; 3]],
        destination: &mut [[u16; N]],
    ) -> Result<(), String> {
        validate_lengths(source.len(), destination.len())?;
        let DeviceLinkTransform::Rgb(transform) = &self.transform else {
            return Err(format!(
                "This {N}-channel DeviceLink was not created for RGB source data."
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
                "This {N}-channel DeviceLink was not created for CMYK source data."
            ));
        };
        transform.transform_pixels(source, destination);
        Ok(())
    }
}

fn open_profile(profile: RuntimeIccProfile<'_>) -> Result<Profile, String> {
    match profile {
        RuntimeIccProfile::Embedded(bytes) => Profile::new_icc(bytes)
            .map_err(|err| format!("Cannot open DeviceLink from embedded bytes: {err}")),
        RuntimeIccProfile::File(path) => Profile::new_file(path)
            .map_err(|err| format!("Cannot open DeviceLink {}: {err}", path.display())),
    }
}

fn device_link_output_format<const N: usize>() -> Result<PixelFormat, String> {
    match N {
        4 => Ok(PixelFormat::CMYK_16),
        5 => Ok(PixelFormat::CMYK5_16),
        6 => Ok(PixelFormat::CMYK6_16),
        7 => Ok(PixelFormat::CMYK7_16),
        8 => Ok(PixelFormat::CMYK8_16),
        9 => Ok(PixelFormat::CMYK9_16),
        10 => Ok(PixelFormat::CMYK10_16),
        11 => Ok(PixelFormat::CMYK11_16),
        12 => Ok(PixelFormat::CMYK12_16),
        _ => Err(format!(
            "Production DeviceLink output currently supports 4..=12 channels; requested {N}."
        )),
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

pub type ProductionCmykDeviceLinkTransform = ProductionDeviceLinkTransform<4>;
pub type Production5ChannelDeviceLinkTransform = ProductionDeviceLinkTransform<5>;
pub type Production6ChannelDeviceLinkTransform = ProductionDeviceLinkTransform<6>;
pub type Production7ChannelDeviceLinkTransform = ProductionDeviceLinkTransform<7>;
pub type Production8ChannelDeviceLinkTransform = ProductionDeviceLinkTransform<8>;
pub type Production9ChannelDeviceLinkTransform = ProductionDeviceLinkTransform<9>;
pub type Production10ChannelDeviceLinkTransform = ProductionDeviceLinkTransform<10>;
pub type Production11ChannelDeviceLinkTransform = ProductionDeviceLinkTransform<11>;
pub type Production12ChannelDeviceLinkTransform = ProductionDeviceLinkTransform<12>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmyk_ink_limit_profile_runs_as_fixed_devicelink() {
        let link = Profile::ink_limiting(ColorSpaceSignature::CmykData, 300.0)
            .expect("create CMYK ink-limiting DeviceLink");
        let bytes = link.icc().expect("serialize DeviceLink");
        let transform = ProductionCmykDeviceLinkTransform::new(
            IccSourceModel::Cmyk,
            RuntimeIccProfile::Embedded(&bytes),
        )
        .expect("open production DeviceLink");

        let source = [[u16::MAX, u16::MAX, u16::MAX, u16::MAX]];
        let mut destination = [[0u16; 4]; 1];
        transform
            .transform_cmyk_chunk(&source, &mut destination)
            .expect("transform CMYK pixel");
        assert_eq!(transform.channel_count(), 4);
    }

    #[test]
    fn devicelink_rejects_wrong_source_model() {
        let link = Profile::ink_limiting(ColorSpaceSignature::CmykData, 300.0)
            .expect("create DeviceLink");
        let bytes = link.icc().expect("serialize DeviceLink");
        let error = ProductionCmykDeviceLinkTransform::new(
            IccSourceModel::Rgb,
            RuntimeIccProfile::Embedded(&bytes),
        )
        .err()
        .expect("RGB must not match CMYK DeviceLink input");
        assert!(error.contains("input color space"));
    }

    #[test]
    fn supported_output_formats_cover_cmyk_and_nchannel() {
        assert_eq!(device_link_output_format::<4>().unwrap(), PixelFormat::CMYK_16);
        assert_eq!(device_link_output_format::<7>().unwrap(), PixelFormat::CMYK7_16);
        assert_eq!(device_link_output_format::<12>().unwrap(), PixelFormat::CMYK12_16);
        assert!(device_link_output_format::<13>().is_err());
    }

    #[test]
    fn chunk_lengths_are_strict() {
        assert!(validate_lengths(32, 32).is_ok());
        assert!(validate_lengths(32, 31).is_err());
    }
}
