//! The modes of a transaction [ISO/IEC 9075:1992 §14.1]: its isolation
//! level, its access mode and its diagnostics size.

use super::ValueSpecification;

/// [ISO/IEC 9075:1992 §14.1] The `<transaction mode>`s of a `<set
/// transaction statement>`, each written at most once \[SR1\].
#[derive(Debug, Clone, Default)]
pub struct TransactionModes {
    /// `<isolation level>`, if written.
    pub isolation: Option<IsolationLevel>,
    /// `<transaction access mode>`, if written.
    pub access: Option<AccessMode>,
    /// `<diagnostics size>`: its `<number of conditions>`, a `<simple value
    /// specification>`, if written.
    pub diagnostics: Option<ValueSpecification>,
}

/// [ISO/IEC 9075:1992 §14.1] `<level of isolation>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationLevel {
    /// READ UNCOMMITTED.
    ReadUncommitted,
    /// READ COMMITTED.
    ReadCommitted,
    /// REPEATABLE READ.
    RepeatableRead,
    /// SERIALIZABLE.
    Serializable,
}

/// [ISO/IEC 9075:1992 §14.1] `<transaction access mode>` ::= READ ONLY |
/// READ WRITE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    /// READ ONLY.
    ReadOnly,
    /// READ WRITE.
    ReadWrite,
}
