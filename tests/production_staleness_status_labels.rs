#![cfg(windows)]

use std::collections::BTreeSet;

use windows_shade_editor::production_staleness::SourceConversionStatus;

#[test]
fn production_staleness_status_labels_are_non_empty_and_distinct() {
    let statuses = [
        SourceConversionStatus::NoPriorConversion,
        SourceConversionStatus::UpToDate,
        SourceConversionStatus::SourceChanged,
        SourceConversionStatus::ProductionOutputMissing,
        SourceConversionStatus::ProductionOutputChanged,
        SourceConversionStatus::ProductionLineageAmbiguous,
        SourceConversionStatus::TargetNoLongerCompatible,
    ];
    let labels = statuses
        .iter()
        .map(|status| status.label())
        .collect::<Vec<_>>();

    assert!(labels.iter().all(|label| !label.trim().is_empty()));
    assert_eq!(
        labels.iter().copied().collect::<BTreeSet<_>>().len(),
        labels.len(),
        "each Source conversion state must remain distinguishable to the operator"
    );
}
