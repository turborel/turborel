//! Deterministic index-key projections. A lossy index stores one projection
//! of each original key, then rechecks the visible row's projection and the
//! complete SQL condition. It cannot supply original values or SQL order.

use super::Key;
use crate::error::Result;

/// An immutable projection shared by an index and its readers (D85).
/// Equal original keys must have equal projections. Different keys may
/// collide; one original key always has exactly one projection. The
/// projection remains fixed for the index's lifetime, including reopen.
///
/// Index reference counts belong to `(projected key, row id)` pairs.
/// `rows` and `key_rows` may therefore return extra projection-equality
/// candidates; callers must check original keys on visible row versions.
/// Search entries carry projected keys, and readers keep a row only when
/// its visible key has that projection. Full SQL predicates remain required.
/// Projection and index callbacks must not call into a database/session,
/// and callers hold no storage lock while invoking them (R-ERR-4).
pub trait KeyProjection: Send + Sync {
    /// Projects a complete original key, without side effects.
    ///
    /// # Errors
    ///
    /// The key is malformed or violates the index's declared type contract.
    fn project(&self, key: &Key) -> Result<Key>;
}
