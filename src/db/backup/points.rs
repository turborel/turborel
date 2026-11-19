//! \[EXTENSION\] Restore points, and a target's history found again
//! (RULES.md D164, D166). A restore point is a record of the log, of no
//! transaction, holding its name and the time by the database's clock,
//! logged and synced. Recovery replays it as nothing, and a restore stops
//! after it. A target's manifests and segments are the truth of what it
//! holds, so a rescan records again what it finds there and the history
//! lacks, and marks gone what the history has and it does not.

use std::collections::BTreeSet;

use super::chain::{backup_ids, manifest_of};
use crate::backup::history::{BackupEntry, BackupState, Entry, SegmentEntry};
use crate::backup::timeline::{FIRST_TIMELINE, highest, segment_name, segments};
use crate::db::{Database, api};
use crate::error::Result;
use crate::log::Lsn;
use crate::log::record::{Change, Record};
use crate::txn::NO_TRANSACTION;

/// \[EXTENSION\] What a rescan recorded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rescanned {
    /// Backups found and recorded.
    pub backups: u64,
    /// Backups recorded and no longer there, marked gone.
    pub gone: u64,
    /// Segments found and recorded.
    pub segments: u64,
    /// Segments recorded and no longer there, marked removed.
    pub removed: u64,
}

impl Database {
    /// \[EXTENSION\] Makes restore point `name`: its record logged and
    /// synced, and the history noting it (RULES.md D164). Its position.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database has no log, or a restore point of the
    /// name was made before; -915 if the log cannot take it, or the record
    /// cannot be synced.
    pub fn create_restore_point(&self, name: &str) -> Result<Lsn> {
        let made = self.backup_history()?.iter().any(|e| point_named(e, name));
        if made {
            return Err(api(format!("[EXTENSION] RULES.md D164: a restore point {name} was made already")));
        }
        let st = self.snapshot()?;
        let log = st
            .log
            .clone()
            .ok_or_else(|| api("[EXTENSION] RULES.md D164: a restore point is a record of the log, and the database has none"))?;
        st.tx.taking_changes()?;
        let wall = st.clock.now();
        let at = log.append(&Record { tx: NO_TRANSACTION, change: Change::RestorePoint { name: name.to_owned(), wall } }.encode())?;
        st.tx.sync(&*log, at)?;
        self.record_history(Entry::Point { name: name.to_owned(), position: at, wall })?;
        Ok(at)
    }

    /// \[EXTENSION\] Records again what the target at `location` holds and
    /// the history lacks, and marks gone what it has and the target does
    /// not (RULES.md D166): see the module's documentation.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 (SQLSTATE TBB01) if a manifest is damaged; the
    /// target's; -915 if the history cannot be written.
    pub fn rescan_target(&self, location: &str) -> Result<Rescanned> {
        let target = self.open_target(location)?;
        let at = target.location();
        let (_, reading) = self.object_formats()?;
        let history = self.backup_history()?;
        let mut done = Rescanned::default();
        let held = backup_ids(&*target)?;
        let known: BTreeSet<String> = history.iter().filter_map(|e| completed_at(e, &at)).collect();
        for id in held.iter().filter(|id| !known.contains(*id)) {
            let m = manifest_of(&target, id, &reading)?;
            let entry = BackupEntry {
                id: m.id,
                location: at.clone(),
                kind: m.kind,
                parent: m.parent,
                timeline: m.timeline,
                from: m.from,
                stop: m.stop,
                bytes: m.bytes,
                state: BackupState::Completed,
                label: m.label,
                started: m.started,
                ended: m.ended,
            };
            self.record_history(Entry::Backup(entry))?;
            done.backups = done.backups.saturating_add(1);
        }
        for entry in history.iter().filter_map(|e| completed_entry(e, &at)).filter(|b| !held.contains(&b.id)) {
            self.record_history(Entry::Backup(BackupEntry { state: BackupState::Gone, ..entry.clone() }))?;
            done.gone = done.gone.saturating_add(1);
        }
        self.rescan_segments(&*target, &at, &history, &mut done)?;
        Ok(done)
    }

    /// The segments part of `rescan_target`.
    fn rescan_segments(
        &self,
        target: &dyn crate::backup::BackupTarget,
        location: &str,
        history: &[Entry],
        done: &mut Rescanned,
    ) -> Result<()> {
        let recorded = archived_names(history, location);
        let mut found = BTreeSet::new();
        let wall = self.snapshot()?.clock.now();
        for timeline in FIRST_TIMELINE..=highest(target)?.unwrap_or_default() {
            for s in segments(target, timeline)? {
                found.insert(s.name.clone());
                if !recorded.contains(&s.name) {
                    let entry = SegmentEntry { location: location.to_owned(), timeline, first: s.first, last: s.last, bytes: 0, wall };
                    self.record_history(Entry::Archived(entry))?;
                    done.segments = done.segments.saturating_add(1);
                }
            }
        }
        for entry in history.iter().filter_map(|e| archived_entry(e, location)) {
            let name = segment_name(entry.timeline, entry.first, entry.last);
            if crate::backup::decide::went((recorded.contains(&name), found.contains(&name))) {
                self.record_history(Entry::Removed(SegmentEntry { wall, ..entry.clone() }))?;
                done.removed = done.removed.saturating_add(1);
            }
        }
        Ok(())
    }
}

/// Whether `entry` records a restore point named `name`.
fn point_named(entry: &Entry, name: &str) -> bool {
    match entry {
        Entry::Point { name: made, .. } => made == name,
        Entry::Backup(_) | Entry::Archived(_) | Entry::Removed(_) | Entry::Restored { .. } => false,
    }
}

/// The id of the backup `entry` records completed to `location`, if it
/// does.
fn completed_at(entry: &Entry, location: &str) -> Option<String> {
    completed_entry(entry, location).map(|b| b.id.clone())
}

/// The backup `entry` records completed to `location`, if it does.
fn completed_entry<'e>(entry: &'e Entry, location: &str) -> Option<&'e BackupEntry> {
    match entry {
        Entry::Backup(b) => ((b.state, b.location.as_str()) == (BackupState::Completed, location)).then_some(b),
        Entry::Archived(_) | Entry::Removed(_) | Entry::Point { .. } | Entry::Restored { .. } => None,
    }
}

/// The segment `entry` records archived to `location`, if it does.
fn archived_entry<'e>(entry: &'e Entry, location: &str) -> Option<&'e SegmentEntry> {
    match entry {
        Entry::Archived(s) => (s.location == location).then_some(s),
        Entry::Backup(_) | Entry::Removed(_) | Entry::Point { .. } | Entry::Restored { .. } => None,
    }
}

/// The segments of `location` the history records archived and not
/// removed since.
pub(super) fn archived_names(history: &[Entry], location: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for entry in history {
        let (removed, s) = match entry {
            Entry::Archived(s) => (false, s),
            Entry::Removed(s) => (true, s),
            Entry::Backup(_) | Entry::Point { .. } | Entry::Restored { .. } => continue,
        };
        if s.location != location {
            continue;
        }
        let name = segment_name(s.timeline, s.first, s.last);
        if removed {
            names.remove(&name);
        } else {
            names.insert(name);
        }
    }
    names
}
