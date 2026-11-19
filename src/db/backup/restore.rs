//! \[EXTENSION\] A restore (RULES.md D162, D164, D165): offline, into a directory
//! not there yet. The backup's chain is read, its files made in a staging
//! directory beside the one asked for (`STAGING`), the newest backup's
//! image recorded as the catalog; then the database is opened there
//! through a seeded log (`backup/seeded.rs`): the backup's records, then
//! the archive's, along the timeline's history, to the recovery target,
//! then a new file log. Recovery replays them as after a crash. A new
//! timeline is begun, its history written to the archive, a checkpoint
//! lets the seed go, and the database is closed and its directory renamed
//! to the one asked for. A crash meanwhile leaves the staging directory
//! alone, which no database opens, and which a restore run again removes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::assemble::assemble;
use super::chain::chain_of;
use crate::backup::history::Entry;
use crate::backup::manifest::Manifest;
use crate::backup::object::{ObjectIn, Reading};
use crate::backup::seed::{RecoveryTarget, Seed, SeedPart};
use crate::backup::seeded::SeededLogs;
use crate::backup::timeline::{self, Ancestry};
use crate::backup::{BackupTarget, IMAGE, LOG, backup_prefix};
use crate::db::image::Image;
use crate::db::{Database, OpenOptions, StorePlace, api};
use crate::disk::io::{FileSystem, OsFileSystem};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::log::{FileLogOptions, Lsn};
use crate::value::{Decimal, Value};

/// \[EXTENSION\] The suffix of a restore's staging directory.
pub const STAGING: &str = ".restoring";

/// \[EXTENSION\] What a restore is asked to do.
#[derive(Debug, Clone)]
pub struct RestoreOptions {
    /// The directory to restore into, not there yet.
    pub dir: PathBuf,
    /// The backup's target's location.
    pub location: String,
    /// The backup's id, or empty for the newest.
    pub backup: String,
    /// The archive's location, if records past the backup are replayed.
    pub archive: Option<String>,
    /// Where recovery stops.
    pub target: RecoveryTarget,
    /// The timeline recovered along, if not the backup's.
    pub timeline: Option<u32>,
}

/// \[EXTENSION\] What a restore did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    /// The backup restored.
    pub backup: String,
    /// The timeline the restored database begins.
    pub timeline: u32,
    /// The last record replayed.
    pub last: Lsn,
    /// The records replayed.
    pub records: u64,
}

/// The staging directory of `dir`.
fn staging(dir: &Path) -> PathBuf {
    let mut name = dir.as_os_str().to_owned();
    name.push(STAGING);
    PathBuf::from(name)
}

/// Where a file named `within` goes under `dir`.
fn place_of(dir: &Path, within: &str) -> PathBuf {
    within.split('/').fold(dir.to_path_buf(), |path, part| path.join(part))
}

/// The objects of records on the path from backup `newest` to timeline
/// `timeline`'s end in `archive`, and the highest position any holds.
fn seed_parts(
    target: &Arc<dyn BackupTarget>,
    newest: &Manifest,
    archive: Option<&Arc<dyn BackupTarget>>,
    (timeline, reading): (u32, &Reading),
) -> Result<(Vec<SeedPart>, Ancestry, Lsn)> {
    let mut parts = vec![SeedPart { target: Arc::clone(target), object: format!("{}{LOG}", backup_prefix(&newest.id)), upto: None }];
    let mut highest = newest.stop;
    let Some(archive) = archive else { return Ok((parts, Vec::new(), highest)) };
    let ancestry = timeline::ancestry(archive, timeline, reading)?;
    let path = ancestry.iter().map(|&(t, branch)| (t, Some(branch))).chain([(timeline, None)]);
    for (t, upto) in path {
        for segment in timeline::segments(&**archive, t)? {
            if segment.last < newest.stop {
                continue;
            }
            highest = highest.max(segment.last);
            parts.push(SeedPart { target: Arc::clone(archive), object: segment.name, upto });
        }
    }
    Ok((parts, ancestry, highest))
}

/// The refusal of a target not met.
fn not_reached(target: &RecoveryTarget) -> SqlError {
    SqlError::new(
        Code::Api,
        format!("[EXTENSION] RULES.md D164: the recovery target {target:?} is not among the records the backup and the archive hold"),
    )
    .with_state(SqlState::RECOVERY_TARGET_NOT_REACHED)
}

