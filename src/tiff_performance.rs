use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const MIB: f64 = 1024.0 * 1024.0;

static PERF_LOG_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TiffPerfPhase {
    SourceIdentity,
    InspectDecode,
    AdjustmentRender,
    SourceSpoolWrite,
    SourceSpoolFlush,
    ColorTransform,
    OutputSpoolWrite,
    OutputSpoolFlush,
    CompressionEncode,
    StagedValidation,
    FinalDurability,
    AtomicPublication,
    OutputIdentity,
    RouteMigrationVerification,
}

impl TiffPerfPhase {
    pub const fn label(self) -> &'static str {
        match self {
            Self::SourceIdentity => "source_identity",
            Self::InspectDecode => "inspect_decode",
            Self::AdjustmentRender => "adjustment_render",
            Self::SourceSpoolWrite => "source_spool_write",
            Self::SourceSpoolFlush => "source_spool_flush",
            Self::ColorTransform => "color_transform",
            Self::OutputSpoolWrite => "output_spool_write",
            Self::OutputSpoolFlush => "output_spool_flush",
            Self::CompressionEncode => "compression_encode",
            Self::StagedValidation => "staged_validation",
            Self::FinalDurability => "final_durability",
            Self::AtomicPublication => "atomic_publication",
            Self::OutputIdentity => "output_identity",
            Self::RouteMigrationVerification => "route_migration_verification",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TiffPerfSample {
    pub phase: TiffPerfPhase,
    pub elapsed: Duration,
    pub logical_bytes: Option<u64>,
}

impl TiffPerfSample {
    pub fn throughput_mib_per_second(&self) -> Option<f64> {
        self.logical_bytes
            .and_then(|bytes| throughput_mib_per_second(bytes, self.elapsed))
    }
}

#[derive(Clone, Debug)]
pub struct TiffPerfReport {
    pub operation: String,
    pub elapsed: Duration,
    pub samples: Vec<TiffPerfSample>,
}

impl TiffPerfReport {
    pub fn format_text(&self) -> String {
        let mut output = format!(
            "[tiff-perf] operation={} total_ms={:.3}",
            self.operation,
            self.elapsed.as_secs_f64() * 1000.0
        );
        for sample in &self.samples {
            append_sample_text(&mut output, sample, true);
        }
        output
    }
}

#[derive(Debug)]
pub struct TiffPerfTrace {
    operation: String,
    started: Instant,
    samples: Vec<TiffPerfSample>,
}

impl TiffPerfTrace {
    pub fn new(operation: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            started: Instant::now(),
            samples: Vec::new(),
        }
    }

    pub fn measure<T, E, F>(
        &mut self,
        phase: TiffPerfPhase,
        logical_bytes: Option<u64>,
        action: F,
    ) -> Result<T, E>
    where
        F: FnOnce() -> Result<T, E>,
    {
        let started = Instant::now();
        let result = action();
        self.samples.push(TiffPerfSample {
            phase,
            elapsed: started.elapsed(),
            logical_bytes,
        });
        result
    }

    pub fn record(
        &mut self,
        phase: TiffPerfPhase,
        elapsed: Duration,
        logical_bytes: Option<u64>,
    ) {
        self.samples.push(TiffPerfSample {
            phase,
            elapsed,
            logical_bytes,
        });
    }

    pub fn finish(self) -> TiffPerfReport {
        TiffPerfReport {
            operation: self.operation,
            elapsed: self.started.elapsed(),
            samples: self.samples,
        }
    }
}

pub fn enabled() -> bool {
    env_flag_enabled("SHADE_TIFF_PERF") || std::env::var_os("SHADE_TIFF_PERF_LOG").is_some()
}

pub fn emit_if_enabled(report: &TiffPerfReport) {
    if enabled() {
        emit_text(&report.format_text());
    }
}

/// Emit one independently measured low-level phase. This is used at shared
/// filesystem boundaries where there is no operation-owned `TiffPerfTrace`
/// available, while retaining the same stable phase labels and MiB/s math.
pub fn emit_phase_if_enabled(
    operation: &str,
    phase: TiffPerfPhase,
    elapsed: Duration,
    logical_bytes: Option<u64>,
) {
    if !enabled() {
        return;
    }
    let sample = TiffPerfSample {
        phase,
        elapsed,
        logical_bytes,
    };
    let mut line = format!("[tiff-perf] operation={operation}");
    append_sample_text(&mut line, &sample, false);
    emit_text(&line);
}

fn emit_text(text: &str) {
    if let Some(file) = perf_log_file() {
        if let Ok(mut file) = file.lock() {
            for line in text.lines() {
                let _ = writeln!(file, "{line}");
            }
            return;
        }
    }
    eprintln!("{text}");
}

fn perf_log_file() -> Option<&'static Mutex<File>> {
    PERF_LOG_FILE
        .get_or_init(|| {
            let path = std::env::var_os("SHADE_TIFF_PERF_LOG").map(PathBuf::from)?;
            if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
                let _ = std::fs::create_dir_all(parent);
            }
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
                .map(Mutex::new)
        })
        .as_ref()
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty() && value != "0")
}

