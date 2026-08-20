from pathlib import Path

path = Path("work/src/conversion_queue.rs")
text = path.read_text(encoding="utf-8")

old = '''        if !matches!(
            item.status,
            ConversionQueueStatus::Failed
                | ConversionQueueStatus::Cancelled
                | ConversionQueueStatus::NeedsRecovery
        ) {
            return false;
        }
'''
new = '''        if !matches!(
            item.status,
            ConversionQueueStatus::Failed | ConversionQueueStatus::Cancelled
        ) {
            return false;
        }
'''
if text.count(old) != 1:
    raise SystemExit(f"retry status marker count={text.count(old)}")
text = text.replace(old, new, 1)

marker = '''    #[test]
    fn enqueue_with_append_disposition_persists_exact_destination_intent() {
'''
if text.count(marker) != 1:
    raise SystemExit(f"append test marker count={text.count(marker)}")

test = r'''    #[test]
    fn retry_needs_recovery_is_blocked_and_preserves_committed_state() {
        let mut queue = ConversionQueue::new();
        let id = queue
            .enqueue(capture(r"C:\Production\recovery.tif"), 220.0)
            .unwrap();
        let recovery = ConversionRecoveryRecord {
            committed_output: CommittedConversionOutput {
                path: PathBuf::from(r"C:\Production\recovery.tif"),
                sha256: HASH.to_owned(),
                converted_at_unix_ms: 42,
            },
            production_project_path: PathBuf::from(r"C:\Production\recovery.shade"),
            production_project: Some(ShadeProject::default()),
            error: "simulated post-commit project save failure".to_owned(),
        };
        let item = queue.items.iter_mut().find(|item| item.id == id).unwrap();
        item.status = ConversionQueueStatus::NeedsRecovery;
        item.error = Some(recovery.error.clone());
        item.recovery = Some(recovery.clone());

        assert!(!queue.retry(id));

        let item = queue.items.iter().find(|item| item.id == id).unwrap();
        assert_eq!(item.status, ConversionQueueStatus::NeedsRecovery);
        let preserved = item.recovery.as_ref().expect("recovery state must remain");
        assert_eq!(preserved.committed_output.path, recovery.committed_output.path);
        assert_eq!(preserved.committed_output.sha256, recovery.committed_output.sha256);
        assert_eq!(preserved.production_project_path, recovery.production_project_path);
        assert!(preserved.production_project.is_some());
        assert_eq!(preserved.error, recovery.error);
    }

'''
text = text.replace(marker, test + marker, 1)
path.write_text(text, encoding="utf-8", newline="\n")
