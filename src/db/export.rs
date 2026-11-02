//! \[DESIGN\] The export of a database (RULES.md D57): its last
//! checkpoint's image, and the manifest of its durable files, what an
//! export carries besides its log's records. Every file in its directory
//! is of a kind the file manager knows (`files/classes.rs`); of those, the
//! durable ones but the log's and the catalog's, which their own stores
//! carry, are listed, each with its length and owner, as the last
//! checkpoint left them: the checkpoints' turn is held while they are
//! read, by the export for its life (`Database::export`), so none changes
//! the data file meanwhile; held shared, so several replicas fetch at
//! once, and a checkpoint waits until the last export is dropped. A file
//! of no kind known is refused, as an
//! export would leave its state behind; a feature keeping state there
//! declares an export hook instead. A replica installs an export by
//! `Standby::import` (`db/standby/import.rs`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::files::{DatabaseFiles, LOCK};
use super::{Database, api};
use crate::disk::io::{DiskFile, OpenMode, walk};
use crate::error::{Code, Result, SqlError};
use crate::files::{FileClass, FileKind, FileOwner};
use crate::threads::Hold;

/// \[DESIGN\] A durable file of a database, as its manifest lists it
/// (RULES.md D57).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedFile {
    /// Where it is.
    pub path: PathBuf,
    /// Where it is within the database's directory: where a replica
    /// installing the export puts it.
    pub within: PathBuf,
    /// Its length in bytes.
    pub length: u64,
    /// Whose it is.
    pub owner: FileOwner,
}

/// \[DESIGN\] An export of a database (RULES.md D57): its last
/// checkpoint's image and the durable files of its manifest, read as they
/// were when the export began. Replication moves it to another replica
/// without decoding it (a `StateTransfer` of crate `turborel-replication`);
/// the core makes one and installs one.
///
/// Contract: `image` and `files` do not change for the life of the source;
/// `read` of file `file` at offset `at` fills `into` from the file as the
/// export began, and returns how many bytes it filled, fewer only at the
/// file's end, so a source read twice gives the same bytes.
pub trait StateSource: Send {
    /// The checkpoint's image: an opaque catalog record.
    fn image(&self) -> &[u8];
    /// The durable files, as the manifest lists them.
    fn files(&self) -> &[ExportedFile];
    /// Reads file `file` (an index into `files`) at offset `at` into `into`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there is no file `file`; -915 if it cannot be read.
    fn read(&mut self, file: usize, at: u64, into: &mut [u8]) -> Result<usize>;
}

impl Database {
    /// \[DESIGN\] The durable files of this database, kept in a directory,
    /// as its last checkpoint left them, but its log's and its catalog's:
    /// see the module's documentation. By path.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it was not opened from files, or a file in its
    /// directory is of no kind the file manager knows; -915 if a file or
    /// directory cannot be read.
    pub fn export_manifest(&self) -> Result<Vec<ExportedFile>> {
        let files = self.files()?.ok_or_else(|| api("[DESIGN] RULES.md D57: only a database opened from its files exports them"))?;
        let hold = files.checkpointer.taking.hold("an export")?;
        let listed = manifest(&files);
        files.checkpointer.taking.leave(hold);
        listed
    }

    /// \[DESIGN\] An export of this database (RULES.md D57): its last
    /// checkpoint's image and its manifest's files, read as they are now,
    /// which the checkpoints' turn, held for the export's life, keeps so.
    ///
    /// # Errors
    ///
    /// As `export_manifest`; SQLCODE -910 if no checkpoint was recorded.
    pub fn export(&self) -> Result<Box<dyn StateSource>> {
        let files = self.files()?.ok_or_else(|| api("[DESIGN] RULES.md D57: only a database opened from its files exports them"))?;
        let hold = files.checkpointer.taking.hold("an export")?;
        // Held from here on by the export, which leaves it as it is dropped:
        // so by one made with nothing yet, if the rest fails.
        let mut export = Export { files: Arc::clone(&files), hold: Some(hold), image: Vec::new(), listed: Vec::new(), open: Vec::new() };
        export.listed = manifest(&files)?;
        export.image = files
            .catalog
            .last()
            .ok_or_else(|| api("[DESIGN] RULES.md D57: a database exports its last checkpoint, and none was recorded"))?;
        export.open = export.listed.iter().map(|_| None).collect();
        Ok(Box::new(export))
    }
}

/// The manifest of `files`' directory, the checkpoints' turn held.
fn manifest(files: &DatabaseFiles) -> Result<Vec<ExportedFile>> {
    let (fs, dir) = (&*files.dir.0, files.dir.1.as_path());
    let mut listed = Vec::new();
    for path in walk(fs, dir)? {
        let within = path.strip_prefix(dir).unwrap_or(&path);
        if within == Path::new(LOCK) {
            continue;
        }
        let kind = FileKind::of(within);
        let Some(class) = kind.class() else {
            return Err(api(format!(
                "[DESIGN] RULES.md D57: {} is of no kind of file the database knows, so an export would leave it behind",
                path.display()
            )));
        };
        // Carried by the log's records and the catalog's image.
        let carried = matches!(kind, FileKind::Log | FileKind::Catalog);
        if class == FileClass::Durable && !carried {
            let length = fs.open(&path, OpenMode::ReadOnly)?.size()?;
            listed.push(ExportedFile { length, owner: kind.owner(within), within: within.to_path_buf(), path });
        }
    }
    Ok(listed)
}

/// An export of a database: see the module's documentation.
struct Export {
    files: Arc<DatabaseFiles>,
    /// Its hold of the checkpoints' turn, until it is dropped.
    hold: Option<Hold>,
    image: Vec<u8>,
    listed: Vec<ExportedFile>,
    /// Each file, once opened for reading.
    open: Vec<Option<Box<dyn DiskFile>>>,
}

impl StateSource for Export {
    fn image(&self) -> &[u8] {
        &self.image
    }

    fn files(&self) -> &[ExportedFile] {
        &self.listed
    }

    fn read(&mut self, file: usize, at: u64, into: &mut [u8]) -> Result<usize> {
        let path = self
            .listed
            .get(file)
            .map(|f| f.path.clone())
            .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] RULES.md D57: the export has no file {file}")))?;
        let slot = self
            .open
            .get_mut(file)
            .ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] RULES.md D57: the export has no file {file}")))?;
        if slot.is_none() {
            *slot = Some(self.files.dir.0.open(&path, OpenMode::ReadOnly)?);
        }
        slot.as_ref().map_or(Ok(0), |opened| opened.read_at(into, at))
    }
}

impl Drop for Export {
    /// The checkpoints go on, once every export is dropped.
    fn drop(&mut self) {
        if let Some(hold) = self.hold.take() {
            self.files.checkpointer.taking.leave(hold);
        }
    }
}
