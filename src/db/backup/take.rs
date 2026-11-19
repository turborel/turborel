//! \[EXTENSION\] An online backup (RULES.md D160, D162). A checkpoint is
//! taken, then an export (`db/export.rs`), which holds the checkpoints'
//! turn shared, so no checkpoint changes the data file or truncates the
//! log while the backup reads them; readers and writers go on, writing to
//! the log and the side files. Each durable file is copied a chunk at a
//! time, every chunk of a full backup, of an incremental one the chunks
//! whose digest differs from its parent's; then the checkpoint's image;
//! then the log's records from the first recovery from it reads to the
//! log's end, synced first. The manifest is written last. Restored, the
//! backup is recovered from its checkpoint with those records, as after a
//! crash at its stop position, so it holds every transaction committed
//! before it and no other. Backups run at once, each holding the turn
//! shared; a checkpoint waits for the last.

use std::sync::Arc;

use super::Progress;
use crate::backup::history::{BackupEntry, BackupState, Entry};
use crate::backup::manifest::{Chunk, Manifest, ManifestFile, chunk_digest};
use crate::backup::object::{ObjectOut, Writing, write_one};
use crate::backup::{BackupKind, BackupTarget, CHUNK_BYTES, FILES, IMAGE, LOG, MANIFEST, POSITION_BYTES, backup_prefix};
use crate::bytes::Put;
use crate::db::Database;
use crate::db::export::{ExportedFile, RECORDS_FILE, StateSource};
use crate::error::{Code, Result};
use crate::files::FileOwner;
use crate::log::Lsn;
use crate::settings::ARCHIVE_TIMELINE;
use crate::sync::lock_builtin;
use crate::txn::Stop;

/// \[EXTENSION\] What a backup is asked to be.
#[derive(Debug, Clone, Default)]
pub struct BackupOptions {
    /// Its target's location.
    pub location: String,
    /// The backup it follows, if incremental: its id, or empty for the
    /// newest in the target.
    pub incremental: Option<String>,
    /// Its label.
    pub label: String,
}

/// \[EXTENSION\] What a backup did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupReport {
    /// Its id.
    pub id: String,
    /// Full or incremental.
    pub kind: BackupKind,
    /// The position after its last record, before which it holds every
    /// transaction committed.
    pub stop: Lsn,
    /// The bytes of its objects.
    pub bytes: u64,
    /// The chunks of its files.
    pub chunks: u64,
    /// Of those, the ones it holds, the rest its parents'.
    pub written: u64,
}

/// A backup being copied: where to, in what formats, what it follows,
/// how it is cancelled, and what it wrote.
struct Copying<'a> {
    db: &'a Database,
    target: &'a dyn BackupTarget,
    writing: &'a Writing,
    prefix: String,
    id: String,
    stop: Option<&'a Stop>,
    bytes: u64,
    chunks: u64,
    written: u64,
}

/// A file's name within a database's directory, its parts by `/`.
fn within(file: &ExportedFile) -> String {
    file.within.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

/// Whose a file is, as a manifest names it.
fn owner(file: &ExportedFile) -> String {
    match &file.owner {
        FileOwner::Database => "DATABASE".to_owned(),
        FileOwner::Log => "LOG".to_owned(),
        FileOwner::Spill => "SPILL".to_owned(),
        FileOwner::Plugin(name) => name.clone(),
        FileOwner::Server => "SERVER".to_owned(),
        FileOwner::Unknown => "UNKNOWN".to_owned(),
    }
}

impl Copying<'_> {
    /// Fails if the backup was cancelled.
    fn check(&self) -> Result<()> {
        self.stop.map_or(Ok(()), Stop::check)
    }

    /// Copies file `index` of `source`, `file`, its chunks whose digests
    /// differ from `parent`'s.
    fn file(
        &mut self,
        source: &mut dyn StateSource,
        (index, file): (usize, &ExportedFile),
        parent: Option<&ManifestFile>,
    ) -> Result<ManifestFile> {
        let name = within(file);
        let mut object = ObjectOut::begin(self.target, &format!("{}{FILES}{name}", self.prefix), self.writing)?;
        let mut chunks = Vec::new();
        let mut at = 0_u64;
        let mut buf = vec![0; CHUNK_BYTES];
        while at < file.length {
            self.check()?;
            let len = usize::try_from(file.length.saturating_sub(at)).unwrap_or(usize::MAX).min(CHUNK_BYTES);
            let piece = buf.get_mut(..len).unwrap_or_default();
            read_fully(source, index, at, piece)?;
            let digest = chunk_digest(piece);
            let number = chunks.len();
            let unchanged = parent.and_then(|p| p.chunks.get(number)).is_some_and(|c| c.digest == digest);
            if !unchanged {
                let mut payload = Vec::with_capacity(len.saturating_add(POSITION_BYTES));
                payload.put_u64(u64::try_from(number).unwrap_or(u64::MAX));
                payload.extend_from_slice(piece);
                object.block(&payload)?;
                self.written = self.written.saturating_add(1);
            }
            chunks.push(Chunk { digest, present: !unchanged });
            at = at.saturating_add(u64::try_from(len).unwrap_or(u64::MAX));
            self.db.progressed(&self.id, u64::try_from(len).unwrap_or(u64::MAX));
        }
        self.chunks = self.chunks.saturating_add(u64::try_from(chunks.len()).unwrap_or(u64::MAX));
        self.bytes = self.bytes.saturating_add(object.finish()?);
        Ok(ManifestFile { within: name, owner: owner(file), length: file.length, chunks })
    }

    /// Copies the log's records from `from` to its end, synced first: the
    /// position after the last, and how many there were.
    fn records(&mut self, from: Lsn) -> Result<(Lsn, u64)> {
        let log = self.db.snapshot()?.log.clone().ok_or_else(|| super::super::api("[EXTENSION] a database kept in files has a log"))?;
        let end = log.end();
        let mut object = ObjectOut::begin(self.target, &format!("{}{LOG}", self.prefix), self.writing)?;
        let (mut last, mut count) = (None, 0_u64);
        for read in log.read_from(from) {
            let (at, bytes) = read?;
            if at >= end {
                break;
            }
            self.check()?;
            let mut payload = Vec::with_capacity(bytes.len().saturating_add(POSITION_BYTES));
            payload.put_u64(at.0);
            payload.extend_from_slice(&bytes);
            object.block(&payload)?;
            (last, count) = (Some(at), count.saturating_add(1));
        }
        if let Some(at) = last {
            log.sync(at)?;
        }
        self.bytes = self.bytes.saturating_add(object.finish()?);
        Ok((end, count))
    }
}

