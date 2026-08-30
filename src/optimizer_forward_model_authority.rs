use crate::color_conversion::ConversionTargetDefinition;
use crate::output_icc_forward_model::output_icc_forward_model_id;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptimizerForwardModelAuthorityError {
    MissingAuthority,
    InvalidOutputProfileIdentity(String),
}

/// Resolve the exact forward-model identity for Custom Optimizer candidate evaluation.
///
/// Measured characterization remains authoritative when present. When it is absent,
/// the exact Output ICC SHA-256 becomes the model identity. This function does not
/// grant measured-production approval; it only binds solver candidates to the same
/// profile bytes that define their device→PCS behavior.
pub fn optimizer_forward_model_identity(
    target: &ConversionTargetDefinition,
) -> Result<String, OptimizerForwardModelAuthorityError> {
    if let Some(identity) = target
        .characterization_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(identity.to_owned());
    }

    let profile = target
        .output_profile_identity
        .as_ref()
        .ok_or(OptimizerForwardModelAuthorityError::MissingAuthority)?;
    output_icc_forward_model_id(profile)
        .map_err(OptimizerForwardModelAuthorityError::InvalidOutputProfileIdentity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_conversion::TargetChannelDefinition;
    use crate::model::IccProfileIdentity;

    fn target() -> ConversionTargetDefinition {
        ConversionTargetDefinition {
            name: "Ceramic 4C".to_owned(),
            channels: ["Blue", "Brown", "Beige", "Black"]
                .into_iter()
                .map(|name| TargetChannelDefinition {
                    name: name.to_owned(),
                    display_rgb: None,
                    solidity: 1.0,
                    max_coverage: None,
                })
                .collect(),
            bit_depth: 16,
            output_profile_identity: Some(IccProfileIdentity {
                description: "Ceramic".to_owned(),
                sha256: "a".repeat(64),
            }),
            output_profile_path: Some("Ceramic.icc".to_owned()),
            device_link_identity: None,
            device_link_path: None,
            characterization_id: None,
            total_ink_limit: None,
        }
    }

    #[test]
    fn measured_authority_wins_when_both_are_present() {
        let mut target = target();
        target.characterization_id = Some("measured-v2".to_owned());
        assert_eq!(
            optimizer_forward_model_identity(&target).unwrap(),
            "measured-v2"
        );
    }

    #[test]
    fn output_icc_sha_is_authority_when_measurement_is_absent() {
        assert_eq!(
            optimizer_forward_model_identity(&target()).unwrap(),
            format!("output-icc-sha256:{}", "a".repeat(64))
        );
    }

    #[test]
    fn missing_or_malformed_authority_fails_closed() {
        let mut target = target();
        target.output_profile_identity = None;
        assert_eq!(
            optimizer_forward_model_identity(&target),
            Err(OptimizerForwardModelAuthorityError::MissingAuthority)
        );

        target.output_profile_identity = Some(IccProfileIdentity {
            description: "Ceramic".to_owned(),
            sha256: "short".to_owned(),
        });
        assert!(matches!(
            optimizer_forward_model_identity(&target),
            Err(OptimizerForwardModelAuthorityError::InvalidOutputProfileIdentity(_))
        ));
    }
}
