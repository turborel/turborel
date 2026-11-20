//! \[EXTENSION\] The entries of a database's backup history (RULES.md
//! D166), format version 1: what its backups, archive, restore points and
//! restores did, an entry a block of its history file
//! (`db/backup/history.rs`), so INFORMATION_SCHEMA shows them after a
//! restart.
//!
//! ```text
//! entry:  format u8 (HISTORY_FORMAT) | kind u8 | body
//! backup (1):    id | location | kind u8 | parent | timeline u32 |
//!                from u64 | stop u64 | bytes u64 | state u8 | label |
//!                started i128 | ended i128
//! archived (2):  segment
//! removed (3):   segment
//! point (4):     name | position u64 | wall i128
//! restored (5):  location | id | timeline u32 | branch u64 | wall i128
//! segment:       location | timeline u32 | first u64 | last u64 |
//!                bytes u64 | wall i128
//! ```

use super::BackupKind;
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::log::Lsn;

/// \[EXTENSION\] The format of the history's entries this code writes and
/// reads.
pub const HISTORY_FORMAT: u8 = 1;

/// The kinds' tags.
const BACKUP: u8 = 1;
const ARCHIVED: u8 = 2;
const REMOVED: u8 = 3;
const POINT: u8 = 4;
const RESTORED: u8 = 5;

/// \[EXTENSION\] Where a backup stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupState {
    /// Being taken, or interrupted by a crash if the database was opened
    /// since.
    Running,
    /// Finished, its manifest written.
    Completed,
    /// Failed, leaving nothing.
    Failed,
    /// Cancelled, leaving nothing.
    Cancelled,
    /// No longer in its target, as a rescan found.
    Gone,
}

/// The states, by their codes, 1 up.
const STATES: [BackupState; 5] =
    [BackupState::Running, BackupState::Completed, BackupState::Failed, BackupState::Cancelled, BackupState::Gone];

impl BackupState {
    /// Its name, as INFORMATION_SCHEMA.BACKUPS shows it.
    pub fn name(self) -> &'static str {
        match self {
            BackupState::Running => "RUNNING",
            BackupState::Completed => "COMPLETED",
            BackupState::Failed => "FAILED",
            BackupState::Cancelled => "CANCELLED",
            BackupState::Gone => "GONE",
        }
    }

    fn code(self) -> u8 {
        let at = STATES.iter().position(|s| *s == self).unwrap_or_default();
        u8::try_from(at).unwrap_or_default().saturating_add(1)
    }

    fn of(code: u8) -> Option<BackupState> {
        STATES.get(usize::from(code.checked_sub(1)?)).copied()
    }
}

/// \[EXTENSION\] A backup, as the history records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupEntry {
    /// Its id.
    pub id: String,
    /// Its target's location.
    pub location: String,
    /// Full or incremental.
    pub kind: BackupKind,
    /// The id of the backup it follows, empty if full.
    pub parent: String,
    /// The timeline it was taken on.
    pub timeline: u32,
    /// The first record it holds.
    pub from: Lsn,
    /// The position after its last record, 0 until it is finished.
    pub stop: Lsn,
    /// The bytes it wrote.
    pub bytes: u64,
    /// Where it stands.
    pub state: BackupState,
    /// Its label.
    pub label: String,
    /// When it began.
    pub started: i64,
    /// When it ended, 0 until it does.
    pub ended: i64,
}

/// \[EXTENSION\] A segment of the archive, as the history records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentEntry {
    /// The archive's location.
    pub location: String,
    /// Its timeline.
    pub timeline: u32,
    /// Its first record's position.
    pub first: Lsn,
    /// Its last record's position.
    pub last: Lsn,
    /// Its bytes.
    pub bytes: u64,
    /// When it was archived or removed.
    pub wall: i64,
}

/// \[EXTENSION\] An entry of the history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A backup begun, or its state changed.
    Backup(BackupEntry),
    /// A segment archived.
    Archived(SegmentEntry),
    /// A segment removed, past the retention.
    Removed(SegmentEntry),
    /// A restore point made.
    Point {
        /// Its name.
        name: String,
        /// Its record's position.
        position: Lsn,
        /// When it was made.
        wall: i64,
    },
    /// The database restored from a backup.
    Restored {
        /// The backup's target's location.
        location: String,
        /// The backup's id.
        id: String,
        /// The timeline it began.
        timeline: u32,
        /// The last record replayed of the timeline before.
        branch: Lsn,
        /// When it was restored.
        wall: i64,
    },
}

