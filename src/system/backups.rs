//! \[EXTENSION\] [ISO/IEC 9075:1992 §21.1] INFORMATION_SCHEMA.BACKUPS,
//! ARCHIVED_SEGMENTS and RESTORE_POINTS (RULES.md D166): objects
//! "associated with INFORMATION_SCHEMA" this implementation adds, made
//! from the database's durable backup history (`db/backup/history.rs`)
//! and the backups under way. BACKUPS shows each backup's latest state,
//! a backup the history records as running and no longer under way as
//! INTERRUPTED, as after a crash; ARCHIVED_SEGMENTS each segment archived
//! and not removed since; RESTORE_POINTS each restore point. The
//! database's owner sees them; a non-owner sees no rows.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::{Seen, SystemTable, text, text_column};
use crate::backup::history::{BackupEntry, BackupState, Entry, SegmentEntry};
use crate::catalog::ColumnDesc;
use crate::db::backup::{Backups as Kept, history_entries};
use crate::error::Result;
use crate::storage::Row;
use crate::sync::lock_builtin;
use crate::value::datetime::{Moment, utc_of_unix};
use crate::value::{DataType, Datetime, Decimal, MAX_EXACT_PRECISION, Spelled, Value};

/// \[DESIGN\] The longest location, id, label or name shown, cut there.
const TEXT_LENGTH: u32 = 1024;
/// The digits of the seconds' fraction of a time shown.
const PRECISION: u8 = 6;
/// The state of a backup the history records running, and no longer
/// under way.
const INTERRUPTED: &str = "INTERRUPTED";

/// A column of a count, not null unless `nullable`.
fn count_column(name: &str, nullable: bool) -> ColumnDesc {
    ColumnDesc {
        name: name.to_owned(),
        data_type: DataType::Exact { precision: MAX_EXACT_PRECISION, scale: 0, spelled: Spelled::Numeric },
        not_null: !nullable,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    }
}

/// A column of a time, not null unless `nullable`.
fn time_column(name: &str, nullable: bool) -> ColumnDesc {
    ColumnDesc { data_type: DataType::Timestamp { precision: u32::from(PRECISION), zoned: true }, ..count_column(name, nullable) }
}

/// A count's value.
fn count(n: impl Into<i128>) -> Value {
    Value::Exact(Decimal::new(n.into(), 0))
}

/// A time's value, `micros` from the epoch.
fn time(micros: i64) -> Value {
    Value::Datetime(Datetime { moment: Moment::Timestamp, utc: utc_of_unix(micros), zone: 0, zoned: true, precision: PRECISION })
}

/// The text `s`, cut at `TEXT_LENGTH` characters.
fn cut(s: &str) -> Value {
    text(&s.chars().take(usize::try_from(TEXT_LENGTH).unwrap_or(usize::MAX)).collect::<String>())
}

/// The entries of the history `kept` holds, if `seen` is the owner's;
/// none else.
fn seen_entries(seen: &Seen<'_>, kept: &Kept) -> Result<Vec<Entry>> {
    if seen.is_owner() { history_entries(seen.state, kept) } else { Ok(Vec::new()) }
}

/// \[EXTENSION\] The views of the backups `kept` holds.
pub(super) fn tables(kept: &Arc<Kept>) -> [Arc<dyn SystemTable>; 3] {
    [Arc::new(Backups(Arc::clone(kept))), Arc::new(ArchivedSegments(Arc::clone(kept))), Arc::new(RestorePoints(Arc::clone(kept)))]
}

/// \[EXTENSION\] INFORMATION_SCHEMA.BACKUPS.
pub(super) struct Backups(Arc<Kept>);

