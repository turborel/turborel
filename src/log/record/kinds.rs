//! \[DESIGN\] The kinds of log record (RULES.md D25, D57): the tag each is
//! written with, and `KINDS`, every one, which whatever must handle each
//! kind is checked against: recovery's replay, and the tests that a
//! statement writes each and that replaying it gives what the statement
//! did. Split from record.rs (RULES.md R-SIZE-2).

use super::Change;

/// The tags of the kinds of record.
pub(super) const INSERT: u8 = 1;
pub(super) const UPDATE: u8 = 2;
pub(super) const DELETE: u8 = 3;
pub(super) const COMMIT: u8 = 4;
pub(super) const ABORT: u8 = 5;
pub(super) const SCHEMA: u8 = 6;
pub(super) const CREATE_INDEX: u8 = 7;
pub(super) const STATEMENT_UNDONE: u8 = 8;
pub(super) const CHECKPOINT: u8 = 9;
pub(super) const DROP_INDEX: u8 = 10;
pub(super) const DROP: u8 = 11;
pub(super) const ALTER_TABLE: u8 = 12;
pub(super) const SETTING: u8 = 13;
pub(super) const ALTER_INDEX: u8 = 14;
pub(super) const STALE: u8 = 15;
pub(super) const USER: u8 = 16;
pub(super) const OWNER: u8 = 17;
pub(super) const DEFINED: u8 = 18;

/// \[DESIGN\] The tag of every kind of record, in tag order (RULES.md
/// D57). A kind added gets its tag in `Change::kind`, whose match has no
/// wildcard, so it does not compile until it has one; and a test fails
/// until its tag is listed here, as `Record::decode` then reads a tag this
/// does not list.
pub const KINDS: [u8; 18] = [
    INSERT,
    UPDATE,
    DELETE,
    COMMIT,
    ABORT,
    SCHEMA,
    CREATE_INDEX,
    STATEMENT_UNDONE,
    CHECKPOINT,
    DROP_INDEX,
    DROP,
    ALTER_TABLE,
    SETTING,
    ALTER_INDEX,
    STALE,
    USER,
    OWNER,
    DEFINED,
];

impl Change {
    /// \[DESIGN\] The tag of its kind, as its record is written: one of
    /// `KINDS`.
    pub fn kind(&self) -> u8 {
        match self {
            Change::Insert { .. } => INSERT,
            Change::Update { .. } => UPDATE,
            Change::Delete { .. } => DELETE,
            Change::Commit { .. } => COMMIT,
            Change::Abort => ABORT,
            Change::Checkpoint => CHECKPOINT,
            Change::StatementUndone { .. } => STATEMENT_UNDONE,
            Change::Stale { .. } => STALE,
            Change::Schema { .. } => SCHEMA,
            Change::Defined { .. } => DEFINED,
            Change::CreateIndex { .. } => CREATE_INDEX,
            Change::DropIndex { .. } => DROP_INDEX,
            Change::AlterIndex { .. } => ALTER_INDEX,
            Change::Drop { .. } => DROP,
            Change::AlterTable { .. } => ALTER_TABLE,
            Change::Setting { .. } => SETTING,
            Change::User { .. } => USER,
            Change::Owner { .. } => OWNER,
        }
    }
}
