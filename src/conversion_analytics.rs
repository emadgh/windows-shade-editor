use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::color_conversion::ConversionRecipe;
use crate::tiff_io::{for_each_decoded_region, stream_info, working_sample_from_tiff};

const HISTOGRAM_BINS: usize = 4096;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct CoveragePercentiles {
    pub p50: f32,
    pub p95: f32,
    pub p99: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChannelUsageStats {
    pub name: String,
    pub mean_coverage: f32,
    pub peak_coverage: f32,
    pub percentiles: CoveragePercentiles,
    pub nonzero_percent: f32,
    pub limit_hit_percent: Option<f32>,
    /// Sum of normalized per-pixel channel coverage. This is a deterministic
    /// relative ink unit, not a printer/RIP volume or mass measurement.
    pub integrated_coverage: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConversionUsageReport {
    pub pixel_count: u64,
    pub channels: Vec<ChannelUsageStats>,
    pub mean_total_ink: f32,
    pub peak_total_ink: f32,
    pub total_ink_percentiles: CoveragePercentiles,
    pub total_ink_limit_hit_percent: Option<f32>,
    /// None until a caller supplies a valid PCS/characterization-based neutral
    /// classification. Output ink values alone must never define neutrality.
    pub neutral_black_share: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeutralClassification {
    Unknown,
    Neutral,
    Chromatic,
}

#[derive(Clone, Debug)]
pub struct ConversionUsageAccumulator {
    channel_names: Vec<String>,
    channel_limits: Vec<Option<f32>>,
    total_ink_limit: Option<f32>,
    black_index: Option<usize>,
    pixel_count: u64,
    sums: Vec<f64>,
    peaks: Vec<u16>,
    nonzero: Vec<u64>,
    limit_hits: Vec<u64>,
    channel_histograms: Vec<Vec<u64>>,
    total_sum: f64,
    total_peak: f32,
    total_limit_hits: u64,
    total_histogram: Vec<u64>,
    neutral_pixels: u64,
    neutral_total_ink: f64,
    neutral_black_ink: f64,
}

impl ConversionUsageAccumulator {
    pub fn from_recipe(recipe: &ConversionRecipe) -> Result<Self, String> {
        recipe
            .validate()
            .map_err(|errors| format!("Cannot analyze invalid conversion recipe: {}", errors.join(" ")))?;

        let channel_names = recipe
            .target
            .channels
            .iter()
            .map(|channel| channel.name.clone())
            .collect::<Vec<_>>();
        let channel_limits = recipe
            .target
            .channels
            .iter()
            .map(|channel| channel.max_coverage)
            .collect::<Vec<_>>();
        let black_index = recipe
            .strategy
            .black_channel
            .as_deref()
            .and_then(|name| channel_names.iter().position(|candidate| candidate == name));
        let total_ink_limit = effective_total_ink_limit(
            recipe.target.total_ink_limit,
            recipe.strategy.total_ink_limit,
        );

        Self::new(channel_names, channel_limits, total_ink_limit, black_index)
    }

    fn new(
        channel_names: Vec<String>,
        channel_limits: Vec<Option<f32>>,
        total_ink_limit: Option<f32>,
        black_index: Option<usize>,
    ) -> Result<Self, String> {
        if channel_names.is_empty() || channel_names.len() != channel_limits.len() {
            return Err("Analytics requires one limit entry for every output channel.".to_owned());
        }
        if channel_names.iter().any(|name| name.trim().is_empty()) {
            return Err("Analytics channel names cannot be empty.".to_owned());
        }
        if channel_limits
            .iter()
            .flatten()
            .any(|limit| !limit.is_finite() || !(0.0..=1.0).contains(limit))
        {
            return Err("Analytics channel limits must be finite values in 0..=1.".to_owned());
        }
        if total_ink_limit.is_some_and(|limit| !limit.is_finite() || limit <= 0.0) {
            return Err("Analytics total ink limit must be finite and greater than zero.".to_owned());
        }
        if black_index.is_some_and(|index| index >= channel_names.len()) {
            return Err("Analytics Black channel index is outside the target topology.".to_owned());
        }

        let channel_count = channel_names.len();
        Ok(Self {
            channel_names,
            channel_limits,
            total_ink_limit,
            black_index,
            pixel_count: 0,
            sums: vec![0.0; channel_count],
            peaks: vec![0; channel_count],
            nonzero: vec![0; channel_count],
            limit_hits: vec![0; channel_count],
            channel_histograms: (0..channel_count)
                .map(|_| vec![0; HISTOGRAM_BINS])
                .collect(),
            total_sum: 0.0,
            total_peak: 0.0,
            total_limit_hits: 0,
            total_histogram: vec![0; HISTOGRAM_BINS],
            neutral_pixels: 0,
            neutral_total_ink: 0.0,
            neutral_black_ink: 0.0,
        })
    }

    pub fn channel_count(&self) -> usize {
        self.channel_names.len()
    }

    pub fn observe_u16(
        &mut self,
        pixel: &[u16],
        neutral: NeutralClassification,
    ) -> Result<(), String> {
        if pixel.len() != self.channel_names.len() {
            return Err(format!(
                "Analytics pixel has {} channels; expected {}.",
                pixel.len(),
                self.channel_names.len()
            ));
        }

        self.pixel_count = self.pixel_count.saturating_add(1);
        let mut total = 0.0f32;
        for (index, sample) in pixel.iter().copied().enumerate() {
            let coverage = f32::from(sample) / f32::from(u16::MAX);
            self.sums[index] += f64::from(coverage);
            self.peaks[index] = self.peaks[index].max(sample);
            self.nonzero[index] += u64::from(sample != 0);
            self.channel_histograms[index][coverage_bin_u16(sample)] += 1;
            if self.channel_limits[index].is_some_and(|limit| coverage >= limit) {
                self.limit_hits[index] += 1;
            }
            total += coverage;
        }

        self.total_sum += f64::from(total);
        self.total_peak = self.total_peak.max(total);
        if self.total_ink_limit.is_some_and(|limit| total >= limit) {
            self.total_limit_hits += 1;
        }
        self.total_histogram[total_ink_bin(total, self.channel_names.len())] += 1;

        if neutral == NeutralClassification::Neutral {
            self.neutral_pixels = self.neutral_pixels.saturating_add(1);
            self.neutral_total_ink += f64::from(total);
            if let Some(index) = self.black_index {
                self.neutral_black_ink +=
                    f64::from(pixel[index]) / f64::from(u16::MAX);
            }
        }
        Ok(())
    }

    pub fn finish(self) -> ConversionUsageReport {
        let count = self.pixel_count.max(1) as f64;
        let channels = self
            .channel_names
            .into_iter()
            .enumerate()
            .map(|(index, name)| ChannelUsageStats {
                name,
                mean_coverage: (self.sums[index] / count) as f32,
                peak_coverage: f32::from(self.peaks[index]) / f32::from(u16::MAX),
                percentiles: coverage_percentiles(&self.channel_histograms[index], self.pixel_count),
                nonzero_percent: percent(self.nonzero[index], self.pixel_count),
                limit_hit_percent: self.channel_limits[index]
                    .map(|_| percent(self.limit_hits[index], self.pixel_count)),
                integrated_coverage: self.sums[index],
            })
            .collect::<Vec<_>>();

        let neutral_black_share = if self.black_index.is_some()
            && self.neutral_pixels > 0
            && self.neutral_total_ink > f64::EPSILON
        {
            Some((self.neutral_black_ink / self.neutral_total_ink) as f32)
        } else {
            None
        };

        ConversionUsageReport {
            pixel_count: self.pixel_count,
            channels,
            mean_total_ink: (self.total_sum / count) as f32,
            peak_total_ink: self.total_peak,
            total_ink_percentiles: total_ink_percentiles(
                &self.total_histogram,
                self.pixel_count,
                self.channel_limits.len(),
            ),
            total_ink_limit_hit_percent: self
                .total_ink_limit
                .map(|_| percent(self.total_limit_hits, self.pixel_count)),
            neutral_black_share,
        }
    }
}

/// Analyze the committed/previewable conversion TIFF itself in bounded memory.
/// The report is rejected unless TIFF sample count and exact channel order match
/// the captured conversion recipe. Neutrality is intentionally `Unknown` here:
/// it cannot be inferred safely from output ink coverages alone.
pub fn analyze_conversion_tiff(
    path: &Path,
    recipe: &ConversionRecipe,
) -> Result<ConversionUsageReport, String> {
    let info = stream_info(path)?;
    let expected_names = recipe
        .target
        .channels
        .iter()
        .map(|channel| channel.name.as_str())
        .collect::<Vec<_>>();
    let actual_names = info
        .metadata
        .channel_names
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();

    if info.metadata.samples_per_pixel != expected_names.len() {
        return Err(format!(
            "Analytics topology mismatch: TIFF has {} samples, recipe has {} channels.",
            info.metadata.samples_per_pixel,
            expected_names.len()
        ));
    }
    if actual_names != expected_names {
        return Err(format!(
            "Analytics channel-order mismatch: TIFF {:?}, recipe {:?}.",
            actual_names, expected_names
        ));
    }

    let mut accumulator = ConversionUsageAccumulator::from_recipe(recipe)?;
    let channel_count = accumulator.channel_count();
    for_each_decoded_region(
        path,
        &info,
        |_x, _y, width, height, samples| {
            let pixels = usize::try_from(width)
                .ok()
                .and_then(|width| usize::try_from(height).ok().and_then(|height| width.checked_mul(height)))
                .ok_or_else(|| "Analytics region dimensions overflow.".to_owned())?;
            let expected_samples = pixels
                .checked_mul(channel_count)
                .ok_or_else(|| "Analytics region sample count overflow.".to_owned())?;
            if samples.len() != expected_samples {
                return Err(format!(
                    "Analytics region contains {} samples; expected {expected_samples}.",
                    samples.len()
                ));
            }
            for pixel_index in 0..pixels {
                let base = pixel_index * channel_count;
                let mut working = Vec::with_capacity(channel_count);
                for channel in 0..channel_count {
                    working.push(working_sample_from_tiff(
                        &info.metadata,
                        channel,
                        samples[base + channel],
                    ));
                }
                accumulator.observe_u16(&working, NeutralClassification::Unknown)?;
            }
            Ok(())
        },
    )?;
    Ok(accumulator.finish())
}

fn effective_total_ink_limit(target: Option<f32>, strategy: Option<f32>) -> Option<f32> {
    match (target, strategy) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn percent(value: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        value as f32 * 100.0 / total as f32
    }
}

fn coverage_bin_u16(sample: u16) -> usize {
    usize::from(sample) * (HISTOGRAM_BINS - 1) / usize::from(u16::MAX)
}

fn total_ink_bin(total: f32, channel_count: usize) -> usize {
    let max_total = channel_count.max(1) as f32;
    let normalized = (total / max_total).clamp(0.0, 1.0);
    (normalized * (HISTOGRAM_BINS - 1) as f32).round() as usize
}

fn percentile_bin(histogram: &[u64], total: u64, percentile: f64) -> usize {
    if total == 0 {
        return 0;
    }
    let threshold = ((total as f64 * percentile).ceil() as u64).max(1);
    let mut cumulative = 0u64;
    for (index, count) in histogram.iter().copied().enumerate() {
        cumulative = cumulative.saturating_add(count);
        if cumulative >= threshold {
            return index;
        }
    }
    histogram.len().saturating_sub(1)
}

fn coverage_percentiles(histogram: &[u64], total: u64) -> CoveragePercentiles {
    let scale = (HISTOGRAM_BINS - 1) as f32;
    CoveragePercentiles {
        p50: percentile_bin(histogram, total, 0.50) as f32 / scale,
        p95: percentile_bin(histogram, total, 0.95) as f32 / scale,
        p99: percentile_bin(histogram, total, 0.99) as f32 / scale,
    }
}

fn total_ink_percentiles(
    histogram: &[u64],
    total: u64,
    channel_count: usize,
) -> CoveragePercentiles {
    let scale = (HISTOGRAM_BINS - 1) as f32;
    let max_total = channel_count.max(1) as f32;
    CoveragePercentiles {
        p50: percentile_bin(histogram, total, 0.50) as f32 / scale * max_total,
        p95: percentile_bin(histogram, total, 0.95) as f32 / scale * max_total,
        p99: percentile_bin(histogram, total, 0.99) as f32 / scale * max_total,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::color_conversion::{
        CONVERSION_RECIPE_SCHEMA_VERSION, ConversionEngineMode, ConversionRenderingIntent,
        ConversionTargetDefinition, SeparationStrategy, TargetChannelDefinition,
    };
    use crate::model::IccProfileIdentity;

    fn recipe() -> ConversionRecipe {
        ConversionRecipe {
            schema_version: CONVERSION_RECIPE_SCHEMA_VERSION,
            engine_mode: ConversionEngineMode::CustomOptimizer,
            source_profile_identity: IccProfileIdentity {
                description: "Source".to_owned(),
                sha256: "a".repeat(64),
            },
            target: ConversionTargetDefinition {
                name: "Ceramic 4C".to_owned(),
                channels: ["Blue", "Brown", "Beige", "Black"]
                    .into_iter()
                    .map(|name| TargetChannelDefinition {
                        name: name.to_owned(),
                        display_rgb: None,
                        solidity: 1.0,
                        max_coverage: Some(0.75),
                    })
                    .collect(),
                bit_depth: 16,
                output_profile_identity: None,
                output_profile_path: None,
                device_link_identity: None,
                device_link_path: None,
                characterization_id: Some("measurement-v1".to_owned()),
                total_ink_limit: Some(1.5),
            },
            rendering_intent: ConversionRenderingIntent::RelativeColorimetric,
            black_point_compensation: false,
            strategy: SeparationStrategy {
                preset_name: "Black-focused".to_owned(),
                black_channel: Some("Black".to_owned()),
                black_generation_strength: 1.0,
                black_start: 0.2,
                black_max: 0.7,
                neutral_chroma_threshold: 8.0,
                per_ink_bias: BTreeMap::new(),
                total_ink_limit: Some(1.4),
                max_delta_e00: Some(2.0),
            },
        }
    }

    #[test]
    fn integrated_coverage_and_limits_are_deterministic() {
        let mut analytics = ConversionUsageAccumulator::from_recipe(&recipe()).unwrap();
        analytics
            .observe_u16(&[u16::MAX / 2, 0, 0, u16::MAX / 4], NeutralClassification::Unknown)
            .unwrap();
        analytics
            .observe_u16(&[u16::MAX, u16::MAX / 2, 0, u16::MAX / 2], NeutralClassification::Unknown)
            .unwrap();
        let report = analytics.finish();

        assert_eq!(report.pixel_count, 2);
        assert!((report.channels[0].integrated_coverage - 1.5).abs() < 0.001);
        assert!((report.channels[0].mean_coverage - 0.75).abs() < 0.001);
        assert_eq!(report.channels[0].nonzero_percent, 100.0);
        assert_eq!(report.channels[2].nonzero_percent, 0.0);
        assert_eq!(report.channels[0].limit_hit_percent, Some(50.0));
        assert_eq!(report.total_ink_limit_hit_percent, Some(50.0));
        assert!(report.channels[0].percentiles.p95 > 0.99);
        assert!(report.total_ink_percentiles.p95 > 1.9);
        assert_eq!(report.neutral_black_share, None);
    }

    #[test]
    fn neutral_black_share_requires_explicit_neutral_classification() {
        let mut unknown = ConversionUsageAccumulator::from_recipe(&recipe()).unwrap();
        unknown
            .observe_u16(&[1000, 1000, 1000, 20000], NeutralClassification::Unknown)
            .unwrap();
        assert_eq!(unknown.finish().neutral_black_share, None);

        let mut classified = ConversionUsageAccumulator::from_recipe(&recipe()).unwrap();
        classified
            .observe_u16(&[1000, 1000, 1000, 20000], NeutralClassification::Neutral)
            .unwrap();
        classified
            .observe_u16(&[30000, 1000, 1000, 1000], NeutralClassification::Chromatic)
            .unwrap();
        let share = classified.finish().neutral_black_share.unwrap();
        assert!(share > 0.85);
    }

    #[test]
    fn effective_strategy_total_limit_is_the_stricter_limit() {
        let analytics = ConversionUsageAccumulator::from_recipe(&recipe()).unwrap();
        assert_eq!(analytics.total_ink_limit, Some(1.4));
    }

    #[test]
    fn topology_mismatch_is_rejected() {
        let mut analytics = ConversionUsageAccumulator::from_recipe(&recipe()).unwrap();
        let error = analytics
            .observe_u16(&[1, 2, 3], NeutralClassification::Unknown)
            .unwrap_err();
        assert!(error.contains("expected 4"));
    }

    #[test]
    fn histogram_memory_is_bounded_by_channel_count_not_image_size() {
        let analytics = ConversionUsageAccumulator::from_recipe(&recipe()).unwrap();
        assert_eq!(analytics.channel_histograms.len(), 4);
        assert!(analytics
            .channel_histograms
            .iter()
            .all(|histogram| histogram.len() == HISTOGRAM_BINS));
        assert_eq!(analytics.total_histogram.len(), HISTOGRAM_BINS);
    }
}
