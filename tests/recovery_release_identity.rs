#![cfg(windows)]

#[test]
fn project_only_recovery_release_uses_v02150_identity() {
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.21.50");
}