/// Fills `into` from file `index` of `source` at `at`.
fn read_fully(source: &mut dyn StateSource, index: usize, at: u64, into: &mut [u8]) -> Result<()> {
    let mut filled = 0;
    while filled < into.len() {
        let offset = at.saturating_add(u64::try_from(filled).unwrap_or(u64::MAX));
        let read = source.read(index, offset, into.get_mut(filled..).unwrap_or_default())?;
        if read == 0 {
            return Err(super::super::api("[EXTENSION] RULES.md D160: a file of the export ends before its length"));
        }
        filled = filled.saturating_add(read);
    }
    Ok(())
}

impl Database {
    /// \[EXTENSION\] Takes a backup as `options` say, while the database
    /// goes on serving (RULES.md D160): see the module's documentation.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the database is not kept in files, or a parent
    /// asked for is not in the target; -916 (SQLSTATE TBB01) if the parent
    /// is damaged; the target's; a checkpoint's.
    pub fn backup(&self, options: &BackupOptions) -> Result<BackupReport> {
        self.backup_stopping(options, None)
    }

    /// `backup`, cancelled by `stop`, if given, as it looks before each
    /// chunk and record.
    pub(super) fn backup_stopping(&self, options: &BackupOptions, stop: Option<&Stop>) -> Result<BackupReport> {
        let target = self.open_target(&options.location)?;
        let (writing, reading) = self.backup_formats()?;
        let parent = options.incremental.as_deref().map(|p| super::chain::manifest_of(&target, p, &reading)).transpose()?;
        self.checkpoint()?;
        let mut export = self.export()?;
        let (image, from) = self.recovery_from(export.image())?;
        let id = self.new_backup_id(&*target)?;
        let timeline = u32::try_from(self.snapshot()?.settings.integer(ARCHIVE_TIMELINE)).unwrap_or(1);
        let kind = if parent.is_some() { BackupKind::Incremental } else { BackupKind::Full };
        let entry = BackupEntry {
            id: id.clone(),
            location: target.location(),
            kind,
            parent: parent.as_ref().map(|p| p.id.clone()).unwrap_or_default(),
            timeline,
            from,
            stop: Lsn(0),
            bytes: 0,
            state: BackupState::Running,
            label: options.label.clone(),
            started: self.snapshot()?.clock.now(),
            ended: 0,
        };
        let total = export.files().iter().map(|f| f.length).fold(0_u64, u64::saturating_add);
        self.begin_backup(&entry, total)?;
        let mut copying = Copying {
            db: self,
            target: &*target,
            writing: &writing,
            prefix: backup_prefix(&id),
            id,
            stop,
            bytes: 0,
            chunks: 0,
            written: 0,
        };
        let made = copying.backup(&mut *export, (image.through, from), parent.as_ref(), &entry);
        drop(export);
        self.end_backup(entry, made, &*target)
    }
}

