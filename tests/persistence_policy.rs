const MAIN_SOURCE: &str = include_str!("../src/main.rs");
const CONVERSION_QUEUE_SOURCE: &str = include_str!("../src/conversion_queue.rs");

#[test]
fn active_source_has_no_timer_autosave_writer() {
    let forbidden = concat!("project_", "autosave");
    assert!(!MAIN_SOURCE.contains(forbidden));
    assert!(!MAIN_SOURCE.contains("maybe_project_autosave"));
    assert!(!MAIN_SOURCE.contains("poll_project_autosave"));
}

#[test]
fn active_source_save_routes_through_explicit_boundary() {
    assert_eq!(
        MAIN_SOURCE
            .matches("project_persistence::save_active_source_project")
            .count(),
        1
    );
    assert!(!MAIN_SOURCE.contains("project.save(&path, &face_paths)"));
}

#[test]
fn conversion_source_link_never_marks_project_saved() {
    let marker = "production_project::link_source_project_to_production";
    let start = MAIN_SOURCE
        .find(marker)
        .expect("conversion link call must exist");
    let end = (start + 900).min(MAIN_SOURCE.len());
    let block = &MAIN_SOURCE[start..end];
    assert!(block.contains("self.mark_project_dirty()"));
    assert!(!block.contains("self.mark_project_saved()"));
}

#[test]
fn conversion_queue_never_persists_reciprocal_source_link() {
    assert!(
        !CONVERSION_QUEUE_SOURCE.contains("commit_source_project_link"),
        "conversion queue must not own a hidden Source-project persistence path"
    );
    assert!(
        !CONVERSION_QUEUE_SOURCE.contains("ShadeProject::load(&capture.source_project_path)"),
        "conversion queue must not reopen the Source project to persist lineage"
    );
    assert!(
        !CONVERSION_QUEUE_SOURCE.contains("source.save(&capture.source_project_path"),
        "conversion queue must leave Source .shade persistence to explicit Save"
    );
}

#[test]
fn save_payload_name_is_staged_before_live_project_identity_changes() {
    assert!(MAIN_SOURCE.contains("project.name = project_name_for_path(&project.name, &path);"));
    assert_eq!(
        MAIN_SOURCE
            .matches("self.project.name = project_name_for_path")
            .count(),
        1,
        "live project name should only change in the successful Save completion path"
    );
}

#[test]
fn recovery_autosave_remains_separate() {
    assert!(MAIN_SOURCE.contains("fn maybe_autosave(&mut self)"));
    assert!(MAIN_SOURCE.contains("fn poll_autosave(&mut self)"));
}
