from pathlib import Path

path = Path('src/model.rs')
text = path.read_text(encoding='utf-8')
old = r'''    #[test]
    fn snapshot_export_history_is_per_face_and_replaceable() {
        let mut project = ShadeProject::default();
        let id = project.create_snapshot();
        assert!(project.record_snapshot_export(
            id,
            "face-a.tif".to_owned(),
            r"C:\exports\one".to_owned(),
            100,
        ));
        assert_eq!(
            project
                .snapshot_export_for_face(id, "face-a.tif")
                .unwrap()
                .folder,
            r"C:\exports\one"
        );
        assert!(project.record_snapshot_export(
            id,
            "face-a.tif".to_owned(),
            r"C:\exports\two".to_owned(),
            200,
        ));
        let record = project.snapshot_export_for_face(id, "face-a.tif").unwrap();
        assert_eq!(record.folder, r"C:\exports\two");
        assert_eq!(record.exported_at_unix_ms, 200);
        assert_eq!(project.snapshots[0].exports.len(), 1);
    }
'''
new = r'''    #[test]
    fn snapshot_export_history_preserves_prior_codes_and_returns_latest_per_face() {
        let mut project = ShadeProject::default();
        let id = project.create_snapshot();
        assert!(project.record_snapshot_export_with_identity(
            id,
            "face-a.tif".to_owned(),
            r"C:\exports\one".to_owned(),
            100,
            "TEST-41".to_owned(),
            "a".repeat(64),
            r"C:\exports\one\TEST-41.tif".to_owned(),
        ));
        let first = project
            .snapshot_export_for_face(id, "face-a.tif")
            .expect("first committed export");
        assert_eq!(first.test_code, "TEST-41");
        assert_eq!(first.adjustment_sha256, "a".repeat(64));
        assert_eq!(first.destination, r"C:\exports\one\TEST-41.tif");

        assert!(project.record_snapshot_export_with_identity(
            id,
            "face-a.tif".to_owned(),
            r"C:\exports\two".to_owned(),
            200,
            "TEST-42".to_owned(),
            "b".repeat(64),
            r"C:\exports\two\TEST-42.tif".to_owned(),
        ));

        let snapshot = project
            .snapshots
            .iter()
            .find(|snapshot| snapshot.id == id)
            .expect("snapshot");
        assert_eq!(snapshot.exports.len(), 2);
        assert_eq!(snapshot.exports[0].test_code, "TEST-41");
        assert_eq!(snapshot.exports[1].test_code, "TEST-42");

        let latest = project
            .snapshot_export_for_face(id, "face-a.tif")
            .expect("latest committed export");
        assert_eq!(latest.folder, r"C:\exports\two");
        assert_eq!(latest.exported_at_unix_ms, 200);
        assert_eq!(latest.test_code, "TEST-42");
        assert_eq!(latest.adjustment_sha256, "b".repeat(64));
        assert_eq!(latest.destination, r"C:\exports\two\TEST-42.tif");
    }
'''
if text.count(old) != 1:
    raise SystemExit(f'expected exactly one old test, found {text.count(old)}')
path.write_text(text.replace(old, new, 1), encoding='utf-8')
print('updated snapshot provenance regression test')