impl Copying<'_> {
    /// Copies the backup: its files, its image, its records, then its
    /// manifest.
    fn backup(
        &mut self,
        export: &mut dyn StateSource,
        (through, from): (Lsn, Lsn),
        parent: Option<&Manifest>,
        entry: &BackupEntry,
    ) -> Result<(Manifest, BackupReport)> {
        let listed: Vec<ExportedFile> = export.files().to_vec();
        let mut files = Vec::new();
        for (index, file) in listed.iter().enumerate() {
            if file.within.as_path() == std::path::Path::new(RECORDS_FILE) {
                continue;
            }
            let name = within(file);
            let prior = parent.and_then(|p| p.files.iter().find(|f| f.within == name));
            files.push(self.file(export, (index, file), prior)?);
        }
        self.bytes = self.bytes.saturating_add(write_one(self.target, &format!("{}{IMAGE}", self.prefix), self.writing, export.image())?);
        let (stop, records) = self.records(from)?;
        let manifest = Manifest {
            id: self.id.clone(),
            kind: entry.kind,
            parent: entry.parent.clone(),
            timeline: entry.timeline,
            from,
            through,
            stop,
            started: entry.started,
            ended: self.db.snapshot()?.clock.now(),
            label: entry.label.clone(),
            records,
            bytes: self.bytes,
            files,
        };
        // The last look before the manifest makes it a backup.
        self.check()?;
        write_one(self.target, &format!("{}{MANIFEST}", self.prefix), self.writing, &manifest.encode())?;
        let report =
            BackupReport { id: self.id.clone(), kind: entry.kind, stop, bytes: self.bytes, chunks: self.chunks, written: self.written };
        Ok((manifest, report))
    }
}

impl Database {
    /// Notes a backup begun: under way, and in the history.
    fn begin_backup(&self, entry: &BackupEntry, total: u64) -> Result<()> {
        lock_builtin(&self.backups().running).insert(entry.id.clone(), Progress { entry: entry.clone(), done: 0, total });
        self.record_history(Entry::Backup(entry.clone()))
    }

    /// Counts `bytes` more copied of backup `id`.
    fn progressed(&self, id: &str, bytes: u64) {
        if let Some(p) = lock_builtin(&self.backups().running).get_mut(id) {
            p.done = p.done.saturating_add(bytes);
        }
    }

    /// Notes a backup ended as `made` says: completed, or failed or
    /// cancelled, what it wrote then removed.
    fn end_backup(
        &self,
        mut entry: BackupEntry,
        made: Result<(Manifest, BackupReport)>,
        target: &dyn BackupTarget,
    ) -> Result<BackupReport> {
        lock_builtin(&self.backups().running).remove(&entry.id);
        entry.ended = self.snapshot()?.clock.now();
        match made {
            Ok((manifest, report)) => {
                entry = BackupEntry { state: BackupState::Completed, stop: manifest.stop, bytes: report.bytes, ..entry };
                self.record_history(Entry::Backup(entry))?;
                Ok(report)
            }
            Err(e) => {
                entry.state = if e.code == Code::Cancelled { BackupState::Cancelled } else { BackupState::Failed };
                for name in target.list(&backup_prefix(&entry.id)).unwrap_or_default() {
                    // Tidying after the failure reported. An object left
                    // is no backup without its manifest, written last.
                    let _ = target.remove(&name);
                }
                // The failure is what is reported; a history entry lost
                // shows the backup as interrupted.
                let _ = self.record_history(Entry::Backup(entry));
                Err(e)
            }
        }
    }

    /// An id for a backup to `target`: when it begins, then a count, so
    /// ids sort as backups began.
    fn new_backup_id(&self, target: &dyn BackupTarget) -> Result<String> {
        let micros = u64::try_from(self.snapshot()?.clock.now()).unwrap_or(0);
        loop {
            let n = self.backups().taken.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let id = format!("B{micros:020}-{n:06}");
            if target.list(&backup_prefix(&id))?.is_empty() {
                return Ok(id);
            }
        }
    }

    /// The formats a backup's objects are written in, compressed first if
    /// BACKUP.COMPRESSION says, and read in.
    pub(super) fn backup_formats(&self) -> Result<(Writing, crate::backup::object::Reading)> {
        let (mut writing, reading) = self.object_formats()?;
        let st = self.snapshot()?;
        if st.settings.is_on(crate::settings::BACKUP_COMPRESSION) {
            let lz4 = reading.encodings.iter().find(|e| e.id() == crate::disk::encoding::EncodingId(LZ4_ID)).cloned().ok_or_else(|| {
                super::super::api("[EXTENSION] RULES.md D161: BACKUP.COMPRESSION is ON, but the LZ4 encoding (id 1) is not registered")
            })?;
            let mut stages: Vec<Arc<dyn crate::disk::encoding::Encoding>> = vec![lz4];
            stages.extend(writing.pipeline.stages().iter().filter(|s| s.id().0 != LZ4_ID).cloned());
            writing.pipeline = crate::disk::encoding::Pipeline::new(stages)?;
        }
        Ok((writing, reading))
    }
}

/// \[EXTENSION\] The id of the LZ4 encoding (RULES.md D48), which
/// BACKUP.COMPRESSION puts first.
const LZ4_ID: u32 = 1;
