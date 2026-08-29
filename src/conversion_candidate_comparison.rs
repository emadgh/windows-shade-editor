use crate::conversion_analytics::ConversionUsageReport;
use crate::conversion_candidate_preview::CandidatePreviewResult;

/// Bounded in-memory comparison snapshot derived from an already-rendered
/// Production Candidate. It intentionally retains analytics/identity only and
/// never clones Candidate raster planes.
#[derive(Clone, Debug, PartialEq)]
pub struct CandidateComparisonSnapshot {
    pub source_state_id: String,
    pub recipe_sha256: String,
    pub width: usize,
    pub height: usize,
    pub channel_names: Vec<String>,
    pub usage: ConversionUsageReport,
}

impl CandidateComparisonSnapshot {
    pub fn from_preview(
        source_state_id: impl Into<String>,
        result: &CandidatePreviewResult,
    ) -> Result<Self, String> {
        let source_state_id = source_state_id.into();
        if source_state_id.trim().is_empty() {
            return Err("Candidate comparison requires a non-empty Source-state identity.".to_owned());
        }
        validate_recipe_sha256(&result.recipe_sha256)?;
        if result.width == 0 || result.height == 0 {
            return Err("Candidate comparison cannot snapshot an empty preview raster.".to_owned());
        }
        let pixels = result
            .width
            .checked_mul(result.height)
            .ok_or_else(|| "Candidate comparison preview dimensions overflow pixel count.".to_owned())?;
        let pixels = u64::try_from(pixels)
            .map_err(|_| "Candidate comparison pixel count does not fit u64.".to_owned())?;
        if result.usage.pixel_count != pixels {
            return Err(format!(
                "Candidate comparison usage covers {} pixels; preview contains {pixels}.",
                result.usage.pixel_count
            ));
        }
        if result.channels.len() != result.usage.channels.len() {
            return Err(format!(
                "Candidate comparison topology has {} target channels but {} analytics channels.",
                result.channels.len(),
                result.usage.channels.len()
            ));
        }

        let channel_names = result
            .channels
            .iter()
            .map(|channel| channel.name.clone())
            .collect::<Vec<_>>();
        if channel_names.iter().any(|name| name.trim().is_empty()) {
            return Err("Candidate comparison target channel names cannot be empty.".to_owned());
        }
        for (target, usage) in channel_names.iter().zip(&result.usage.channels) {
            if usage.name != *target {
                return Err(format!(
                    "Candidate comparison analytics channel '{}' does not match target channel '{}'.",
                    usage.name, target
                ));
            }
        }

        Ok(Self {
            source_state_id,
            recipe_sha256: result.recipe_sha256.clone(),
            width: result.width,
            height: result.height,
            channel_names,
            usage: result.usage.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CandidateChannelDelta {
    pub name: String,
    /// Candidate B minus baseline A, normalized 0..=1 coverage units.
    pub mean_coverage: f32,
    pub p95_coverage: f32,
    pub p99_coverage: f32,
    pub peak_coverage: f32,
    /// Candidate B minus baseline A in percentage points.
    pub nonzero_percent: f32,
    /// Candidate B minus baseline A in percentage points when both recipes
    /// expose a comparable configured channel limit.
    pub limit_hit_percent: Option<f32>,
    /// Candidate B minus baseline A in deterministic relative ink units.
    pub integrated_coverage: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CandidateComparison {
    pub source_state_id: String,
    pub baseline_recipe_sha256: String,
    pub candidate_recipe_sha256: String,
    /// Candidate B minus baseline A.
    pub mean_total_ink: f32,
    pub p95_total_ink: f32,
    pub p99_total_ink: f32,
    pub peak_total_ink: f32,
    /// Candidate B minus baseline A in percentage points when both recipes
    /// expose a comparable configured total-ink limit.
    pub total_ink_limit_hit_percent: Option<f32>,
    /// Candidate B minus baseline A when both reports contain valid measured
    /// neutral classification. Missing measured evidence stays explicit.
    pub neutral_black_share: Option<f32>,
    pub channels: Vec<CandidateChannelDelta>,
}

/// Compare two already-rendered Candidate snapshots. The function refuses to
/// compare different Source states, raster populations, or target topologies so
/// A/B metrics cannot silently mix unrelated inputs.
pub fn compare_candidate_snapshots(
    baseline: &CandidateComparisonSnapshot,
    candidate: &CandidateComparisonSnapshot,
) -> Result<CandidateComparison, String> {
    if baseline.source_state_id != candidate.source_state_id {
        return Err(
            "Candidate A/B comparison requires the exact same Source-state identity.".to_owned(),
        );
    }
    if baseline.recipe_sha256 == candidate.recipe_sha256 {
        return Err("Candidate A/B comparison requires two distinct recipe identities.".to_owned());
    }
    if baseline.width != candidate.width || baseline.height != candidate.height {
        return Err("Candidate A/B comparison requires identical preview dimensions.".to_owned());
    }
    if baseline.usage.pixel_count != candidate.usage.pixel_count {
        return Err("Candidate A/B comparison requires identical pixel populations.".to_owned());
    }
    if baseline.channel_names != candidate.channel_names {
        return Err(
            "Candidate A/B comparison requires the exact same target channel order and names."
                .to_owned(),
        );
    }
    if baseline.usage.channels.len() != candidate.usage.channels.len()
        || baseline.usage.channels.len() != baseline.channel_names.len()
    {
        return Err("Candidate A/B analytics topology is inconsistent.".to_owned());
    }

    let channels = baseline
        .usage
        .channels
        .iter()
        .zip(&candidate.usage.channels)
        .zip(&baseline.channel_names)
        .map(|((a, b), expected_name)| {
            if a.name != *expected_name || b.name != *expected_name {
                return Err(format!(
                    "Candidate A/B analytics channel order drifted at '{}'.",
                    expected_name
                ));
            }
            Ok(CandidateChannelDelta {
                name: expected_name.clone(),
                mean_coverage: b.mean_coverage - a.mean_coverage,
                p95_coverage: b.percentiles.p95 - a.percentiles.p95,
                p99_coverage: b.percentiles.p99 - a.percentiles.p99,
                peak_coverage: b.peak_coverage - a.peak_coverage,
                nonzero_percent: b.nonzero_percent - a.nonzero_percent,
                limit_hit_percent: option_delta(a.limit_hit_percent, b.limit_hit_percent),
                integrated_coverage: b.integrated_coverage - a.integrated_coverage,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(CandidateComparison {
        source_state_id: baseline.source_state_id.clone(),
        baseline_recipe_sha256: baseline.recipe_sha256.clone(),
        candidate_recipe_sha256: candidate.recipe_sha256.clone(),
        mean_total_ink: candidate.usage.mean_total_ink - baseline.usage.mean_total_ink,
        p95_total_ink: candidate.usage.total_ink_percentiles.p95
            - baseline.usage.total_ink_percentiles.p95,
        p99_total_ink: candidate.usage.total_ink_percentiles.p99
            - baseline.usage.total_ink_percentiles.p99,
        peak_total_ink: candidate.usage.peak_total_ink - baseline.usage.peak_total_ink,
        total_ink_limit_hit_percent: option_delta(
            baseline.usage.total_ink_limit_hit_percent,
            candidate.usage.total_ink_limit_hit_percent,
        ),
        neutral_black_share: option_delta(
            baseline.usage.neutral_black_share,
            candidate.usage.neutral_black_share,
        ),
        channels,
    })
}

fn option_delta(a: Option<f32>, b: Option<f32>) -> Option<f32> {
    Some(b? - a?)
}

fn validate_recipe_sha256(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Candidate comparison requires a canonical 64-digit recipe SHA-256.".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::TargetChannelDefinition;
    use crate::conversion_analytics::{
        ChannelUsageStats, CoveragePercentiles,
    };

    fn result(
        recipe: char,
        source_mean: f32,
        total_mean: f32,
        neutral_black_share: Option<f32>,
    ) -> CandidatePreviewResult {
        CandidatePreviewResult {
            width: 2,
            height: 1,
            recipe_sha256: recipe.to_string().repeat(64),
            channels: vec![TargetChannelDefinition {
                name: "Black".to_owned(),
                display_rgb: Some([0, 0, 0]),
                solidity: 1.0,
                max_coverage: Some(1.0),
            }],
            planes: vec![vec![0, u16::MAX]],
            histograms: vec![[0; 256]],
            usage: ConversionUsageReport {
                pixel_count: 2,
                channels: vec![ChannelUsageStats {
                    name: "Black".to_owned(),
                    mean_coverage: source_mean,
                    peak_coverage: source_mean + 0.1,
                    percentiles: CoveragePercentiles {
                        p50: source_mean,
                        p95: source_mean + 0.05,
                        p99: source_mean + 0.08,
                    },
                    nonzero_percent: source_mean * 100.0,
                    limit_hit_percent: Some(source_mean * 10.0),
                    integrated_coverage: f64::from(source_mean) * 2.0,
                }],
                mean_total_ink: total_mean,
                peak_total_ink: total_mean + 0.1,
                total_ink_percentiles: CoveragePercentiles {
                    p50: total_mean,
                    p95: total_mean + 0.05,
                    p99: total_mean + 0.08,
                },
                total_ink_limit_hit_percent: Some(total_mean * 10.0),
                neutral_black_share,
            },
        }
    }

    #[test]
    fn comparison_uses_real_candidate_analytics_without_retaining_raster_planes() {
        let a = CandidateComparisonSnapshot::from_preview("source-1", &result('a', 0.25, 0.4, None))
            .unwrap();
        let b = CandidateComparisonSnapshot::from_preview("source-1", &result('b', 0.35, 0.55, None))
            .unwrap();
        let comparison = compare_candidate_snapshots(&a, &b).unwrap();
        assert!((comparison.mean_total_ink - 0.15).abs() < 1e-6);
        assert!((comparison.channels[0].mean_coverage - 0.10).abs() < 1e-6);
        assert_eq!(comparison.neutral_black_share, None);

        let source = include_str!("conversion_candidate_comparison.rs");
        let runtime = source.split("\n#[cfg(test)]").next().unwrap_or(source);
        assert!(!runtime.contains("result.planes"));
        assert!(!runtime.contains("render_candidate_preview("));
        assert!(!runtime.contains("ConversionUsageAccumulator"));
    }

    #[test]
    fn comparison_rejects_different_source_state() {
        let a = CandidateComparisonSnapshot::from_preview("source-a", &result('a', 0.2, 0.3, None))
            .unwrap();
        let b = CandidateComparisonSnapshot::from_preview("source-b", &result('b', 0.3, 0.4, None))
            .unwrap();
        let error = compare_candidate_snapshots(&a, &b).unwrap_err();
        assert!(error.contains("exact same Source-state"));
    }

    #[test]
    fn measured_neutral_delta_stays_unavailable_until_both_candidates_have_evidence() {
        let a = CandidateComparisonSnapshot::from_preview("source", &result('a', 0.2, 0.3, Some(0.4)))
            .unwrap();
        let b = CandidateComparisonSnapshot::from_preview("source", &result('b', 0.3, 0.4, None))
            .unwrap();
        assert_eq!(
            compare_candidate_snapshots(&a, &b)
                .unwrap()
                .neutral_black_share,
            None
        );

        let b = CandidateComparisonSnapshot::from_preview("source", &result('b', 0.3, 0.4, Some(0.6)))
            .unwrap();
        let delta = compare_candidate_snapshots(&a, &b)
            .unwrap()
            .neutral_black_share
            .unwrap();
        assert!((delta - 0.2).abs() < 1e-6);
    }

    #[test]
    fn comparison_rejects_target_topology_drift() {
        let a = CandidateComparisonSnapshot::from_preview("source", &result('a', 0.2, 0.3, None))
            .unwrap();
        let mut b = CandidateComparisonSnapshot::from_preview("source", &result('b', 0.3, 0.4, None))
            .unwrap();
        b.channel_names[0] = "Blue".to_owned();
        let error = compare_candidate_snapshots(&a, &b).unwrap_err();
        assert!(error.contains("target channel order"));
    }
}
