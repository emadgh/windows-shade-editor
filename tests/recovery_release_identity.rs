#![cfg(windows)]

#[test]
fn project_only_recovery_build_uses_repository_version_identity() {
    assert_eq!(
        env!("CARGO_PKG_VERSION"),
        include_str!("../VERSION").trim(),
        "Cargo package version and repository VERSION must remain synchronized"
    );
}
