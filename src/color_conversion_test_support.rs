use crate::device_characterization_model::{
    ForwardModelValidationPolicy, LocalForwardModelConfig, ValidatedLocalForwardModel,
};
use crate::device_characterization_package::{
    CharacterizationMeasurementMetadata, CharacterizationPackage, CharacterizationPayload,
    CharacterizationProductionContext, CharacterizationSample, CharacterizationValidationLevel,
    MeasuredLabColor, ValidatedCharacterizationPackage,
};

pub(crate) fn channel_names() -> Vec<String> {
    ["Cyan", "Magenta", "Yellow", "Black"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

pub(crate) fn validated_characterization_package() -> ValidatedCharacterizationPackage {
    let samples = [
        [0.0, 0.0, 0.0, 0.0],
        [0.25, 0.0, 0.0, 0.0],
        [0.0, 0.25, 0.0, 0.0],
        [0.0, 0.0, 0.25, 0.0],
        [0.0, 0.0, 0.0, 0.25],
        [0.25, 0.25, 0.25, 0.25],
    ]
    .into_iter()
    .map(|coverages| CharacterizationSample {
        coverages: coverages.to_vec(),
        lab: MeasuredLabColor {
            l: 50.0,
            a: 0.0,
            b: 0.0,
        },
    })
    .collect();

    CharacterizationPackage::new(CharacterizationPayload {
        revision: "test-production-d50-v1".to_owned(),
        validation_level: CharacterizationValidationLevel::ProductionValidated,
        output_bit_depth: 16,
        channel_names: channel_names(),
        measured_channel_max_coverage: vec![1.0; 4],
        measured_total_ink_limit: 4.0,
        production_context: CharacterizationProductionContext {
            machine_id: "test-machine".to_owned(),
            rip_name: "test-rip".to_owned(),
            rip_version: "1.0".to_owned(),
            linearization_id: "test-linearization".to_owned(),
            substrate: "test-substrate".to_owned(),
            glaze: None,
            body: None,
            product_family: Some("test".to_owned()),
        },
        measurement: CharacterizationMeasurementMetadata {
            instrument_model: "test-spectro".to_owned(),
            instrument_serial: Some("test-serial".to_owned()),
            illuminant: "D50".to_owned(),
            observer: "2deg".to_owned(),
            measurement_condition: "M1".to_owned(),
            measured_at_unix_ms: Some(1),
            operator_or_lab: Some("test-lab".to_owned()),
        },
        samples,
    })
    .expect("valid characterization fixture")
    .validated()
    .expect("validated characterization fixture")
}

pub(crate) fn characterization_id() -> String {
    validated_characterization_package().identity().id.clone()
}

pub(crate) fn local_model(config: LocalForwardModelConfig) -> ValidatedLocalForwardModel {
    ValidatedLocalForwardModel::build(
        &validated_characterization_package(),
        config,
        ForwardModelValidationPolicy::default(),
    )
    .expect("validated local forward-model fixture")
}

pub(crate) fn default_local_model() -> ValidatedLocalForwardModel {
    local_model(LocalForwardModelConfig {
        neighbor_count: 2,
        distance_power: 2.0,
        max_support_distance: 0.5,
    })
}
