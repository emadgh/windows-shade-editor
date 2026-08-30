use std::path::Path;

use crate::conversion_transaction::{
    CommittedConversionOutput, ConversionCancellation, ConversionJobCapture, ConversionProgress,
    ConversionTransactionBackend,
};
use crate::model::ShadeProject;

/// The existing measured/ICC/DeviceLink worker remains isolated in this core
/// module. Keeping it intact makes the profile-backed authority additive rather
/// than relaxing any #191/#205 execution boundary.
mod core {
    include!("icc_conversion_worker_core.rs");

    pub(super) mod profile_backed {
        use super::*;
        include!("icc_conversion_worker/profile_backed.rs");
    }
}

pub use core::sha256_file;

/// Public filesystem backend that routes only an explicitly captured
/// profile-backed authority to the new profile execution path. Every existing
/// job delegates to the unchanged core backend.
pub struct FilesystemIccConversionBackend {
    core: core::FilesystemIccConversionBackend,
}

impl FilesystemIccConversionBackend {
    pub fn new(default_dpi: f64) -> Result<Self, String> {
        Ok(Self {
            core: core::FilesystemIccConversionBackend::new(default_dpi)?,
        })
    }
}

impl ConversionTransactionBackend for FilesystemIccConversionBackend {
    fn render_convert_and_commit(
        &mut self,
        capture: &ConversionJobCapture,
        cancellation: &ConversionCancellation,
        report: &mut dyn FnMut(ConversionProgress),
    ) -> Result<CommittedConversionOutput, String> {
        if capture.profile_backed_optimizer_execution.is_some() {
            core::profile_backed::render_convert_and_commit_profile_backed(
                &mut self.core,
                capture,
                cancellation,
                report,
            )
        } else {
            ConversionTransactionBackend::render_convert_and_commit(
                &mut self.core,
                capture,
                cancellation,
                report,
            )
        }
    }

    fn save_production_project(
        &mut self,
        path: &Path,
        project: &ShadeProject,
    ) -> Result<(), String> {
        ConversionTransactionBackend::save_production_project(&mut self.core, path, project)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_preserves_original_backend_construction_contract() {
        assert!(FilesystemIccConversionBackend::new(f64::NAN).is_err());
        assert!(FilesystemIccConversionBackend::new(0.0).is_err());
        assert!(FilesystemIccConversionBackend::new(220.0).is_ok());
    }

    #[test]
    fn profile_dispatch_contains_no_measured_authority_fallback() {
        let source = include_str!("icc_conversion_worker.rs");
        assert!(source.contains("profile_backed_optimizer_execution.is_some()"));
        assert!(source.contains("render_convert_and_commit_profile_backed"));
        assert!(!source.contains("load_and_authorize_custom_optimizer_evidence"));
    }
}
