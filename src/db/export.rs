//! \[DESIGN\] The export of a database (RULES.md D57): its last
//! checkpoint's image, the manifest of its durable files, and the log's
//! records the checkpoint is recovered with. Every file in its directory
//! is of a kind the file manager knows (`files/classes.rs`); of those, the
//! durable ones but the log's and the catalog's, which their own stores
//! carry, are listed, each with its length and owner, as the last
//! checkpoint left them: the checkpoints' turn is held while they are
//! read, by the export for its life (`Database::export`), so none changes
//! the data file meanwhile; held shared, so several replicas fetch at
//! once, and a checkpoint waits until the last export is dropped. A file
//! of no kind known is refused, as an
//! export would leave its state behind; a feature keeping state there
//! declares an export hook instead. The records recovery reads from the
//! checkpoint, from the first of the oldest transaction open at it, or
//! the oldest change a table's pages lack, through its position, go with
//! it as one more file (`RECORDS_FILE`): a replica installing the export
//! holds them in its own log first, so its database recovers from the
//! checkpoint as this one would, the transactions open at it undone or
//! redone from them; without them their changes in the pages, and their
//! older versions, which the undo store does not carry, would be beyond
//! reach. A replica installs an export by `Standby::import`
//! (`db/standby/import.rs`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::files::{DatabaseFiles, LOCK};
use super::image::Image;
use super::{Database, api};
use crate::bytes::{Put, Reader};
use crate::disk::io::{DiskFile, OpenMode, walk};
use crate::error::{Code, Result, SqlError};
use crate::files::{FileClass, FileKind, FileOwner};
use crate::log::Lsn;
use crate::threads::Hold;
use crate::txn::Persistence;

/// \[DESIGN\] The file of an export carrying the log's records its
/// checkpoint is recovered with, within the database's directory: of the
/// log's kind, so a replica installing the export writes it among its
/// files not, but into its log.
pub const RECORDS_FILE: &str = "log/export.records";

/// The records of an export's file `RECORDS_FILE` encoded: the position
/// before the first, then each, its position and bytes.
fn encode_records(holds: Lsn, records: &[(Lsn, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.put_u64(holds.0);
    out.put_u32(u32::try_from(records.len()).unwrap_or(u32::MAX));
    for (at, record) in records {
        out.put_u64(at.0);
        out.put_bytes(record);
    }
    out
}

/// \[DESIGN\] The records of an export's file `RECORDS_FILE`, as
/// `Database::export` wrote it: the position before the first, and the
/// records from it on, in order (RULES.md D57).
///
/// # Errors
///
/// SQLCODE -916 if the bytes are malformed, or the records are out of
/// order.
pub fn decode_records(bytes: &[u8]) -> Result<(Lsn, Vec<Vec<u8>>)> {
    let mut r = Reader::new(bytes, "an export's records");
    let holds = Lsn(r.u64()?);
    let count = r.u32()?;
    let mut records = Vec::new();
    for index in 0..u64::from(count) {
        let at = r.u64()?;
        if at != holds.0.saturating_add(index).saturating_add(1) {
            return Err(SqlError::new(Code::Corrupt, format!("[DESIGN] RULES.md D57: an export's record at {at} is out of order")));
        }
        records.push(r.bytes()?.to_vec());
    }
    r.finish()?;
    Ok((holds, records))
}

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
    /// which the checkpoints' turn, held for the export's life, keeps so,
    /// and the log's records the checkpoint is recovered with, as one
    /// more file (`RECORDS_FILE`).
    ///
    /// # Errors
    ///
    /// As `export_manifest`; SQLCODE -910 if no checkpoint was recorded.
    pub fn export(&self) -> Result<Box<dyn StateSource>> {
        let files = self.files()?.ok_or_else(|| api("[DESIGN] RULES.md D57: only a database opened from its files exports them"))?;
        let hold = files.checkpointer.taking.hold("an export")?;
        // Held from here on by the export, which leaves it as it is dropped:
        // so by one made with nothing yet, if the rest fails.
        let mut export = Export {
            files: Arc::clone(&files),
            hold: Some(hold),
            image: Vec::new(),
            listed: Vec::new(),
            open: Vec::new(),
            records: Vec::new(),
        };
        export.listed = manifest(&files)?;
        export.image = files
            .catalog
            .last()
            .ok_or_else(|| api("[DESIGN] RULES.md D57: a database exports its last checkpoint, and none was recorded"))?;
        export.records = self.recovery_records(&export.image)?;
        let length = u64::try_from(export.records.len()).unwrap_or(u64::MAX);
        export.listed.push(ExportedFile { path: PathBuf::new(), within: PathBuf::from(RECORDS_FILE), length, owner: FileOwner::Database });
        export.open = export.listed.iter().map(|_| None).collect();
        Ok(Box::new(export))
    }

    /// The log's records recovery from the checkpoint `image` records
    /// reads, encoded for `RECORDS_FILE`: from the first of the oldest
    /// transaction open at it, or the oldest change a logged table's
    /// pages lack, whichever is first, through the checkpoint's position,
    /// as `db/files.rs` reads them to recover.
    fn recovery_records(&self, image: &[u8]) -> Result<Vec<u8>> {
        let snapshot = self.snapshot()?;
        let image = Image::decode(image, &snapshot.types)?;
        let log = snapshot.log.clone().ok_or_else(|| api("[DESIGN] a database opened from its files has a log"))?;
        let logged = snapshot.storage.values().filter(|s| s.capabilities().persistence == Persistence::Logged);
        let from = logged.map(|s| s.durable_through()).fold(image.start, Ord::min);
        let mut records = Vec::new();
        for read in log.read_from(from) {
            let (at, bytes) = read?;
            if at >= image.through {
                break;
            }
            records.push((at, bytes));
        }
        Ok(encode_records(Lsn(from.0.saturating_sub(1)), &records))
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
    /// The files, the records' last.
    listed: Vec<ExportedFile>,
    /// Each file, once opened for reading.
    open: Vec<Option<Box<dyn DiskFile>>>,
    /// The records file's bytes (`RECORDS_FILE`).
    records: Vec<u8>,
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
        if file.saturating_add(1) == self.listed.len() {
            let from = usize::try_from(at).unwrap_or(usize::MAX).min(self.records.len());
            let piece = self.records.get(from..).unwrap_or_default();
            let count = piece.len().min(into.len());
            into.get_mut(..count).unwrap_or_default().copy_from_slice(piece.get(..count).unwrap_or_default());
            return Ok(count);
        }
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
