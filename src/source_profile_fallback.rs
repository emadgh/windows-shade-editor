use lcms2::Profile;
use sha2::{Digest, Sha256};

use crate::model::IccProfileIdentity;

/// Explicit production interpretation used only for otherwise-untagged RGB artwork.
///
/// The generated LittleCMS sRGB payload is canonicalized before hashing and stored in the
/// conversion recipe, so using the fallback remains deterministic and auditable rather than an
/// unmanaged guess. ICC creation timestamps and optional profile IDs are deliberately normalized
/// because neither changes the colorimetry but both would otherwise make the fallback SHA unstable.
pub const SRGB_FALLBACK_DESCRIPTION: &str = "sRGB fallback (untagged RGB)";

const ICC_HEADER_SIZE: usize = 128;
const ICC_DATETIME_RANGE: std::ops::Range<usize> = 24..36;
const ICC_PROFILE_ID_RANGE: std::ops::Range<usize> = 84..100;
const CANONICAL_DATETIME: [u8; 12] = [
    0x07, 0xE4, // 2020
    0x00, 0x01, // January
    0x00, 0x01, // day 1
    0x00, 0x00, // hour 0
    0x00, 0x00, // minute 0
    0x00, 0x00, // second 0
];

pub fn srgb_fallback_icc() -> Result<Vec<u8>, String> {
    let profile = Profile::new_srgb();
    let mut bytes = profile
        .icc()
        .map_err(|error| format!("Cannot materialize built-in sRGB fallback ICC: {error}"))?;
    if bytes.len() < ICC_HEADER_SIZE {
        return Err(format!(
            "Built-in sRGB fallback ICC is truncated: {} bytes.",
            bytes.len()
        ));
    }
    bytes[ICC_DATETIME_RANGE].copy_from_slice(&CANONICAL_DATETIME);
    bytes[ICC_PROFILE_ID_RANGE].fill(0);
    // Reopen after canonicalization so an invalid header normalization can never reach a recipe.
    Profile::new_icc(&bytes)
        .map_err(|error| format!("Canonical sRGB fallback ICC is invalid: {error}"))?;
    Ok(bytes)
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
        let first_bytes = srgb_fallback_icc().unwrap();
        let second_bytes = srgb_fallback_icc().unwrap();
        assert_eq!(first_bytes, second_bytes);
        assert_eq!(&first_bytes[ICC_DATETIME_RANGE], &CANONICAL_DATETIME);
        assert!(first_bytes[ICC_PROFILE_ID_RANGE].iter().all(|byte| *byte == 0));
        Profile::new_icc(&first_bytes).unwrap();

        let first = srgb_fallback_identity().unwrap();
        let second = srgb_fallback_identity().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.sha256.len(), 64);
        assert!(is_srgb_fallback_identity(&first));
        assert!(is_srgb_fallback_sha256(&first.sha256));
    }
}
