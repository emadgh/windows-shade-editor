use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use tiff::decoder::{Decoder, Limits};
use tiff::encoder::Rational;
use tiff::tags::Tag;

pub const DEFAULT_DPI: f64 = 220.0;

#[derive(Clone, Copy, Debug)]
pub struct DpiInfo {
    pub dpi_x: f64,
    pub dpi_y: f64,
    pub raw_x: Option<f64>,
    pub raw_y: Option<f64>,
    /// TIFF ResolutionUnit: 1=None, 2=Inch, 3=Centimeter.
    pub unit: u16,
    pub has_physical_resolution: bool,
    pub used_default: bool,
}

impl Default for DpiInfo {
    fn default() -> Self {
        Self::with_default(DEFAULT_DPI)
    }
}

impl DpiInfo {
    pub fn with_default(default_dpi: f64) -> Self {
        let dpi = normalize_default_dpi(default_dpi);
        Self {
            dpi_x: dpi,
            dpi_y: dpi,
            raw_x: None,
            raw_y: None,
            unit: 1,
            has_physical_resolution: false,
            used_default: true,
        }
    }

    pub fn effective_tiff_resolution(self) -> (f64, f64, u16) {
        if self.has_physical_resolution && matches!(self.unit, 2 | 3) {
            let fallback_raw = match self.unit {
                3 => self.dpi_x / 2.54,
                _ => self.dpi_x,
            };
            let x = self.raw_x.or(self.raw_y).unwrap_or(fallback_raw);
            let fallback_y = match self.unit {
                3 => self.dpi_y / 2.54,
                _ => self.dpi_y,
            };
            let y = self.raw_y.or(self.raw_x).unwrap_or(fallback_y);
            (x, y, self.unit)
        } else {
            (self.dpi_x, self.dpi_y, 2)
        }
    }
}

pub fn read_dpi(path: &Path, default_dpi: f64) -> DpiInfo {
    let fallback = normalize_default_dpi(default_dpi);
    let Ok(file) = File::open(path) else {
        return DpiInfo::with_default(fallback);
    };
    let Ok(mut decoder) = Decoder::new(BufReader::new(file)) else {
        return DpiInfo::with_default(fallback);
    };
    decoder = decoder.with_limits(Limits::unlimited());

    let raw_x = decoder
        .get_tag_f64(Tag::XResolution)
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0);
    let raw_y = decoder
        .get_tag_f64(Tag::YResolution)
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0);
    // TIFF 6.0 defines ResolutionUnit's default as 2 (inch).
    let unit = decoder
        .get_tag_unsigned::<u16>(Tag::ResolutionUnit)
        .unwrap_or(2);

    #[cfg(test)]
    eprintln!(
        "SHADE_DPI_DIAG path={} raw_x={raw_x:?} raw_y={raw_y:?} unit={unit}",
        path.display()
    );

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
    let resolved = x.or(y).unwrap_or(fallback);

    DpiInfo {
        dpi_x: x.unwrap_or(resolved),
        dpi_y: y.unwrap_or(resolved),
        raw_x,
        raw_y,
        unit,
        has_physical_resolution: has,
        used_default: !has,
    }
}

fn normalize_default_dpi(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(36.0, 2400.0)
    } else {
        DEFAULT_DPI
    }
}

pub fn rational(value: f64) -> Rational {
    let value = value.max(0.0001);
    let d = 10_000u32;
    let n = (value * f64::from(d))
        .round()
        .clamp(1.0, f64::from(u32::MAX)) as u32;
    Rational { n, d }
}

pub fn pixels_for_cm(cm: f32, dpi: f64) -> usize {
    ((f64::from(cm.max(0.0)) / 2.54) * dpi.max(1.0))
        .round()
        .max(0.0) as usize
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

    #[test]
    fn fallback_dpi_is_configurable_and_written_as_inches() {
        let info = DpiInfo::with_default(220.0);
        assert_eq!(info.dpi_x, 220.0);
        assert_eq!(info.dpi_y, 220.0);
        assert!(info.used_default);
        assert_eq!(info.effective_tiff_resolution(), (220.0, 220.0, 2));
    }
}
