#![cfg(windows)]

use windows_shade_editor::icc_profile_registry::IccProfileRole;

#[test]
fn output_and_devicelink_roles_remain_distinct_for_shared_profile_browsers() {
    assert_ne!(IccProfileRole::Output, IccProfileRole::DeviceLink);
    assert_eq!(IccProfileRole::Output.label(), "Output / printer");
    assert_eq!(IccProfileRole::DeviceLink.label(), "DeviceLink");
}
