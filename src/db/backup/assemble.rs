//! \[EXTENSION\] A restored file made from a backup's chain (RULES.md
//! D162): each chunk taken from the newest backup of the chain holding
//! it, checked against the newest manifest's digest, written at its place;
//! the file cut to its length and synced.

use std::path::Path;
use std::sync::Arc;

use super::verify::chunk;
use crate::backup::manifest::{Manifest, ManifestFile, chunk_digest};
use crate::backup::object::{ObjectIn, Reading};
use crate::backup::{BackupTarget, CHUNK_BYTES, FILES, backup_prefix, damaged};
use crate::disk::io::{Durability, FileSystem, OpenMode};
use crate::error::Result;

/// A backup of the chain as a source of a file's chunks: its entry for
/// the file, if it has one, and its object, once opened.
struct Source<'a> {
    file: Option<&'a ManifestFile>,
    prefix: String,
    object: Option<ObjectIn>,
}

impl Source<'_> {
    /// Whether it holds chunk `number`.
    fn holds(&self, number: usize) -> bool {
        self.file.and_then(|f| f.chunks.get(number)).is_some_and(|c| c.present)
    }

    /// Chunk `number` of its object, those before it passed over.
    fn take(&mut self, target: &Arc<dyn BackupTarget>, number: usize, reading: &Reading) -> Result<Vec<u8>> {
        let within = self.file.map(|f| f.within.clone()).unwrap_or_default();
        let name = format!("{}{FILES}{within}", self.prefix);
        if self.object.is_none() {
            self.object = Some(ObjectIn::open(target, &name, reading)?);
        }
        while let Some(payload) = self.object.as_mut().map_or(Ok(None), ObjectIn::next)? {
            let (found, bytes) = chunk(&payload, &name)?;
            if found == number {
                return Ok(bytes);
            }
            if found > number {
                break;
            }
        }
        Err(damaged(&name, &format!("it lacks its chunk {number}")))
    }
}

/// Writes `file`, as the newest backup of `chain` lists it, from the
/// chain's objects in `target`, to `path` of `fs`: the bytes written.
///
/// # Errors
///
/// SQLCODE -916 (SQLSTATE TBB01) if a chunk is missing or not the one
/// listed; -915 if the file cannot be written; the target's.
pub(super) fn assemble(
    target: &Arc<dyn BackupTarget>,
    (chain, file): (&[Manifest], &ManifestFile),
    (fs, path): (&dyn FileSystem, &Path),
    reading: &Reading,
) -> Result<u64> {
    let mut sources: Vec<Source<'_>> = chain
        .iter()
        .map(|m| Source { file: m.files.iter().find(|f| f.within == file.within), prefix: backup_prefix(&m.id), object: None })
        .collect();
    if let Some(parent) = path.parent() {
        fs.create_dir_all(parent)?;
    }
    let out = fs.open(path, OpenMode::CreateNew)?;
    let size = u64::try_from(CHUNK_BYTES).unwrap_or(u64::MAX);
    for (number, listed) in file.chunks.iter().enumerate() {
        let source = sources
            .iter_mut()
            .find(|s| s.holds(number))
            .ok_or_else(|| damaged(&file.within, &format!("no backup of its chain holds its chunk {number}")))?;
        let bytes = source.take(target, number, reading)?;
        if chunk_digest(&bytes) != listed.digest {
            return Err(damaged(&file.within, &format!("its chunk {number} is not the one its manifest lists")));
        }
        out.write_all_at(&bytes, u64::try_from(number).unwrap_or(u64::MAX).saturating_mul(size))?;
    }
    out.set_size(file.length)?;
    out.sync(Durability::Full)?;
    Ok(file.length)
}
