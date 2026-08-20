#![cfg(windows)]

use std::path::PathBuf;

use windows_shade_editor::production_destination_selection::FrozenProductionDestination;
use windows_shade_editor::production_project_disposition::ProductionProjectDisposition;

#[test]
fn create_new_destination_freezes_only_a_shade_project_path() {
    let path = PathBuf::from(r"C:\Production\Job.shade");
    let frozen = FrozenProductionDestination::create_new(path.clone()).unwrap();

    assert_eq!(frozen.production_project_path, path);
    assert_eq!(frozen.disposition, ProductionProjectDisposition::CreateNew);
    assert!(FrozenProductionDestination::create_new(PathBuf::from(r"C:\Production\Job.tif")).is_err());
}
