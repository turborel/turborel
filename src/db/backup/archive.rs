//! \[EXTENSION\] The archiving of the log (RULES.md D163): the job
//! `turborel-archive`, run as JOBS.POLICY says, reads the log's records
//! after the last archived, by `Log::read_from`, so whatever the `Log`,
//! and archives them a segment an object under the database's timeline
//! (`backup/timeline.rs`): once ARCHIVE.SEGMENT_BYTES are gathered, or
//! once a record has waited ARCHIVE.TIMEOUT_MS. Only durable records are
//! archived, the log synced through the last before its segment is
//! written. While ARCHIVE.MODE is ON a checkpoint truncates the log only
//! before the last record archived (`Floor`), and before nothing until the
//! archiver has found how far the archive goes. A segment older than
//! ARCHIVE.RETENTION_SECONDS, whose records the newest backup does not
//! need, is removed.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::backup::history::{BackupState, Entry, SegmentEntry};
use crate::backup::object::{ObjectOut, Writing};
use crate::backup::timeline::{segment_name, segments};
use crate::backup::{BackupTarget, POSITION_BYTES};
use crate::bytes::Put;
use crate::db::{Database, api};
use crate::error::Result;
use crate::log::{Log, Lsn};
use crate::settings::{
    ARCHIVE_MODE, ARCHIVE_RETENTION_SECONDS, ARCHIVE_SEGMENT_BYTES, ARCHIVE_TARGET, ARCHIVE_TIMELINE, ARCHIVE_TIMEOUT_MS,
};
use crate::sync::lock;

/// \[EXTENSION\] The least wait between the archiver's rounds while
/// ARCHIVE.MODE is ON: a tenth of a second, so ARCHIVE.TIMEOUT_MS 0 does
/// not spin.
const LEAST_WAIT: Duration = Duration::from_millis(100);

/// \[EXTENSION\] Microseconds in a second, for ARCHIVE.RETENTION_SECONDS.
const MICROS: i64 = 1_000_000;

/// Where the archiver stands: the archive it writes to, and how far.
#[derive(Default)]
pub(super) struct Archiver {
    location: String,
    timeline: u32,
    target: Option<Arc<dyn BackupTarget>>,
    /// Whether the archive was listed, so `archived` is known.
    listed: bool,
    /// The last record archived, if any.
    archived: Option<Lsn>,
    /// Since when records have waited to be archived.
    waiting: Option<Instant>,
}

/// What the archiver's settings say.
struct Asked {
    location: String,
    timeline: u32,
    segment: u64,
    timeout: Duration,
    force: bool,
}

/// The archive job's work: a round, then another as ARCHIVE.TIMEOUT_MS
/// says, while ARCHIVE.MODE is ON.
pub(in crate::db) fn archived(db: &Database) -> Result<Option<Duration>> {
    let Some(asked) = db.archive_asked(false)? else { return Ok(None) };
    let timeout = asked.timeout;
    db.archive_round(&asked)?;
    Ok(Some(timeout.max(LEAST_WAIT)))
}

impl Database {
    /// What the settings ask of the archiver, if it is to run: ARCHIVE.MODE
    /// ON, the database kept in files and serving. The floor freed if not.
    fn archive_asked(&self, force: bool) -> Result<Option<Asked>> {
        let st = self.snapshot()?;
        let Some(files) = self.files()? else { return Ok(None) };
        if !crate::backup::decide::archiving((st.settings.is_on(ARCHIVE_MODE), st.tx.held().is_some())) {
            files.archived.free();
            return Ok(None);
        }
        files.archived.hold_all();
        let location = st.settings.word(ARCHIVE_TARGET).to_owned();
        if location.is_empty() {
            return Err(api("[EXTENSION] RULES.md D163: ARCHIVE.MODE is ON, and ARCHIVE.TARGET names no target"));
        }
        let timeline = u32::try_from(st.settings.integer(ARCHIVE_TIMELINE)).unwrap_or(1);
        let segment = u64::try_from(st.settings.integer(ARCHIVE_SEGMENT_BYTES)).unwrap_or(u64::MAX);
        let timeout = Duration::from_millis(u64::try_from(st.settings.integer(ARCHIVE_TIMEOUT_MS)).unwrap_or(u64::MAX));
        Ok(Some(Asked { location, timeline, segment, timeout, force }))
    }

