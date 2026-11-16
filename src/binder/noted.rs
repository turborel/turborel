//! What a binding notes of what it references, where its `Context` says:
//! the columns of named tables and views [ISO/IEC 9075:1992 §21.3.12,
//! §21.3.18], those of each select list item, the subject routines it
//! invokes [ISO/IEC 9075-2:2023 §4.35.2], and the sequence generators it
//! draws from [§11.74 SR3], each for a definition to keep.

use std::cell::RefCell;

use crate::catalog::QualifiedName;

/// The columns a query references, each as its table or view and column
/// name, in the order referenced, once or more.
pub type Uses = RefCell<Vec<(QualifiedName, String)>>;

/// The columns each item of a select list references, as `Uses` notes
/// them, its subqueries' too.
pub type Selected = RefCell<Vec<Vec<(QualifiedName, String)>>>;

/// The specific names of the SQL-invoked routines a statement invokes.
pub type Invoked = RefCell<Vec<QualifiedName>>;

/// [ISO/IEC 9075-2:2023 §6.14, §11.74 SR3] The names of the sequence
/// generators a statement's NEXT VALUE FOR draws from, once or more.
pub type DrawnFrom = RefCell<Vec<QualifiedName>>;
