pub use update_via_github::{UpdateInfo, UpdateStatus};
use update_via_github::UpdateConfig;

const REPOSITORY: &str = "emadgh/windows-shade-editor";
const ASSET_NAME: &str = "ShadeEditor.exe";
const CHECKSUM_ASSET_NAME: &str = "ShadeEditor.exe.sha256";
const MAX_DOWNLOAD_SIZE: usize = 200 * 1024 * 1024;

#[derive(Clone)]
pub struct UpdateManager {
    inner: update_via_github::UpdateManager,
}

impl Default for UpdateManager {
    fn default() -> Self {
        let config = UpdateConfig::new(REPOSITORY, ASSET_NAME, env!("CARGO_PKG_VERSION"))
            .with_app_name("ShadeEditor")
            .with_checksum_asset(CHECKSUM_ASSET_NAME)
            .with_required_checksum(true)
            .with_max_download_size(MAX_DOWNLOAD_SIZE);
        Self {
            inner: update_via_github::UpdateManager::new(config),
        }
    }
}

impl UpdateManager {
    pub fn status(&self) -> UpdateStatus {
        self.inner.status()
    }

    pub fn start_check(&self, auto_download: bool) -> bool {
        self.inner.start_check(auto_download)
    }

    pub fn start_download(&self) -> bool {
        self.inner.start_download()
    }

    pub fn apply_ready(&self) -> Result<bool, String> {
        self.inner.apply_ready()
    }
}
