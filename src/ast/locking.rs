//! \[EXTENSION\] The locks a statement asks for by name: a query's row
//! locks (`FOR UPDATE`, `FOR SHARE`, RULES.md D185) and LOCK TABLE
//! (RULES.md D186).

use super::TableName;

/// \[EXTENSION\] `<locking clause>` ::= FOR {UPDATE | SHARE} \[OF `<table
/// or query name>` \[{, `<table or query name>`}...\]\] \[NOWAIT | SKIP
/// LOCKED\], of a query run directly or prepared (RULES.md D185).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockingClause {
    /// FOR UPDATE, else FOR SHARE.
    pub exclusive: bool,
    /// The tables whose rows it locks, each named as the query's FROM
    /// clause names it, and where: none, each table it reads.
    pub of: Option<Vec<(String, usize)>>,
    /// What it does of a row another transaction holds.
    pub waits: LockWaits,
    /// Where FOR is written.
    pub at: usize,
}

/// \[EXTENSION\] What a lock does when another transaction holds what it
/// asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockWaits {
    /// It waits, as the session's LOCKS.WAIT says.
    Wait,
    /// NOWAIT: it is refused at once, lock not available.
    NoWait,
    /// SKIP LOCKED: the row is passed over.
    SkipLocked,
}

/// \[EXTENSION\] `<lock table statement>` ::= LOCK \[TABLE\] `<target
/// table>` \[{, `<target table>`}...\] \[IN `<lock table mode>` MODE\]
/// \[NOWAIT\] (RULES.md D186).
#[derive(Debug, Clone)]
pub struct LockTable {
    /// Each table locked, with whether ONLY was written, and where it is
    /// named.
    pub tables: Vec<(TableName, bool, usize)>,
    /// The mode, ACCESS EXCLUSIVE if none is written.
    pub mode: LockTableMode,
    /// NOWAIT: refused at once if a table is locked against it.
    pub nowait: bool,
}

/// \[EXTENSION\] `<lock table mode>`, each held to the end of the
/// transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockTableMode {
    /// ACCESS SHARE.
    AccessShare,
    /// ROW SHARE.
    RowShare,
    /// ROW EXCLUSIVE.
    RowExclusive,
    /// SHARE UPDATE EXCLUSIVE.
    ShareUpdateExclusive,
    /// SHARE.
    Share,
    /// SHARE ROW EXCLUSIVE.
    ShareRowExclusive,
    /// EXCLUSIVE.
    Exclusive,
    /// ACCESS EXCLUSIVE.
    AccessExclusive,
}

impl LockTableMode {
    /// Each mode, with the words that name it.
    pub const NAMED: [(LockTableMode, &'static [&'static str]); 8] = [
        (LockTableMode::AccessShare, &["ACCESS", "SHARE"]),
        (LockTableMode::RowShare, &["ROW", "SHARE"]),
        (LockTableMode::RowExclusive, &["ROW", "EXCLUSIVE"]),
        (LockTableMode::ShareUpdateExclusive, &["SHARE", "UPDATE", "EXCLUSIVE"]),
        (LockTableMode::ShareRowExclusive, &["SHARE", "ROW", "EXCLUSIVE"]),
        (LockTableMode::Share, &["SHARE"]),
        (LockTableMode::Exclusive, &["EXCLUSIVE"]),
        (LockTableMode::AccessExclusive, &["ACCESS", "EXCLUSIVE"]),
    ];
}
