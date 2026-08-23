use std::path::{Path, PathBuf};

use crate::file_observer::{self, ExternalFileRole};
use crate::icc_profile_registry_raw as raw;
use crate::model::IccProfileIdentity;

pub use raw::{IccProfileRecord, IccProfileRole, is_profile_path};

/// Shared-observer facade over the ICC parser/cache implementation.
///
/// Generic existence/change tracking is owned by `file_observer`; ICC byte identity remains
/// SHA-256 authoritative and is always freshly verified at production/persistence boundaries.
#[derive(Clone, Copy, Debug, Default)]
pub struct IccProfileRegistry;

impl IccProfileRegistry {
    pub fn inspect(self, path: &Path) -> Result<IccProfileRecord, String> {
        inspect_profile(path)
    }

    pub fn installed(self) -> Result<Vec<IccProfileRecord>, String> {
        installed_profiles()
    }

    pub fn resolve(
        self,
        stored_path: Option<&str>,
        identity: Option<&IccProfileIdentity>,
        profiles: &[IccProfileRecord],
    ) -> Option<PathBuf> {
        resolve_external_profile_path(stored_path, identity, profiles)
    }

    pub fn verify_identity(
        self,
        path: &Path,
        expected: &IccProfileIdentity,
    ) -> Result<IccProfileRecord, String> {
        if expected.sha256.trim().is_empty() {
            return Err("ICC profile has no stored SHA-256 identity. Select it again.".to_owned());
        }
        file_observer::observe(path, ExternalFileRole::IccProfile);
        file_observer::rescan(path);
        let actual = raw::inspect_profile_fresh(path)?;
        if !actual.matches_identity(expected) {
            return Err(format!(
                "ICC profile at {} no longer matches stored profile '{}'. Relink or select it again.",
                path.display(),
                expected.description
            ));
        }
        file_observer::acknowledge(path);
        Ok(actual)
    }
}

/// Browse/search inspection. Native file events invalidate the effective cache boundary by
/// forcing a fresh parse/hash when the shared observer has a sticky change state.
pub fn inspect_profile(path: &Path) -> Result<IccProfileRecord, String> {
    let observed = file_observer::observe(path, ExternalFileRole::IccProfile);
    if observed.is_changed() {
        let profile = raw::inspect_profile_fresh(path)?;
        file_observer::acknowledge(path);
        Ok(profile)
    } else {
        raw::inspect_profile(path)
    }
}

/// Identity-bearing inspection always rereads bytes and establishes a new observed baseline only
/// after the ICC parser/hash succeeds.
pub fn inspect_profile_fresh(path: &Path) -> Result<IccProfileRecord, String> {
    file_observer::observe(path, ExternalFileRole::IccProfile);
    let profile = raw::inspect_profile_fresh(path)?;
    file_observer::acknowledge(path);
    Ok(profile)
}

pub fn installed_profiles() -> Result<Vec<IccProfileRecord>, String> {
    let profiles = raw::installed_profiles()?;
    let mut refreshed = Vec::with_capacity(profiles.len());
    for profile in profiles {
        let observed = file_observer::observe(&profile.path, ExternalFileRole::IccProfile);
        if observed.is_changed() {
            match raw::inspect_profile_fresh(&profile.path) {
                Ok(current) => {
                    file_observer::acknowledge(&profile.path);
                    refreshed.push(current);
                }
                Err(_) => {
                    // Enumeration can race an external delete/replace. Drop the stale row;
                    // a subsequent registry refresh can add a valid profile again.
                }
            }
        } else if observed.is_available() {
            refreshed.push(profile);
        }
    }
    Ok(refreshed)
}

pub fn resolve_external_profile_path(
    stored_path: Option<&str>,
    identity: Option<&IccProfileIdentity>,
    profiles: &[IccProfileRecord],
) -> Option<PathBuf> {
    if let Some(path) = stored_path.map(PathBuf::from) {
        let observed = file_observer::observe(&path, ExternalFileRole::IccProfile);
        if observed.is_available() {
            if identity.is_none() {
                file_observer::acknowledge(&path);
                return Some(path);
            }
            if raw::inspect_profile_fresh(&path)
                .ok()
                .zip(identity)
                .is_some_and(|(profile, expected)| profile.matches_identity(expected))
            {
                file_observer::acknowledge(&path);
                return Some(path);
            }
        }
    }

    let identity = identity?;
    profiles
        .iter()
        .find(|profile| profile.matches_identity(identity))
        .map(|profile| {
            file_observer::observe(&profile.path, ExternalFileRole::IccProfile);
            profile.path.clone()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lcms2::Profile;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_profile(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "shade-observed-icc-{label}-{}-{stamp}.icc",
            std::process::id()
        ));
        let mut profile = Profile::new_srgb();
        profile.save_profile_to_file(&path).unwrap();
        path
    }

    #[test]
    fn facade_registers_and_verifies_fresh_identity() {
        let path = temp_profile("verify");
        let selected = inspect_profile_fresh(&path).unwrap();
        let verified = IccProfileRegistry
            .verify_identity(&path, &selected.identity)
            .unwrap();
        assert!(verified.matches_identity(&selected.identity));
        let state = file_observer::snapshot(&path).unwrap();
        assert!(state.is_available());
        assert!(!state.is_changed());
        file_observer::release(&path, ExternalFileRole::IccProfile);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn resolve_rejects_replaced_bytes_with_old_identity() {
        let path = temp_profile("replace");
        let selected = inspect_profile_fresh(&path).unwrap();
        fs::write(&path, b"not an icc profile").unwrap();
        file_observer::rescan(&path);
        assert!(resolve_external_profile_path(
            Some(path.to_string_lossy().as_ref()),
            Some(&selected.identity),
            &[]
        )
        .is_none());
        file_observer::release(&path, ExternalFileRole::IccProfile);
        let _ = fs::remove_file(path);
    }
}
