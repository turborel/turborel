//! \[EXTENSION\] A replica's database installing an export of the
//! primary's (RULES.md D57, `Standby::import`): deposed or following, its
//! service ended as it is opened again in place (`standby/reopen.rs`),
//! then, in its directory, the data file, its side files, the plug-ins'
//! files and the catalog's files removed, the export's files written where
//! its manifest says and synced, and the export's image recorded in a new
//! catalog file; the journal in `log/` is its replica's, kept. Opened
//! again to follow, it recovers from the image alone, and holds the log
//! through the position the image does. A crash meanwhile leaves a
//! directory no checkpoint holds whole, which the replica's next start
//! fetches the state again into: nothing of it is read before the catalog
//! is, and the catalog is the last written.

use std::path::Path;
use std::sync::Arc;

use super::super::export::StateSource;
use super::super::files::{LOCK, Mode};
use super::super::stores::StorePlace;
use super::super::{Database, State, api};
use crate::disk::io::{Durability, FileSystem, OpenMode, walk};
use crate::error::Result;
use crate::files::FileKind;
use crate::log::Lsn;
use crate::sync::lock;
use crate::txn::Transactions;

/// \[DESIGN\] The bytes read from an export's file at a time.
const PIECE: usize = 1 << 20;

/// Writes the export `source`'s files into `dir` of `fs`, where its
/// manifest says, each synced.
/// Removes the files of `dir` that hold the database's state, the
/// export's replacing them: the data file and its side files, the
/// plug-ins' and the catalog's; the log's and the lock stay.
///
/// # Errors
///
/// SQLCODE -915 if a file cannot be removed.
pub(in crate::db) fn clear_state_files(fs: &dyn FileSystem, dir: &Path) -> Result<()> {
    for path in walk(fs, dir)? {
        let within = path.strip_prefix(dir).unwrap_or(&path);
        let replaced = matches!(FileKind::of(within), FileKind::Data | FileKind::Shadow | FileKind::Plugin | FileKind::Catalog);
        if replaced && within != Path::new(LOCK) {
            fs.remove(&path)?;
        }
    }
    Ok(())
}

fn write_files(fs: &dyn FileSystem, dir: &Path, source: &mut dyn StateSource) -> Result<()> {
    let mut piece = vec![0; PIECE];
    for (index, file) in source.files().to_vec().iter().enumerate() {
        // The records the export carries are the log's: the replica's
        // driver holds them in its log before the export is installed.
        if FileKind::of(&file.within) == FileKind::Log {
            continue;
        }
        let target = dir.join(&file.within);
        if let Some(parent) = target.parent() {
            fs.create_dir_all(parent)?;
        }
        let written = fs.open(&target, OpenMode::CreateNew)?;
        let mut at = 0_u64;
        while at < file.length {
            let read = source.read(index, at, &mut piece)?;
            if read == 0 {
                return Err(api(format!("[DESIGN] RULES.md D57: the export's file {} ends before its length", file.within.display())));
            }
            written.write_all_at(piece.get(..read).unwrap_or_default(), at)?;
            at = at.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        }
        written.sync(Durability::Full)?;
    }
    Ok(())
}

impl Database {
    /// Installs `source`: see the module's documentation.
    pub(in crate::db) fn import_state(&self, mut source: Box<dyn StateSource>) -> Result<Lsn> {
        let opening = self.opening()?.ok_or_else(|| api("[DESIGN] RULES.md D57: only a database opened from its files takes an export"))?;
        let files = self.files()?.ok_or_else(|| api("[DESIGN] RULES.md D57: only a database opened from its files takes an export"))?;
        let current = self.snapshot()?;
        if current.tx.held().is_none() {
            return Err(api("[DESIGN] RULES.md D57: a database serving is deposed before it takes an export"));
        }
        let retired = current.tx.held() == Some(crate::txn::Held::Retired);
        {
            // No checkpoint nor maintenance of the service ending meanwhile.
            let _checkpoint = files.checkpointer.taking.take("a checkpoint")?;
            let _maintenance = self.background().maintenance_turn.take("maintenance")?;
            lock(&files.held)?.take();
            let fresh = State {
                generation: current.generation.saturating_add(1),
                tx: Arc::new(Transactions::default()),
                // The system tables registered since it was first opened
                // (a replica's REPLICAS) are the handle's, kept.
                system: current.system.clone(),
                ..(*opening.state).clone()
            };
            drop(current);
            self.restart_service(fresh)?;
        }
        let (fs, dir) = (&opening.fs, opening.dir.as_path());
        clear_state_files(&**fs, dir)?;
        write_files(&**fs, dir, &mut *source)?;
        let formats = opening.state.layout(&opening.state.settings)?.formats;
        let catalog = self.catalog_storage().create(&StorePlace::new(fs, dir, &formats))?;
        catalog.record(source.image(), opening.state.compact_after())?;
        drop(catalog);
        fs.sync_dir(dir)?;
        self.open_managed(fs, dir, &opening.options, Mode::Follow)?;
        self.stay_retired(retired)?;
        self.holds_through()
    }
}
