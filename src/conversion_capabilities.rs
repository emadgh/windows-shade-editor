use crate::color_conversion::ConversionEngineMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlAvailability {
    /// The selected engine can honor this control directly.
    Available,
    /// The transform contains a fixed/precomputed strategy; the UI may report it
    /// but must not expose an editable control that implies runtime freedom.
    FixedByTransform,
    /// The control is not meaningful for this engine path.
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConversionCapabilities {
    pub rendering_intent: ControlAvailability,
    pub black_point_compensation: ControlAvailability,
    pub black_generation: ControlAvailability,
    pub neutral_strategy: ControlAvailability,
    pub per_ink_bias: ControlAvailability,
    pub total_ink_limit: ControlAvailability,
    pub per_channel_limits: ControlAvailability,
}

impl ConversionCapabilities {
    pub fn expert_separation_controls_available(self) -> bool {
        matches!(self.black_generation, ControlAvailability::Available)
            || matches!(self.neutral_strategy, ControlAvailability::Available)
            || matches!(self.per_ink_bias, ControlAvailability::Available)
    }
}

/// Capability contract used by Conversion UI/preflight.
///
/// The critical invariant is that Standard ICC and DeviceLink paths never
/// masquerade as freely adjustable N-ink optimizers. If an ICC/DeviceLink fixes
/// separation construction, the UI must show the strategy as fixed rather than
/// applying post-transform channel multipliers.
pub fn capabilities_for_engine(mode: ConversionEngineMode) -> ConversionCapabilities {
    match mode {
        ConversionEngineMode::Icc => ConversionCapabilities {
            rendering_intent: ControlAvailability::Available,
            black_point_compensation: ControlAvailability::Available,
            black_generation: ControlAvailability::FixedByTransform,
            neutral_strategy: ControlAvailability::FixedByTransform,
            per_ink_bias: ControlAvailability::Unsupported,
            total_ink_limit: ControlAvailability::FixedByTransform,
            per_channel_limits: ControlAvailability::FixedByTransform,
        },
        ConversionEngineMode::DeviceLink => ConversionCapabilities {
            // DeviceLinks normally encode the device-to-device behavior already;
            // do not imply that source/output intent or BPC can be changed after
            // the link has been authored.
            rendering_intent: ControlAvailability::FixedByTransform,
            black_point_compensation: ControlAvailability::Unsupported,
            black_generation: ControlAvailability::FixedByTransform,
            neutral_strategy: ControlAvailability::FixedByTransform,
            per_ink_bias: ControlAvailability::FixedByTransform,
            total_ink_limit: ControlAvailability::FixedByTransform,
            per_channel_limits: ControlAvailability::FixedByTransform,
        },
        ConversionEngineMode::CustomOptimizer => ConversionCapabilities {
            // The optimizer consumes characterized device data directly. Intent
            // and BPC are not presented as ordinary ICC transform knobs here;
            // color-error constraints belong to the optimizer recipe instead.
            rendering_intent: ControlAvailability::Unsupported,
            black_point_compensation: ControlAvailability::Unsupported,
            black_generation: ControlAvailability::Available,
            neutral_strategy: ControlAvailability::Available,
            per_ink_bias: ControlAvailability::Available,
            total_ink_limit: ControlAvailability::Available,
            per_channel_limits: ControlAvailability::Available,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_icc_does_not_expose_fake_ink_bias_controls() {
        let capabilities = capabilities_for_engine(ConversionEngineMode::Icc);
        assert_eq!(capabilities.per_ink_bias, ControlAvailability::Unsupported);
        assert_eq!(
            capabilities.black_generation,
            ControlAvailability::FixedByTransform
        );
        assert!(!capabilities.expert_separation_controls_available());
    }

    #[test]
    fn device_link_strategy_is_reported_as_fixed() {
        let capabilities = capabilities_for_engine(ConversionEngineMode::DeviceLink);
        assert_eq!(
            capabilities.black_generation,
            ControlAvailability::FixedByTransform
        );
        assert_eq!(
            capabilities.per_ink_bias,
            ControlAvailability::FixedByTransform
        );
        assert_eq!(
            capabilities.rendering_intent,
            ControlAvailability::FixedByTransform
        );
        assert!(!capabilities.expert_separation_controls_available());
    }

    #[test]
    fn custom_optimizer_exposes_real_separation_controls() {
        let capabilities = capabilities_for_engine(ConversionEngineMode::CustomOptimizer);
        assert_eq!(capabilities.black_generation, ControlAvailability::Available);
        assert_eq!(capabilities.per_ink_bias, ControlAvailability::Available);
        assert_eq!(capabilities.total_ink_limit, ControlAvailability::Available);
        assert!(capabilities.expert_separation_controls_available());
    }
}