impl Database {
    /// \[EXTENSION\] Restores a backup into `options.dir`, recovering it to
    /// `options.target` (RULES.md D164): see the module's documentation.
    /// This database, given the plug-ins, encodings and keys the backup's
    /// was, recovers the files and is closed after; the directory is then
    /// opened again as any is.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if this database holds files or a log already, the
    /// directory is there, or there is no such backup; -910 (SQLSTATE
    /// TBB02) if the target is not met; -916 (SQLSTATE TBB01) if an object
    /// is damaged; the target's; recovery's.
    pub fn restore(&self, options: &RestoreOptions) -> Result<RestoreReport> {
        if crate::backup::decide::restore_refused((self.files()?.is_some(), self.log_storage_set().is_some())) {
            return Err(api("[EXTENSION] RULES.md D162: a restore opens a database of nothing yet, with no log storage of its own"));
        }
        // A host plug-in's archive is reached before any database opens.
        self.start_host_plugins()?;
        let fs: Arc<dyn FileSystem> = Arc::new(OsFileSystem);
        if fs.exists(&options.dir)? {
            return Err(api(format!(
                "[EXTENSION] RULES.md D162: {} is there already, and a restore makes a directory anew",
                options.dir.display()
            )));
        }
        let staged = staging(&options.dir);
        if fs.exists(&staged)? {
            std::fs::remove_dir_all(&staged).map_err(|e| api(format!("[EXTENSION] {} could not be removed: {e}", staged.display())))?;
        }
        let restored = self.restore_staged(options, &fs, &staged);
        if restored.is_err() {
            // The failure is what is reported; what was staged goes.
            let _ = self.close();
            let _ = std::fs::remove_dir_all(&staged);
        }
        let report = restored?;
        std::fs::rename(&staged, &options.dir)
            .map_err(|e| SqlError::new(Code::Io, format!("[EXTENSION] {} could not be renamed: {e}", staged.display())))?;
        options.dir.parent().filter(|p| !p.as_os_str().is_empty()).map_or(Ok(()), |p| fs.sync_dir(p))?;
        Ok(report)
    }

    /// The work of `restore`, into `staged` of `fs`.
    fn restore_staged(&self, options: &RestoreOptions, fs: &Arc<dyn FileSystem>, staged: &Path) -> Result<RestoreReport> {
        let target = self.open_target(&options.location)?;
        let (_, reading) = self.object_formats()?;
        let chain = chain_of(&target, &options.backup, &reading)?;
        let newest = chain.first().ok_or_else(|| api("[EXTENSION] a backup's chain holds the backup"))?.clone();
        fs.create_dir_durably(staged)?;
        for file in &newest.files {
            assemble(&target, (&chain, file), (&**fs, &place_of(staged, &file.within)), &reading)?;
        }
        let image = ObjectIn::only(&target, &format!("{}{IMAGE}", backup_prefix(&newest.id)), &reading)?;
        self.record_restored_image(fs, staged, &image)?;
        let archive = options.archive.as_deref().map(|l| self.open_target(l)).transpose()?;
        let timeline = options.timeline.unwrap_or(newest.timeline);
        let (parts, ancestry, highest) = seed_parts(&target, &newest, archive.as_ref(), (timeline, &reading))?;
        let seed = Arc::new(Seed::new(parts, reading.clone(), options.target.clone()));
        self.set_log_storage(Box::new(SeededLogs::new(Arc::clone(&seed), highest, FileLogOptions::default())))?;
        self.open_files(fs, staged, &OpenOptions::default())?;
        let reached = seed.reached();
        if !reached.met {
            return Err(not_reached(&options.target));
        }
        let next = self.next_timeline(archive.as_ref(), (timeline, ancestry), reached.last)?;
        self.set_setting("ARCHIVE.TIMELINE", &Value::Exact(Decimal::new(i128::from(next), 0)))?;
        let wall = self.snapshot()?.clock.now();
        self.record_history(Entry::Restored {
            location: target.location(),
            id: newest.id.clone(),
            timeline: next,
            branch: reached.last,
            wall,
        })?;
        self.checkpoint()?;
        self.close()?;
        Ok(RestoreReport { backup: newest.id, timeline: next, last: reached.last, records: reached.records })
    }

    /// Records `image` as the catalog of the database in `dir` of `fs`, in
    /// the formats its settings name.
    fn record_restored_image(&self, fs: &Arc<dyn FileSystem>, dir: &Path, image: &[u8]) -> Result<()> {
        let st = self.snapshot()?;
        let decoded = Image::decode(image, &st.types)?;
        let recorded = |name: &str| decoded.snapshot.setting(name).cloned();
        let values = st.recorded(&recorded)?;
        let formats = st.layout(&values)?.formats;
        let catalog = self.catalog_storage().create(&StorePlace::new(fs, dir, &formats))?;
        catalog.record(image, st.compact_after())
    }

    /// The timeline a restore begins: past every one the archive has, its
    /// history written there, naming `ancestry` and `timeline` as far as
    /// `branch`; or past `timeline`, with no archive.
    fn next_timeline(&self, archive: Option<&Arc<dyn BackupTarget>>, (timeline, ancestry): (u32, Ancestry), branch: Lsn) -> Result<u32> {
        let Some(archive) = archive else { return Ok(timeline.saturating_add(1)) };
        let next = timeline::highest(&**archive)?.unwrap_or(timeline).max(timeline).saturating_add(1);
        let mut lineage = ancestry;
        lineage.push((timeline, branch));
        let (writing, _) = self.backup_formats()?;
        timeline::write_history(&**archive, (next, &lineage), &writing)?;
        Ok(next)
    }
}
