//! \[EXTENSION\] The conditions a package is refused with (design
//! document §13.4), of class 38, subclasses beginning with T this
//! implementation's [ISO/IEC 9075-2:2023 §24.1].

use crate::error::{Code, SqlError, SqlState};

/// \[DESIGN\] A file of a package, or the package, is not what its digest
/// says: it changed since it was packed or installed.
pub const CHANGED: SqlState = SqlState::of_bytes(*b"38T07");
/// \[DESIGN\] A package or index is unsigned, signed by no key trusted,
/// its signature does not verify, or an index has expired.
pub const UNTRUSTED: SqlState = SqlState::of_bytes(*b"38T08");
/// \[DESIGN\] No versions satisfy every requirement: a conflict, a cycle,
/// or a package no version of which is found.
pub const CONFLICT: SqlState = SqlState::of_bytes(*b"38T09");

/// 38T07: `why`.
pub fn changed(why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] the package changed: {why}")).with_state(CHANGED)
}

/// 38T08: `why`.
pub fn untrusted(why: &str) -> SqlError {
    SqlError::new(Code::Privilege, format!("[EXTENSION] the package is not trusted: {why}")).with_state(UNTRUSTED)
}

/// 38T09: `why`.
pub fn conflict(why: &str) -> SqlError {
    SqlError::new(Code::Data, format!("[EXTENSION] the packages cannot be resolved: {why}")).with_state(CONFLICT)
}

/// A package file or index of the wrong form, `why`: 38T07, as what was
/// written is not what is read.
pub fn malformed(why: &str) -> SqlError {
    changed(&format!("it is malformed: {why}"))
}