/// An instant as an entry writes it.
fn put_wall(out: &mut Vec<u8>, wall: i64) {
    out.put_i128(i128::from(wall));
}

/// An instant read from `r`.
fn wall(r: &mut Reader<'_>) -> Result<i64> {
    i64::try_from(r.i128()?).map_err(|_| r.corrupt("an instant is out of range"))
}

fn put_segment(out: &mut Vec<u8>, s: &SegmentEntry) {
    out.put_str(&s.location);
    out.put_u32(s.timeline);
    out.put_u64(s.first.0);
    out.put_u64(s.last.0);
    out.put_u64(s.bytes);
    put_wall(out, s.wall);
}

fn segment(r: &mut Reader<'_>) -> Result<SegmentEntry> {
    let (location, timeline, first, last, bytes) = (r.str()?, r.u32()?, Lsn(r.u64()?), Lsn(r.u64()?), r.u64()?);
    Ok(SegmentEntry { location, timeline, first, last, bytes, wall: wall(r)? })
}

fn put_backup(out: &mut Vec<u8>, b: &BackupEntry) {
    out.put_str(&b.id);
    out.put_str(&b.location);
    out.put_u8(b.kind.code());
    out.put_str(&b.parent);
    out.put_u32(b.timeline);
    out.put_u64(b.from.0);
    out.put_u64(b.stop.0);
    out.put_u64(b.bytes);
    out.put_u8(b.state.code());
    out.put_str(&b.label);
    put_wall(out, b.started);
    put_wall(out, b.ended);
}

fn backup(r: &mut Reader<'_>) -> Result<BackupEntry> {
    let (id, location) = (r.str()?, r.str()?);
    let kind = BackupKind::of(r.u8()?).ok_or_else(|| r.corrupt("a backup's kind is unknown"))?;
    let (parent, timeline, from, stop, bytes) = (r.str()?, r.u32()?, Lsn(r.u64()?), Lsn(r.u64()?), r.u64()?);
    let state = BackupState::of(r.u8()?).ok_or_else(|| r.corrupt("a backup's state is unknown"))?;
    let label = r.str()?;
    let (started, ended) = (wall(r)?, wall(r)?);
    Ok(BackupEntry { id, location, kind, parent, timeline, from, stop, bytes, state, label, started, ended })
}

