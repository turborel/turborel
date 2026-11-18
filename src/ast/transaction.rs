//! The modes of a transaction [ISO/IEC 9075:1992 §14.1]: its isolation
//! level, its access mode and its diagnostics size. The statements of its
//! savepoints [ISO/IEC 9075-2:2023 §17.5, §17.6, §17.8].

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

/// [ISO/IEC 9075-2:2023 §17.5, §17.6, §17.8] A statement of a savepoint,
/// with the `<savepoint name>` its `<savepoint specifier>` is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavepointStatement {
    /// [§17.5] `<savepoint statement>` ::= SAVEPOINT `<savepoint
    /// specifier>`
    Establish(String),
    /// [§17.6] `<release savepoint statement>` ::= RELEASE SAVEPOINT
    /// `<savepoint specifier>`
    Release(String),
    /// [§17.8] `<rollback statement>` ::= ROLLBACK \[WORK\] \[AND NO
    /// CHAIN\] `<savepoint clause>`, where `<savepoint clause>` ::= TO
    /// SAVEPOINT `<savepoint specifier>`
    RollbackTo(String),
}

impl SavepointStatement {
    /// The `<savepoint name>` it names.
    pub fn name(&self) -> &str {
        match self {
            SavepointStatement::Establish(name) | SavepointStatement::Release(name) | SavepointStatement::RollbackTo(name) => name,
        }
    }
}
