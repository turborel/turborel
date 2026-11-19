//! \[EXTENSION\] The plug-in points of work on another branch, mirrored
//! here as its design gives them, so the adapter of `backup.rs` is written
//! whole against them: backups' `BackupTarget` and `TargetProvider`
//! (D160-D166). When that branch is merged, this file gives way to its own
//! traits and the adapter implements those, unchanged but for its paths.

use std::sync::Arc;

use crate::error::Result;

/// Where backups and archived log go: a flat store of objects.
pub trait BackupTarget: Send + Sync {
    /// Its location, as named in SQL and settings.
    fn location(&self) -> String;
    /// Begins writing object `name`, giving an upload's handle.
    ///
    /// # Errors
    ///
    /// The target's.
    fn begin(&self, name: &str) -> Result<u64>;
    /// Appends `bytes` to upload `upload`.
    ///
    /// # Errors
    ///
    /// The target's.
    fn write(&self, upload: u64, bytes: &[u8]) -> Result<()>;
    /// Makes the upload its object, durably.
    ///
    /// # Errors
    ///
    /// The target's.
    fn finish(&self, upload: u64) -> Result<()>;
    /// Abandons an upload, leaving no object.
    ///
    /// # Errors
    ///
    /// The target's.
    fn abandon(&self, upload: u64) -> Result<()>;
    /// Up to `max` bytes of `name` from `offset`, empty at its end.
    ///
    /// # Errors
    ///
    /// The target's.
    fn read(&self, name: &str, offset: u64, max: u32) -> Result<Vec<u8>>;
    /// The names of the objects beginning with `prefix`, sorted.
    ///
    /// # Errors
    ///
    /// The target's.
    fn list(&self, prefix: &str) -> Result<Vec<String>>;
    /// Removes object `name`, if it exists.
    ///
    /// # Errors
    ///
    /// The target's.
    fn remove(&self, name: &str) -> Result<()>;
}

/// Opens targets of one scheme from a location.
pub trait TargetProvider: Send + Sync {
    /// The scheme, before the colon of a location.
    fn scheme(&self) -> &str;
    /// The target at `location`.
    ///
    /// # Errors
    ///
    /// The provider's, of a location it cannot open.
    fn open(&self, location: &str) -> Result<Arc<dyn BackupTarget>>;
}