    /// \[EXTENSION\] Archives now every durable record not yet archived,
    /// whatever ARCHIVE.SEGMENT_BYTES and ARCHIVE.TIMEOUT_MS say (RULES.md
    /// D163): the last record archived, if any.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if ARCHIVE.MODE is OFF or ARCHIVE.TARGET empty, or the
    /// archive holds records of this timeline past the log's end, as of
    /// another database; the log's; the target's.
    pub fn archive_now(&self) -> Result<Option<Lsn>> {
        let asked =
            self.archive_asked(true)?.ok_or_else(|| api("[EXTENSION] RULES.md D163: the log is archived only while ARCHIVE.MODE is ON"))?;
        self.archive_round(&asked)
    }

    /// The archive's target and the last record archived to it, found by
    /// listing it if the settings changed or it was never listed.
    fn archive_place(&self, asked: &Asked, log: &dyn Log) -> Result<(Arc<dyn BackupTarget>, Option<Lsn>)> {
        let known = lock(&self.backups().archiver)?;
        let moved = (&known.location, known.timeline) != (&asked.location, asked.timeline);
        if let (false, true, Some(target)) = (moved, known.listed, known.target.clone()) {
            return Ok((target, known.archived));
        }
        drop(known);
        let target = self.open_target(&asked.location)?;
        let archived = segments(&*target, asked.timeline)?.last().map(|s| s.last);
        if archived.is_some_and(|last| last >= log.end()) {
            return Err(api(format!(
                "[EXTENSION] RULES.md D165: the archive at {} holds records of timeline {} past this database's log, as of another database: archiving waits",
                asked.location, asked.timeline
            )));
        }
        let mut state = lock(&self.backups().archiver)?;
        *state = Archiver {
            location: asked.location.clone(),
            timeline: asked.timeline,
            target: Some(Arc::clone(&target)),
            listed: true,
            archived,
            waiting: None,
        };
        Ok((target, archived))
    }

    /// A round of the archiver: see the module's documentation.
    fn archive_round(&self, asked: &Asked) -> Result<Option<Lsn>> {
        let log = self.snapshot()?.log.clone().ok_or_else(|| api("[EXTENSION] a database kept in files has a log"))?;
        let (target, archived) = self.archive_place(asked, &*log)?;
        if let Some(files) = self.files()? {
            files.archived.hold_from(archived.unwrap_or_default());
        }
        let end = log.end();
        let mut gathered: Vec<(Lsn, Vec<u8>)> = Vec::new();
        let mut bytes = 0_u64;
        let mut last = archived;
        for read in records_after(&*log, archived) {
            let (at, record) = read?;
            if at >= end {
                break;
            }
            bytes = bytes.saturating_add(u64::try_from(record.len()).unwrap_or(u64::MAX));
            gathered.push((at, record));
            if bytes >= asked.segment {
                last = Some(self.archive_segment(&*target, asked, &*log, std::mem::take(&mut gathered))?);
                bytes = 0;
            }
        }
        if gathered.is_empty() {
            lock(&self.backups().archiver)?.waiting = None;
        } else if self.waited(asked)? {
            last = Some(self.archive_segment(&*target, asked, &*log, gathered)?);
        }
        self.retain(&*target, asked)?;
        Ok(last)
    }

    /// Whether records gathered have waited long enough to be archived
    /// short of a segment's bytes: asked to, or ARCHIVE.TIMEOUT_MS since
    /// they were first found waiting.
    fn waited(&self, asked: &Asked) -> Result<bool> {
        let mut state = lock(&self.backups().archiver)?;
        let since = *state.waiting.get_or_insert_with(Instant::now);
        Ok(crate::backup::decide::waited_enough(asked.force, since.elapsed(), asked.timeout))
    }