impl Entry {
    /// It encoded.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![HISTORY_FORMAT];
        match self {
            Entry::Backup(b) => {
                out.put_u8(BACKUP);
                put_backup(&mut out, b);
            }
            Entry::Archived(s) => {
                out.put_u8(ARCHIVED);
                put_segment(&mut out, s);
            }
            Entry::Removed(s) => {
                out.put_u8(REMOVED);
                put_segment(&mut out, s);
            }
            Entry::Point { name, position, wall } => {
                out.put_u8(POINT);
                out.put_str(name);
                out.put_u64(position.0);
                put_wall(&mut out, *wall);
            }
            Entry::Restored { location, id, timeline, branch, wall } => {
                out.put_u8(RESTORED);
                out.put_str(location);
                out.put_str(id);
                out.put_u32(*timeline);
                out.put_u64(branch.0);
                put_wall(&mut out, *wall);
            }
        }
        out
    }

    /// The entry `bytes` encode.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are malformed, or of another format.
    pub fn decode(bytes: &[u8]) -> Result<Entry> {
        let mut r = Reader::new(bytes, "an entry of the backup history");
        if r.u8()? != HISTORY_FORMAT {
            return Err(SqlError::new(Code::Corrupt, "[EXTENSION] RULES.md D166: an entry of the backup history of an unknown format"));
        }
        let entry = match r.u8()? {
            BACKUP => Entry::Backup(backup(&mut r)?),
            ARCHIVED => Entry::Archived(segment(&mut r)?),
            REMOVED => Entry::Removed(segment(&mut r)?),
            POINT => Entry::Point { name: r.str()?, position: Lsn(r.u64()?), wall: wall(&mut r)? },
            RESTORED => {
                Entry::Restored { location: r.str()?, id: r.str()?, timeline: r.u32()?, branch: Lsn(r.u64()?), wall: wall(&mut r)? }
            }
            _ => return Err(r.corrupt("its kind is unknown")),
        };
        r.finish()?;
        Ok(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An entry of each kind.
    fn entries() -> [Entry; 5] {
        let segment = SegmentEntry { location: "/a".into(), timeline: 1, first: Lsn(1), last: Lsn(4), bytes: 9, wall: 3 };
        let backup = BackupEntry {
            id: "B".into(),
            location: "/b".into(),
            kind: BackupKind::Full,
            parent: String::new(),
            timeline: 1,
            from: Lsn(1),
            stop: Lsn(8),
            bytes: 10,
            state: BackupState::Completed,
            label: "l".into(),
            started: 1,
            ended: 2,
        };
        [
            Entry::Backup(backup),
            Entry::Archived(segment.clone()),
            Entry::Removed(segment),
            Entry::Point { name: "P".into(), position: Lsn(3), wall: -4 },
            Entry::Restored { location: "/b".into(), id: "B".into(), timeline: 2, branch: Lsn(7), wall: 5 },
        ]
    }

    /// Verifies: [EXTENSION] RULES.md D166, the history's entries, format
    /// version 1: each kind round trips, and an unknown kind is refused.
    #[test]
    fn entries_round_trip() {
        let entries = entries();
        for entry in entries {
            assert_eq!(Entry::decode(&entry.encode()).unwrap(), entry);
        }
        assert_eq!(Entry::decode(&[HISTORY_FORMAT, 9]).err().map(|e| e.code), Some(Code::Corrupt));
        assert_eq!(BackupState::of(0), None);
        assert_eq!(BackupState::of(BackupState::Gone.code()), Some(BackupState::Gone));
    }

    /// Verifies: [EXTENSION] RULES.md D166, the history's entries: each
    /// kind cut at any byte, or followed by one, of another format, of a
    /// backup state or kind no code names, or of an instant out of range
    /// is refused with SQLCODE -916; each state has its name.
    #[test]
    fn malformed_entries_are_refused() {
        let corrupt = |bytes: &[u8]| Entry::decode(bytes).err().map(|e| e.code);
        for entry in entries() {
            let bytes = entry.encode();
            for cut in 0..bytes.len() {
                assert_eq!(corrupt(&bytes[..cut]), Some(Code::Corrupt), "{entry:?} cut at {cut}");
            }
            assert_eq!(corrupt(&[bytes.as_slice(), &[0]].concat()), Some(Code::Corrupt));
            let mut format = bytes.clone();
            format[0] = HISTORY_FORMAT.wrapping_add(1);
            assert_eq!(corrupt(&format), Some(Code::Corrupt));
        }
        let [backup, _, _, point, _] = entries();
        let bytes = backup.encode();
        // The format, the kind, then the id "B", the location "/b" and the
        // backup's kind: strings are a u32 length and their bytes.
        let kind_at = 2 + 4 + 1 + 4 + 2;
        assert_eq!(bytes[kind_at], BackupKind::Full.code());
        let mut kind = bytes.clone();
        kind[kind_at] = 0xEE;
        assert_eq!(corrupt(&kind), Some(Code::Corrupt), "a backup's kind");
        // Then the parent "", the timeline, the two positions and the bytes.
        let state_at = kind_at + 1 + 4 + 4 + 8 + 8 + 8;
        assert_eq!(bytes[state_at], BackupState::Completed.code());
        let mut state = bytes;
        state[state_at] = 0xEE;
        assert_eq!(corrupt(&state), Some(Code::Corrupt), "a backup's state");
        let mut far = point.encode();
        let wall_at = far.len() - 16;
        far[wall_at..].copy_from_slice(&i128::MAX.to_le_bytes());
        assert_eq!(corrupt(&far), Some(Code::Corrupt), "an instant out of range");
        let names: Vec<&str> = STATES.iter().map(|s| s.name()).collect();
        assert_eq!(names, ["RUNNING", "COMPLETED", "FAILED", "CANCELLED", "GONE"]);
    }
}
