#[derive(Clone, Debug, PartialEq)]
pub struct ChannelUsageStats {
    pub name: String,
    pub mean_coverage: f64,
    pub peak_coverage: f64,
    pub nonzero_percent: f64,
    pub limit_hit_percent: f64,
    /// Sum of normalized coverage over observed pixels. This is useful for
    /// relative ink-consumption comparison even without physical drop-volume calibration.
    pub integrated_coverage: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConversionUsageReport {
    pub pixel_count: u64,
    pub channels: Vec<ChannelUsageStats>,
    pub mean_total_ink: f64,
    pub peak_total_ink: f64,
    pub total_ink_limit_hit_percent: f64,
    /// Share of neutral-region ink coverage contributed by the configured Black
    /// channel. None when no Black channel is configured or no neutral pixels were observed.
    pub neutral_black_share: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct ConversionUsageAccumulator {
    channel_names: Vec<String>,
    channel_limits: Vec<Option<f64>>,
    total_ink_limit: Option<f64>,
    black_index: Option<usize>,
    pixel_count: u64,
    channel_sum: Vec<f64>,
    channel_peak: Vec<f64>,
    channel_nonzero: Vec<u64>,
    channel_limit_hits: Vec<u64>,
    total_ink_sum: f64,
    total_ink_peak: f64,
    total_ink_limit_hits: u64,
    neutral_ink_sum: f64,
    neutral_black_sum: f64,
    neutral_pixel_count: u64,
}

impl ConversionUsageAccumulator {
    pub fn new(
        channel_names: Vec<String>,
        channel_limits: Vec<Option<f64>>,
        total_ink_limit: Option<f64>,
        black_channel: Option<&str>,
    ) -> Result<Self, String> {
        if channel_names.is_empty() {
            return Err("Conversion analytics requires at least one target channel.".to_owned());
        }
        if channel_limits.len() != channel_names.len() {
            return Err(format!(
                "Channel-limit count mismatch: {} names, {} limits.",
                channel_names.len(),
                channel_limits.len()
            ));
        }
        for (index, limit) in channel_limits.iter().copied().enumerate() {
            if limit.is_some_and(|value| !(0.0..=1.0).contains(&value)) {
                return Err(format!(
                    "Invalid coverage limit for channel '{}'.",
                    channel_names[index]
                ));
            }
        }
        if total_ink_limit.is_some_and(|value| value <= 0.0 || !value.is_finite()) {
            return Err("Total ink limit must be a finite value greater than zero.".to_owned());
        }

        let black_index = black_channel
            .map(|name| {
                channel_names
                    .iter()
                    .position(|channel| channel == name)
                    .ok_or_else(|| format!("Black channel '{name}' is not present in target topology."))
            })
            .transpose()?;
        let channel_count = channel_names.len();

        Ok(Self {
            channel_names,
            channel_limits,
            total_ink_limit,
            black_index,
            pixel_count: 0,
            channel_sum: vec![0.0; channel_count],
            channel_peak: vec![0.0; channel_count],
            channel_nonzero: vec![0; channel_count],
            channel_limit_hits: vec![0; channel_count],
            total_ink_sum: 0.0,
            total_ink_peak: 0.0,
            total_ink_limit_hits: 0,
            neutral_ink_sum: 0.0,
            neutral_black_sum: 0.0,
            neutral_pixel_count: 0,
        })
    }

    /// Observe one normalized 16-bit target pixel. `is_neutral` must come from
    /// the same source/PCS classification used by the separation strategy; this
    /// accumulator deliberately does not guess neutrality from ink values.
    pub fn observe_u16(&mut self, pixel: &[u16], is_neutral: bool) -> Result<(), String> {
        if pixel.len() != self.channel_names.len() {
            return Err(format!(
                "Analytics pixel channel mismatch: expected {}, got {}.",
                self.channel_names.len(),
                pixel.len()
            ));
        }

        let mut total = 0.0f64;
        let mut black = 0.0f64;
        for (index, sample) in pixel.iter().copied().enumerate() {
            let coverage = f64::from(sample) / f64::from(u16::MAX);
            total += coverage;
            self.channel_sum[index] += coverage;
            self.channel_peak[index] = self.channel_peak[index].max(coverage);
            if sample > 0 {
                self.channel_nonzero[index] += 1;
            }
            if self.channel_limits[index].is_some_and(|limit| coverage >= limit) {
                self.channel_limit_hits[index] += 1;
            }
            if self.black_index == Some(index) {
                black = coverage;
            }
        }

        self.pixel_count += 1;
        self.total_ink_sum += total;
        self.total_ink_peak = self.total_ink_peak.max(total);
        if self.total_ink_limit.is_some_and(|limit| total >= limit) {
            self.total_ink_limit_hits += 1;
        }

        if is_neutral {
            self.neutral_pixel_count += 1;
            self.neutral_ink_sum += total;
            self.neutral_black_sum += black;
        }
        Ok(())
    }

    pub fn finish(self) -> ConversionUsageReport {
        let pixels = self.pixel_count.max(1) as f64;
        let channels = self
            .channel_names
            .into_iter()
            .enumerate()
            .map(|(index, name)| ChannelUsageStats {
                name,
                mean_coverage: self.channel_sum[index] / pixels,
                peak_coverage: self.channel_peak[index],
                nonzero_percent: self.channel_nonzero[index] as f64 * 100.0 / pixels,
                limit_hit_percent: self.channel_limit_hits[index] as f64 * 100.0 / pixels,
                integrated_coverage: self.channel_sum[index],
            })
            .collect();

        let neutral_black_share = if self.black_index.is_some()
            && self.neutral_pixel_count > 0
            && self.neutral_ink_sum > 0.0
        {
            Some(self.neutral_black_sum / self.neutral_ink_sum)
        } else {
            None
        };

        ConversionUsageReport {
            pixel_count: self.pixel_count,
            channels,
            mean_total_ink: self.total_ink_sum / pixels,
            peak_total_ink: self.total_ink_peak,
            total_ink_limit_hit_percent: self.total_ink_limit_hits as f64 * 100.0 / pixels,
            neutral_black_share,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(value: f64) -> u16 {
        (value.clamp(0.0, 1.0) * f64::from(u16::MAX)).round() as u16
    }

    #[test]
    fn reports_per_channel_and_total_usage() {
        let mut acc = ConversionUsageAccumulator::new(
            vec!["Blue".into(), "Black".into()],
            vec![Some(0.8), Some(0.7)],
            Some(1.2),
            Some("Black"),
        )
        .unwrap();
        acc.observe_u16(&[sample(0.2), sample(0.1)], true).unwrap();
        acc.observe_u16(&[sample(0.4), sample(0.3)], true).unwrap();
        let report = acc.finish();

        assert_eq!(report.pixel_count, 2);
        assert!((report.channels[0].mean_coverage - 0.3).abs() < 0.0001);
        assert!((report.channels[1].mean_coverage - 0.2).abs() < 0.0001);
        assert!((report.mean_total_ink - 0.5).abs() < 0.0001);
        assert!(report.neutral_black_share.is_some());
    }

    #[test]
    fn black_focused_candidate_has_higher_neutral_black_share() {
        let mut balanced = ConversionUsageAccumulator::new(
            vec!["Blue".into(), "Brown".into(), "Beige".into(), "Black".into()],
            vec![None; 4],
            None,
            Some("Black"),
        )
        .unwrap();
        let mut focused = balanced.clone();

        balanced
            .observe_u16(
                &[sample(0.30), sample(0.15), sample(0.25), sample(0.05)],
                true,
            )
            .unwrap();
        focused
            .observe_u16(
                &[sample(0.10), sample(0.05), sample(0.08), sample(0.32)],
                true,
            )
            .unwrap();

        let balanced = balanced.finish();
        let focused = focused.finish();
        assert!(focused.neutral_black_share.unwrap() > balanced.neutral_black_share.unwrap());
        assert!(focused.channels[0].integrated_coverage < balanced.channels[0].integrated_coverage);
    }

    #[test]
    fn tracks_channel_and_total_limit_hits() {
        let mut acc = ConversionUsageAccumulator::new(
            vec!["Blue".into(), "Black".into()],
            vec![Some(0.5), Some(0.6)],
            Some(0.9),
            Some("Black"),
        )
        .unwrap();
        acc.observe_u16(&[sample(0.5), sample(0.4)], false).unwrap();
        acc.observe_u16(&[sample(0.6), sample(0.5)], false).unwrap();
        let report = acc.finish();

        assert_eq!(report.channels[0].limit_hit_percent, 100.0);
        assert_eq!(report.total_ink_limit_hit_percent, 100.0);
    }

    #[test]
    fn rejects_mismatched_pixel_topology() {
        let mut acc = ConversionUsageAccumulator::new(
            vec!["Blue".into(), "Black".into()],
            vec![None, None],
            None,
            Some("Black"),
        )
        .unwrap();
        assert!(acc.observe_u16(&[1], false).is_err());
    }
}