impl SystemTable for Backups {
    fn name(&self) -> &'static str {
        "BACKUPS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        vec![
            text_column(("BACKUP_ID", TEXT_LENGTH), true),
            text_column(("LOCATION", TEXT_LENGTH), true),
            text_column(("KIND", 11), true),
            text_column(("PARENT_ID", TEXT_LENGTH), false),
            count_column("TIMELINE", false),
            count_column("START_POSITION", false),
            count_column("STOP_POSITION", true),
            count_column("BYTES", false),
            text_column(("STATE", 11), true),
            text_column(("LABEL", TEXT_LENGTH), false),
            time_column("STARTED", false),
            time_column("ENDED", true),
        ]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let mut latest: BTreeMap<(String, String), BackupEntry> = BTreeMap::new();
        for entry in seen_entries(seen, &self.0)? {
            if let Some(b) = backup_of(entry) {
                latest.insert((b.id.clone(), b.location.clone()), b);
            }
        }
        let running = lock_builtin(&self.0.running);
        Ok(latest
            .into_values()
            .map(|b| {
                let progress = running.get(&b.id).filter(|_| b.state == BackupState::Running);
                let state = match (b.state, progress) {
                    (BackupState::Running, None) => INTERRUPTED,
                    (s, _) => s.name(),
                };
                let ended = b.state != BackupState::Running;
                vec![
                    cut(&b.id),
                    cut(&b.location),
                    text(b.kind.name()),
                    if b.parent.is_empty() { Value::Null } else { cut(&b.parent) },
                    count(b.timeline),
                    count(b.from.0),
                    if b.state == BackupState::Completed { count(b.stop.0) } else { Value::Null },
                    count(progress.map_or(b.bytes, |p| p.done)),
                    text(state),
                    if b.label.is_empty() { Value::Null } else { cut(&b.label) },
                    time(b.started),
                    if ended { time(b.ended) } else { Value::Null },
                ]
            })
            .collect())
    }
}

/// The backup `entry` records, if it records one.
fn backup_of(entry: Entry) -> Option<BackupEntry> {
    match entry {
        Entry::Backup(b) => Some(b),
        Entry::Archived(_) | Entry::Removed(_) | Entry::Point { .. } | Entry::Restored { .. } => None,
    }
}

/// \[EXTENSION\] INFORMATION_SCHEMA.ARCHIVED_SEGMENTS.
pub(super) struct ArchivedSegments(Arc<Kept>);

impl SystemTable for ArchivedSegments {
    fn name(&self) -> &'static str {
        "ARCHIVED_SEGMENTS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        vec![
            text_column(("LOCATION", TEXT_LENGTH), true),
            count_column("TIMELINE", false),
            count_column("FIRST_POSITION", false),
            count_column("LAST_POSITION", false),
            count_column("BYTES", false),
            time_column("ARCHIVED", false),
        ]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let mut kept: BTreeMap<(String, u32, u64), SegmentEntry> = BTreeMap::new();
        for entry in seen_entries(seen, &self.0)? {
            match entry {
                Entry::Archived(s) => {
                    kept.insert((s.location.clone(), s.timeline, s.first.0), s);
                }
                Entry::Removed(s) => {
                    kept.remove(&(s.location.clone(), s.timeline, s.first.0));
                }
                Entry::Backup(_) | Entry::Point { .. } | Entry::Restored { .. } => {}
            }
        }
        Ok(kept
            .into_values()
            .map(|s| vec![cut(&s.location), count(s.timeline), count(s.first.0), count(s.last.0), count(s.bytes), time(s.wall)])
            .collect())
    }
}

/// \[EXTENSION\] INFORMATION_SCHEMA.RESTORE_POINTS.
pub(super) struct RestorePoints(Arc<Kept>);

impl SystemTable for RestorePoints {
    fn name(&self) -> &'static str {
        "RESTORE_POINTS"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        vec![text_column(("POINT_NAME", TEXT_LENGTH), true), count_column("POSITION", false), time_column("CREATED", false)]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        Ok(seen_entries(seen, &self.0)?
            .into_iter()
            .filter_map(|e| match e {
                Entry::Point { name, position, wall } => Some(vec![cut(&name), count(position.0), time(wall)]),
                Entry::Backup(_) | Entry::Archived(_) | Entry::Removed(_) | Entry::Restored { .. } => None,
            })
            .collect())
    }
}
