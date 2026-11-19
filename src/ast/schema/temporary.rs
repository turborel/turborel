//! Abstract syntax of a temporary table's kind [ISO/IEC 9075:1992 §4.9].

/// [ISO/IEC 9075:1992 §4.9] A temporary table's kind, and what a commit
/// does to its rows [§11.3 SR4, §14.3 GR3].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Temporary {
    /// Its kind.
    pub kind: TemporaryKind,
    /// ON COMMIT DELETE ROWS, rather than PRESERVE ROWS.
    pub delete_rows: bool,
}

/// [ISO/IEC 9075:1992 §4.9] The kinds of temporary table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporaryKind {
    /// GLOBAL TEMPORARY: a global temporary table.
    Global,
    /// LOCAL TEMPORARY: a created local temporary table.
    CreatedLocal,
    /// DECLARE LOCAL TEMPORARY TABLE: a declared local temporary table
    /// [§13.11].
    Declared,
}
