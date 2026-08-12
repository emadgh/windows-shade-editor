use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use tiff::decoder::{Decoder, Limits};
use tiff::encoder::Rational;
use tiff::tags::Tag;

#[derive(Clone, Copy, Debug)]
pub struct DpiInfo {
    pub dpi_x: f64,
    pub dpi_y: f64,
    pub raw_x: Option<f64>,
    pub raw_y: Option<f64>,
    /// TIFF ResolutionUnit: 1=None, 2=Inch, 3=Centimeter.
    pub unit: u16,
    pub has_physical_resolution: bool,
}

impl Default for DpiInfo {
    fn default() -> Self {
        Self {
            dpi_x: 72.0,
            dpi_y: 72.0,
            raw_x: None,
            raw_y: None,
            unit: 1,
            has_physical_resolution: false,
        }
    }
}

pub fn read_dpi(path: &Path) -> DpiInfo {
    let Ok(file) = File::open(path) else { return DpiInfo::default(); };
    let Ok(mut decoder) = Decoder::new(BufReader::new(file)) else { return DpiInfo::default(); };
    decoder = decoder.with_limits(Limits::unlimited());

    let raw_x = decoder.get_tag_f64(Tag::XResolution).ok().filter(|v| v.is_finite() && *v > 0.0);
    let raw_y = decoder.get_tag_f64(Tag::YResolution).ok().filter(|v| v.is_finite() && *v > 0.0);
    let unit = decoder.get_tag_unsigned::<u16>(Tag::ResolutionUnit).unwrap_or(1);

    let convert = |value: Option<f64>| -> Option<f64> {
        let value = value?;
        match unit {
            2 => Some(value),
            3 => Some(value * 2.54),
            _ => None,
        }
    };

    let x = convert(raw_x);
    let y = convert(raw_y);
    let has = x.is_some() || y.is_some();
    let fallback = x.or(y).unwrap_or(72.0);

    DpiInfo {
        dpi_x: x.unwrap_or(fallback),
        dpi_y: y.unwrap_or(fallback),
        raw_x,
        raw_y,
        unit,
        has_physical_resolution: has,
    }
}

pub fn rational(value: f64) -> Rational {
    let value = value.max(0.0001);
    let d = 10_000u32;
    let n = (value * f64::from(d)).round().clamp(1.0, f64::from(u32::MAX)) as u32;
    Rational { n, d }
}

pub fn pixels_for_cm(cm: f32, dpi: f64) -> usize {
    ((f64::from(cm.max(0.0)) / 2.54) * dpi.max(1.0)).round().max(0.0) as usize
}

pub fn pixels_for_points(points: f32, dpi: f64) -> f32 {
    (f64::from(points.max(1.0)) * dpi.max(1.0) / 72.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_units_convert_correctly() {
        assert_eq!(pixels_for_cm(2.54, 300.0), 300);
        assert!((pixels_for_points(12.0, 300.0) - 50.0).abs() < 0.001);
    }
}
