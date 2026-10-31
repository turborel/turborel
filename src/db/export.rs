//! \[DESIGN\] The manifest of a database's durable files (RULES.md D57):
//! what an export of the database carries besides its log's records and
//! its catalog's image. Every file in its directory is of a kind the file
//! manager knows (`files/classes.rs`); of those, the durable ones but the
//! log's and the catalog's, which their own stores carry, are listed, each
//! with its length and owner, as the last checkpoint left them: the
//! checkpoints' turn is held while they are read, so none changes the data
//! file meanwhile. A file of no kind known is refused, as an export would
//! leave its state behind; a feature keeping state there declares an
//! export hook instead.

use std::path::{Path, PathBuf};

use super::files::LOCK;
use super::{Database, api};
use crate::disk::io::{FileSystem, OpenMode};
use crate::error::Result;
use crate::files::{FileClass, FileKind, FileOwner};

/// \[DESIGN\] A durable file of a database, as its manifest lists it
/// (RULES.md D57).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedFile {
    /// Where it is.
    pub path: PathBuf,
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
        let _turn = files.checkpointer.taking.take("an export")?;
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
                listed.push(ExportedFile { length, owner: kind.owner(within), path });
            }
        }
        Ok(listed)
    }
}

/// Every file under `dir` of `fs`, by path: an entry that reads as a
/// directory is one, and is walked in turn.
fn walk(fs: &dyn FileSystem, dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(at) = pending.pop() {
        for entry in fs.read_dir(&at)? {
            match fs.read_dir(&entry) {
                Ok(_) => pending.push(entry),
                Err(_) => found.push(entry),
            }
        }
    }
    found.sort();
    Ok(found)
}
