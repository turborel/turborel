//! \[DESIGN\] The kinds of log record (RULES.md D25, D57): the tag each is
//! written with, and `KINDS`, every one, which whatever must handle each
//! kind is checked against: recovery's replay, and the tests that a
//! statement writes each and that replaying it gives what the statement
//! did.

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
pub(super) const RECONFIGURATION: u8 = 19;
pub(super) const SEQUENCE: u8 = 20;
pub(super) const ROLE_CREATOR: u8 = 21;
pub(super) const LOB_PUT: u8 = 22;
pub(super) const LOB_STALE: u8 = 23;
/// [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20] A spatial catalog change.
pub(super) const SPATIAL: u8 = 24;
/// \[EXTENSION\] A restore point (RULES.md D164).
pub(super) const RESTORE_POINT: u8 = 25;

/// \[DESIGN\] The tag of every kind of record, in tag order (RULES.md
/// D57). A kind added gets its tag in `Change::kind`, whose match has no
/// wildcard, so it does not compile until it has one; and a test fails
/// until its tag is listed here, as `Record::decode` then reads a tag this
/// does not list.
pub const KINDS: [u8; 25] = [
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
    RECONFIGURATION,
    SEQUENCE,
    ROLE_CREATOR,
    LOB_PUT,
    LOB_STALE,
    SPATIAL,
    RESTORE_POINT,
];

/// \[DESIGN\] The kinds written only through a replicated log (RULES.md
/// D57): a database alone writes none, so the tests of the core's own
/// statements cover every kind but these, which the replication crate's
/// tests cover.
pub const REPLICATED: [u8; 1] = [RECONFIGURATION];

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
            Change::LobPut { .. } => LOB_PUT,
            Change::LobStale { .. } => LOB_STALE,
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
            Change::Reconfiguration { .. } => RECONFIGURATION,
            Change::Sequence { .. } => SEQUENCE,
            Change::RoleCreator { .. } => ROLE_CREATOR,
            Change::Spatial(_) => SPATIAL,
            Change::RestorePoint { .. } => RESTORE_POINT,
        }
    }
}