    /// Archives `records`, synced first, as a segment: its last record.
    fn archive_segment(&self, target: &dyn BackupTarget, asked: &Asked, log: &dyn Log, records: Vec<(Lsn, Vec<u8>)>) -> Result<Lsn> {
        let (Some(&(first, _)), Some(&(last, _))) = (records.first(), records.last()) else {
            return Err(api("[EXTENSION] a segment holds a record"));
        };
        log.sync(last)?;
        let (writing, _): (Writing, _) = self.backup_formats()?;
        let mut object = ObjectOut::begin(target, &segment_name(asked.timeline, first, last), &writing)?;
        for (at, record) in records {
            let mut payload = Vec::with_capacity(record.len().saturating_add(POSITION_BYTES));
            payload.put_u64(at.0);
            payload.extend_from_slice(&record);
            object.block(&payload)?;
        }
        let bytes = object.finish()?;
        {
            let mut state = lock(&self.backups().archiver)?;
            state.archived = Some(last);
            state.waiting = None;
        }
        if let Some(files) = self.files()? {
            files.archived.hold_from(last);
        }
        let wall = self.snapshot()?.clock.now();
        self.record_history(Entry::Archived(SegmentEntry {
            location: asked.location.clone(),
            timeline: asked.timeline,
            first,
            last,
            bytes,
            wall,
        }))?;
        Ok(last)
    }

    /// Removes the segments of `target` past ARCHIVE.RETENTION_SECONDS
    /// whose records the newest backup completed does not need.
    fn retain(&self, target: &dyn BackupTarget, asked: &Asked) -> Result<()> {
        let st = self.snapshot()?;
        let seconds = i64::try_from(st.settings.integer(ARCHIVE_RETENTION_SECONDS)).unwrap_or(i64::MAX);
        if seconds == 0 {
            return Ok(());
        }
        let cutoff = st.clock.now().saturating_sub(seconds.saturating_mul(MICROS));
        let history = self.backup_history()?;
        let needed = history.iter().filter_map(newest_from).max().unwrap_or_default();
        for segment in segments(target, asked.timeline)? {
            let archived_at = history.iter().find_map(|e| archived_when(e, &asked.location, &segment.name));
            if crate::backup::decide::segment_expired(archived_at, cutoff, (segment.last, needed)) {
                target.remove(&segment.name)?;
                let removed = SegmentEntry {
                    location: asked.location.clone(),
                    timeline: segment.timeline,
                    first: segment.first,
                    last: segment.last,
                    bytes: 0,
                    wall: st.clock.now(),
                };
                self.record_history(Entry::Removed(removed))?;
            }
        }
        Ok(())
    }

    /// Takes the archiver's settings as they now are: the floor held or
    /// freed, and the job asked for while ARCHIVE.MODE is ON.
    pub(in crate::db) fn archive_settings_applied(&self) -> Result<()> {
        let st = self.snapshot()?;
        let Some(files) = self.files()? else { return Ok(()) };
        if crate::backup::decide::archiving((st.settings.is_on(ARCHIVE_MODE), st.tx.held().is_some())) {
            files.archived.hold_all();
            self.request_archive(Duration::ZERO);
        } else {
            files.archived.free();
        }
        Ok(())
    }
}

/// The position recovery of a completed backup in `entry` reads from.
fn newest_from(entry: &Entry) -> Option<Lsn> {
    match entry {
        Entry::Backup(b) => (b.state == BackupState::Completed).then_some(b.from),
        Entry::Archived(_) | Entry::Removed(_) | Entry::Point { .. } | Entry::Restored { .. } => None,
    }
}

/// When the segment `name` of the archive at `location` was archived, if
/// `entry` records it.
fn archived_when(entry: &Entry, location: &str, name: &str) -> Option<i64> {
    match entry {
        Entry::Archived(s) => {
            ((s.location.as_str(), segment_name(s.timeline, s.first, s.last).as_str()) == (location, name)).then_some(s.wall)
        }
        Entry::Backup(_) | Entry::Removed(_) | Entry::Point { .. } | Entry::Restored { .. } => None,
    }
}

/// The records of `log` after `archived`: read from it, which the floor
/// keeps, or from the start if it was truncated past it, each before or
/// at it passed over.
fn records_after(log: &dyn Log, archived: Option<Lsn>) -> Box<dyn Iterator<Item = Result<(Lsn, Vec<u8>)>> + '_> {
    let Some(from) = archived else { return log.read_from(Lsn(0)) };
    let mut at_last = log.read_from(from).peekable();
    let found = at_last.peek().and_then(|r| r.as_ref().ok()).is_some_and(|(at, _)| *at == from);
    let records: Box<dyn Iterator<Item = Result<(Lsn, Vec<u8>)>> + '_> = if found { Box::new(at_last) } else { log.read_from(Lsn(0)) };
    Box::new(records.filter(move |r| r.as_ref().map_or(true, |(at, _)| *at > from)))
}
