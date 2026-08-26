//! Public inspection surface for narrow TIFF helpers that remain internal to
//! the canonical TIFF IO implementation.
//!
//! The binary TIFF inspector historically consumed `declared_ink_names` while
//! `tiff_io` was compiled inside the binary crate, where that helper was
//! `pub(crate)`. With `tiff_io` now library-owned, expose the same behavior
//! through this forwarding API without duplicating raw-tag parsing logic or
//! widening the implementation helper itself.

use std::path::Path;

pub fn declared_ink_names(path: &Path) -> Result<Vec<String>, String> {
    crate::tiff_io::declared_ink_names(path)
}
