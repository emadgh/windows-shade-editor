use serde::{Deserialize, Serialize};

/// Explicit, reproducible handling for source alpha before production color conversion.
///
/// The background values are encoded in the source color space as full-range u16 RGB
/// samples. Selecting a policy is always an operator action; there is intentionally no
/// implicit/default flattening policy for alpha-bearing artwork.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SourceTransparencyPolicy {
    FlattenSolidRgb16 { background_rgb: [u16; 3] },
}

impl SourceTransparencyPolicy {
    pub fn background_rgb(self) -> [u16; 3] {
        match self {
            Self::FlattenSolidRgb16 { background_rgb } => background_rgb,
        }
    }

    pub fn label(self) -> String {
        let [r, g, b] = self.background_rgb();
        format!("Flatten alpha on RGB16 background ({r}, {g}, {b})")
    }
}

/// Straight-alpha compositing in full-range u16 sample space with deterministic
/// round-to-nearest behavior. The calculation uses u64 intermediates so endpoint
/// products cannot overflow.
pub fn composite_channel_u16(foreground: u16, alpha: u16, background: u16) -> u16 {
    const MAX: u64 = u16::MAX as u64;
    let alpha = alpha as u64;
    let inverse = MAX - alpha;
    let numerator = foreground as u64 * alpha + background as u64 * inverse + MAX / 2;
    (numerator / MAX) as u16
}

pub fn composite_rgb_u16(
    foreground: [u16; 3],
    alpha: u16,
    policy: SourceTransparencyPolicy,
) -> [u16; 3] {
    let background = policy.background_rgb();
    [
        composite_channel_u16(foreground[0], alpha, background[0]),
        composite_channel_u16(foreground[1], alpha, background[1]),
        composite_channel_u16(foreground[2], alpha, background[2]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_endpoints_preserve_foreground_or_background_exactly() {
        let policy = SourceTransparencyPolicy::FlattenSolidRgb16 {
            background_rgb: [1000, 2000, 3000],
        };
        let foreground = [50000, 40000, 30000];
        assert_eq!(composite_rgb_u16(foreground, u16::MAX, policy), foreground);
        assert_eq!(composite_rgb_u16(foreground, 0, policy), [1000, 2000, 3000]);
    }

    #[test]
    fn half_alpha_uses_deterministic_full_range_rounding() {
        assert_eq!(composite_channel_u16(u16::MAX, 32768, 0), 32768);
        assert_eq!(composite_channel_u16(0, 32768, u16::MAX), 32767);
    }

    #[test]
    fn policy_round_trips_without_semantic_loss() {
        let policy = SourceTransparencyPolicy::FlattenSolidRgb16 {
            background_rgb: [65535, 32768, 0],
        };
        let json = serde_json::to_string(&policy).expect("serialize policy");
        let restored: SourceTransparencyPolicy =
            serde_json::from_str(&json).expect("deserialize policy");
        assert_eq!(restored, policy);
    }
}
