from pathlib import Path

repo = Path(__file__).resolve().parents[1] / "work"
path = repo / "src" / "conversion_queue.rs"
text = path.read_text(encoding="utf-8")

old = """use crate::conversion_transaction::{
    CommittedConversionOutput, CompletedConversionTransaction, ConversionCancellation,
    ConversionJobCapture, ConversionPhase, ConversionTransactionOutcome,
    run_conversion_transaction,
};
"""
new = """use crate::conversion_transaction::{
    CommittedConversionOutput, CompletedConversionTransaction, ConversionCancellation,
    ConversionJobCapture, ConversionPhase, ConversionTransactionOutcome,
};
use crate::conversion_transaction_disposition::run_conversion_transaction_with_disposition;
"""
if text.count(old) != 1:
    raise SystemExit(f"transaction import marker count={text.count(old)}")
text = text.replace(old, new, 1)

old = "use crate::production_project::link_source_project_to_production;\n"
new = old + "use crate::production_project_disposition::ProductionProjectDisposition;\n"
if text.count(old) != 1:
    raise SystemExit(f"production project import marker count={text.count(old)}")
text = text.replace(old, new, 1)

old = """struct QueuedConversionSpec {
    capture: ConversionJobCapture,
    default_dpi: f64,
}
"""
new = """struct QueuedConversionSpec {
    capture: ConversionJobCapture,
    #[serde(default)]
    production_project_disposition: ProductionProjectDisposition,
    default_dpi: f64,
}
"""
if text.count(old) != 1:
    raise SystemExit(f"queued spec marker count={text.count(old)}")
text = text.replace(old, new, 1)

old = """    pub fn enqueue(
        &mut self,
        capture: ConversionJobCapture,
        default_dpi: f64,
    ) -> Result<u64, String> {
        capture.validate()?;
        if !default_dpi.is_finite() || default_dpi <= 0.0 {
            return Err("Conversion fallback DPI must be finite and positive.".to_owned());
        }
        for item in self
            .items
            .iter()
            .filter(|item| item.status.reserves_destination())
        {
            if paths_match(&item.destination, &capture.output_tiff_path)
                || paths_match(
                    &item.production_project_path,
                    &capture.production_project_path,
                )
            {
                return Err("Conversion output or Production project is already reserved by another queued job.".to_owned());
            }
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.items.push(item_from_spec(
            id,
            QueuedConversionSpec {
                capture,
                default_dpi,
            },
            ConversionQueueStatus::Waiting,
            false,
            false,
            None,
            None,
        ));
        self.persist();
        Ok(id)
    }
"""
new = """    pub fn enqueue(
        &mut self,
        capture: ConversionJobCapture,
        default_dpi: f64,
    ) -> Result<u64, String> {
        self.enqueue_with_production_project_disposition(
            capture,
            ProductionProjectDisposition::CreateNew,
            default_dpi,
        )
    }

    pub fn enqueue_with_production_project_disposition(
        &mut self,
        capture: ConversionJobCapture,
        production_project_disposition: ProductionProjectDisposition,
        default_dpi: f64,
    ) -> Result<u64, String> {
        capture.validate()?;
        production_project_disposition.validate()?;
        if !default_dpi.is_finite() || default_dpi <= 0.0 {
            return Err("Conversion fallback DPI must be finite and positive.".to_owned());
        }
        for item in self
            .items
            .iter()
            .filter(|item| item.status.reserves_destination())
        {
            if paths_match(&item.destination, &capture.output_tiff_path)
                || paths_match(
                    &item.production_project_path,
                    &capture.production_project_path,
                )
            {
                return Err("Conversion output or Production project is already reserved by another queued job.".to_owned());
            }
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.items.push(item_from_spec(
            id,
            QueuedConversionSpec {
                capture,
                production_project_disposition,
                default_dpi,
            },
            ConversionQueueStatus::Waiting,
            false,
            false,
            None,
            None,
        ));
        self.persist();
        Ok(id)
    }
"""
if text.count(old) != 1:
    raise SystemExit(f"enqueue marker count={text.count(old)}")
text = text.replace(old, new, 1)

old = """        thread::spawn(move || {
            let capture = spec.capture;
            let worker_tx = tx.clone();
"""
new = """        thread::spawn(move || {
            let capture = spec.capture;
            let production_project_disposition = spec.production_project_disposition;
            let worker_tx = tx.clone();
"""
if text.count(old) != 1:
    raise SystemExit(f"start-next capture marker count={text.count(old)}")
text = text.replace(old, new, 1)

old = """                run_conversion_transaction(&capture, &cancellation, &mut backend, |progress| {
                    let _ = worker_tx.send(ConversionQueueEvent::Progress {
"""
new = """                run_conversion_transaction_with_disposition(
                    &capture,
                    &production_project_disposition,
                    &cancellation,
                    &mut backend,
                    |progress| {
                    let _ = worker_tx.send(ConversionQueueEvent::Progress {
"""
if text.count(old) != 1:
    raise SystemExit(f"transaction dispatch marker count={text.count(old)}")
text = text.replace(old, new, 1)

old = """                    });
                })
            }))
"""
new = """                    });
                },
                )
            }))
"""
if text.count(old) != 1:
    raise SystemExit(f"transaction dispatch close marker count={text.count(old)}")
text = text.replace(old, new, 1)

insert = r'''

    #[test]
    fn queued_spec_without_project_disposition_defaults_to_create_new() {
        let spec = QueuedConversionSpec {
            capture: capture(r"C:\Production\legacy.tif"),
            production_project_disposition: ProductionProjectDisposition::CreateNew,
            default_dpi: 220.0,
        };
        let mut value = serde_json::to_value(&spec).unwrap();
        value
            .as_object_mut()
            .expect("queued spec object")
            .remove("production_project_disposition");
        let restored: QueuedConversionSpec = serde_json::from_value(value).unwrap();
        assert_eq!(
            restored.production_project_disposition,
            ProductionProjectDisposition::CreateNew
        );
    }

    #[test]
    fn enqueue_with_append_disposition_persists_exact_destination_intent() {
        let mut queue = ConversionQueue::new();
        let key = crate::production_project_compat::ProductionCompatibilityKey {
            engine_mode: ConversionEngineMode::Icc,
            output_profile_sha256: Some(HASH.to_owned()),
            device_link_sha256: None,
            characterization_id: None,
            channel_names: vec![
                "Cyan".to_owned(),
                "Magenta".to_owned(),
                "Yellow".to_owned(),
                "Black".to_owned(),
            ],
            bit_depth: 16,
        };
        let disposition = ProductionProjectDisposition::append_existing(HASH.to_owned(), &key)
            .unwrap();
        queue
            .enqueue_with_production_project_disposition(
                capture(r"C:\Production\append.tif"),
                disposition.clone(),
                220.0,
            )
            .unwrap();
        assert_eq!(
            queue.items[0].spec.production_project_disposition,
            disposition
        );
    }
'''
marker = "\n}\n"
pos = text.rfind(marker)
if pos == -1:
    raise SystemExit("tests module closing marker not found")
text = text[:pos] + insert + text[pos:]

path.write_text(text, encoding="utf-8", newline="\n")
