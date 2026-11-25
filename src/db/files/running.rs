//! \[DESIGN\] The mark a database reopened clean keeps in its directory
//! while it runs (RULES.md D1164), in place of an image recorded again.
//!
//! A database reopened from the image of a clean close used to record
//! that image again, not clean, before anything else ran, so a crash from
//! then on was recovered; and its close recorded one more, clean. A start
//! that changed nothing so grew the catalog by two images. A crash cannot
//! be told by the log alone: a transaction open at the crash may have
//! logged nothing and still left its older versions or pages behind.
//!
//! So a database reopened clean makes the file `opened` in its directory,
//! durably, before anything else runs, and its clean close takes it out
//! once the close's image, if one was recorded, is durable. Opened while
//! the file is there, its last image is taken as not clean, and it is
//! recovered from it, as from the image the open recorded before. One
//! recovered, or made, records its image not clean as before.

use std::path::Path;
use std::sync::Arc;

use super::{DatabaseFiles, Image, Opened};
use crate::db::stores::CatalogStore;
use crate::disk::io::{Durability, FileSystem, OpenMode};
use crate::error::Result;

pub(in crate::db) use crate::files::OPENED;

/// `image`, the last recorded in `dir`, as not clean if the database ran
/// from it and stopped without closing.
pub(super) fn as_left(fs: &Arc<dyn FileSystem>, dir: &Path, image: Option<Image>) -> Result<Option<Image>> {
    let ran = fs.exists(&dir.join(OPENED))?;
    Ok(image.map(|image| Image { clean: image.clean && !ran, ..image }))
}

/// As a database in `dir` opened as `opened`, from `image`: marked as
/// running, if reopened clean; else its image recorded again, not clean.
pub(super) fn begun(
    (fs, dir): (&Arc<dyn FileSystem>, &Path),
    (catalog, compact_after): (&dyn CatalogStore, usize),
    (opened, image): (Opened, &Image),
) -> Result<()> {
    if opened != Opened::Reopened {
        return catalog.record(&Image { clean: false, ..image.clone() }.encode(), compact_after);
    }
    let path = dir.join(OPENED);
    if !fs.exists(&path)? {
        fs.open(&path, OpenMode::CreateNew)?.sync(Durability::Full)?;
        fs.sync_dir(dir)?;
    }
    Ok(())
}

impl DatabaseFiles {
    /// Takes the mark of running out, durably, of a database closed clean.
    ///
    /// # Errors
    ///
    /// The file system's: the database is then recovered as it opens again.
    pub(in crate::db) fn ended(&self) -> Result<()> {
        let (fs, dir) = &self.dir;
        let path = dir.join(OPENED);
        if fs.exists(&path)? {
            fs.remove(&path)?;
            fs.sync_dir(dir)?;
        }
        Ok(())
    }
}
