use lcms2::Profile;
use sha2::{Digest, Sha256};

use crate::model::IccProfileIdentity;

/// Explicit production interpretation used only for otherwise-untagged RGB artwork.
///
/// The generated LittleCMS sRGB payload is hashed and stored in the conversion recipe,
/// so using the fallback remains deterministic and auditable rather than an unmanaged guess.
pub const SRGB_FALLBACK_DESCRIPTION: &str = "sRGB fallback (untagged RGB)";

pub fn srgb_fallback_icc() -> Result<Vec<u8>, String> {
    Profile::new_srgb()
        .icc()
        .map_err(|error| format!("Cannot materialize built-in sRGB fallback ICC: {error}"))
}

pub fn srgb_fallback_identity() -> Result<IccProfileIdentity, String> {
    let bytes = srgb_fallback_icc()?;
    Ok(IccProfileIdentity {
        description: SRGB_FALLBACK_DESCRIPTION.to_owned(),
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    })
}

pub fn is_srgb_fallback_identity(identity: &IccProfileIdentity) -> bool {
    is_srgb_fallback_sha256(&identity.sha256)
        && identity.description.eq_ignore_ascii_case(SRGB_FALLBACK_DESCRIPTION)
}

pub fn is_srgb_fallback_sha256(sha256: &str) -> bool {
    srgb_fallback_identity().is_ok_and(|fallback| {
        fallback.sha256.eq_ignore_ascii_case(sha256.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_identity_is_stable_and_self_verifying() {
        let first = srgb_fallback_identity().unwrap();
        let second = srgb_fallback_identity().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.sha256.len(), 64);
        assert!(is_srgb_fallback_identity(&first));
        assert!(is_srgb_fallback_sha256(&first.sha256));
    }
}
