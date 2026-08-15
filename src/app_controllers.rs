use std::path::PathBuf;

use crate::color_management::InstalledIccProfile;
use crate::export_queue::ExportQueue;
use crate::tiff_inspect::TiffInspection;

pub struct ExportController {
    pub show_all: bool,
    pub all_folder: String,
    pub show_queue: bool,
    pub queue: ExportQueue,
    pub open_folder_after: Option<PathBuf>,
    pub remind_after_export: bool,
    pub show_snapshot_save_reminder: bool,
}

impl ExportController {
    pub fn new(queue: ExportQueue) -> Self {
        let show_queue = queue.restored_count() > 0;
        Self {
            show_all: false,
            all_folder: String::new(),
            show_queue,
            queue,
            open_folder_after: None,
            remind_after_export: false,
            show_snapshot_save_reminder: false,
        }
    }
}

#[derive(Default)]
pub struct ColorManagementController {
    pub show: bool,
    pub query: String,
    pub profiles: Vec<InstalledIccProfile>,
    pub selected: Option<String>,
    pub scan_done: bool,
    pub scan_error: Option<String>,
    pub show_incompatible: bool,
}

#[derive(Default)]
pub struct TiffInspectorController {
    pub show: bool,
    pub inspection: Option<TiffInspection>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracted_controllers_start_closed_and_idle() {
        let export = ExportController::new(ExportQueue::new());
        assert!(!export.show_all);
        assert!(!export.show_queue);
        assert!(!export.queue.has_pending());

        let color = ColorManagementController::default();
        assert!(!color.show);
        assert!(color.profiles.is_empty());

        let inspector = TiffInspectorController::default();
        assert!(!inspector.show);
        assert!(inspector.inspection.is_none());
    }
}