fn append_sample_text(output: &mut String, sample: &TiffPerfSample, leading_newline: bool) {
    if leading_newline {
        output.push('\n');
    } else {
        output.push(' ');
    }
    output.push_str(&format!(
        "[tiff-perf] phase={} ms={:.3}",
        sample.phase.label(),
        sample.elapsed.as_secs_f64() * 1000.0
    ));
    if let Some(bytes) = sample.logical_bytes {
        output.push_str(&format!(" bytes={bytes}"));
    }
    if let Some(rate) = sample.throughput_mib_per_second() {
        output.push_str(&format!(" mib_s={rate:.2}"));
    }
}

pub fn throughput_mib_per_second(bytes: u64, elapsed: Duration) -> Option<f64> {
    let seconds = elapsed.as_secs_f64();
    if bytes == 0 || seconds <= 0.0 || !seconds.is_finite() {
        return None;
    }
    Some(bytes as f64 / MIB / seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throughput_uses_binary_mebibytes() {
        let rate = throughput_mib_per_second(10 * 1024 * 1024, Duration::from_secs(2)).unwrap();
        assert!((rate - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn zero_duration_or_zero_bytes_has_no_rate() {
        assert_eq!(throughput_mib_per_second(1024, Duration::ZERO), None);
        assert_eq!(
            throughput_mib_per_second(0, Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn measure_records_failed_actions_too() {
        let mut trace = TiffPerfTrace::new("fixture");
        let result: Result<(), &'static str> = trace.measure(
            TiffPerfPhase::CompressionEncode,
            Some(1024),
            || Err("failed"),
        );
        assert_eq!(result, Err("failed"));
        let report = trace.finish();
        assert_eq!(report.samples.len(), 1);
        assert_eq!(report.samples[0].phase, TiffPerfPhase::CompressionEncode);
    }

    #[test]
    fn report_contains_stable_machine_readable_labels() {
        let mut trace = TiffPerfTrace::new("export");
        trace.record(
            TiffPerfPhase::SourceSpoolWrite,
            Duration::from_millis(250),
            Some(1024 * 1024),
        );
        let text = trace.finish().format_text();
        assert!(text.contains("operation=export"));
        assert!(text.contains("phase=source_spool_write"));
        assert!(text.contains("bytes=1048576"));
    }

    #[test]
    fn single_phase_format_uses_same_stable_label() {
        let sample = TiffPerfSample {
            phase: TiffPerfPhase::FinalDurability,
            elapsed: Duration::from_millis(10),
            logical_bytes: Some(1024 * 1024),
        };
        let mut text = String::new();
        append_sample_text(&mut text, &sample, false);
        assert!(text.contains("phase=final_durability"));
        assert!(text.contains("bytes=1048576"));
    }
}
