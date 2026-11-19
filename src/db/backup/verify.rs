//! \[EXTENSION\] A backup checked without restoring it (RULES.md D161):
//! every object of it and of each backup it follows read whole, each
//! block against its digest and through its encodings, which authenticate
//! what they encrypt; each chunk against its manifest's digest, length and
//! place; its image decoded; its records in order, within its positions,
//! as many as its manifest says. Nothing is written.

use std::sync::Arc;

use super::chain::chain_of;
use crate::backup::manifest::{Manifest, ManifestFile, chunk_digest};
use crate::backup::object::{ObjectIn, Reading};
use crate::backup::{BackupTarget, CHUNK_BYTES, FILES, IMAGE, LOG, backup_prefix, damaged};
use crate::bytes::Reader;
use crate::db::Database;
use crate::db::image::Image;
use crate::error::Result;
use crate::log::Lsn;

/// \[EXTENSION\] What a verification read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Verified {
    /// The backups of the chain.
    pub backups: u64,
    /// The chunks read.
    pub chunks: u64,
    /// The records read.
    pub records: u64,
}

/// The bytes chunk `number` of a file of `length` bytes has.
fn chunk_len(length: u64, number: usize) -> u64 {
    let size = u64::try_from(CHUNK_BYTES).unwrap_or(u64::MAX);
    let start = u64::try_from(number).unwrap_or(u64::MAX).saturating_mul(size);
    length.saturating_sub(start).min(size)
}

/// A chunk's block: its number, then its bytes.
pub(super) fn chunk(payload: &[u8], object: &str) -> Result<(usize, Vec<u8>)> {
    let mut r = Reader::new(payload, "a backup's chunk");
    let number = r.u64().map_err(|e| damaged(object, &e.msg))?;
    let number = usize::try_from(number).map_err(|_| damaged(object, "a chunk's number is out of range"))?;
    Ok((number, r.rest().to_vec()))
}

/// Checks the chunks of `file` in backup `prefix`: the chunks read.
fn verify_file(target: &Arc<dyn BackupTarget>, prefix: &str, file: &ManifestFile, reading: &Reading) -> Result<u64> {
    let name = format!("{prefix}{FILES}{}", file.within);
    let mut object = ObjectIn::open(target, &name, reading)?;
    let mut expected = file.chunks.iter().enumerate().filter(|(_, c)| c.present).map(|(n, _)| n);
    let mut read = 0_u64;
    while let Some(payload) = object.next()? {
        let (number, bytes) = chunk(&payload, &name)?;
        if expected.next() != Some(number) {
            return Err(damaged(&name, &format!("its chunk {number} is not the one its manifest lists next")));
        }
        let listed = file.chunks.get(number).map(|c| c.digest);
        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if !crate::backup::decide::chunk_matches((listed, chunk_digest(&bytes)), (length, chunk_len(file.length, number))) {
            return Err(damaged(&name, &format!("its chunk {number} is not the chunk its manifest lists")));
        }
        read = read.saturating_add(1);
    }
    if expected.next().is_some() {
        return Err(damaged(&name, "it ends before the chunks its manifest lists"));
    }
    Ok(read)
}

/// Checks the records of backup `m`: how many there are.
fn verify_records(target: &Arc<dyn BackupTarget>, m: &Manifest, reading: &Reading) -> Result<u64> {
    let name = format!("{}{LOG}", backup_prefix(&m.id));
    let mut object = ObjectIn::open(target, &name, reading)?;
    let (mut last, mut count) = (None::<Lsn>, 0_u64);
    while let Some(payload) = object.next()? {
        let mut r = Reader::new(&payload, "a backup's record");
        let at = Lsn(r.u64().map_err(|e| damaged(&name, &e.msg))?);
        if !crate::backup::decide::record_in_order(last, at, (m.from, m.stop)) {
            return Err(damaged(&name, &format!("its record at {} is out of order or of its positions", at.0)));
        }
        (last, count) = (Some(at), count.saturating_add(1));
    }
    if count != m.records {
        return Err(damaged(&name, &format!("it holds {count} records, and its manifest says {}", m.records)));
    }
    Ok(count)
}

impl Database {
    /// \[EXTENSION\] Checks backup `id` (the newest if empty) of the target
    /// at `location`, and each it follows, without restoring it: see the
    /// module's documentation.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 (SQLSTATE TBB01) naming the first object found
    /// damaged; -910 if there is no such backup, or an object names a
    /// function, encoding or key the database lacks; the target's.
    pub fn verify_backup(&self, location: &str, id: &str) -> Result<Verified> {
        let target = self.open_target(location)?;
        let (_, reading) = self.object_formats()?;
        let types = self.snapshot()?.types.clone();
        let mut verified = Verified::default();
        for m in chain_of(&target, id, &reading)? {
            let prefix = backup_prefix(&m.id);
            for file in &m.files {
                verified.chunks = verified.chunks.saturating_add(verify_file(&target, &prefix, file, &reading)?);
            }
            let image = format!("{prefix}{IMAGE}");
            Image::decode(&ObjectIn::only(&target, &image, &reading)?, &types).map_err(|e| damaged(&image, &e.msg))?;
            verified.records = verified.records.saturating_add(verify_records(&target, &m, &reading)?);
            verified.backups = verified.backups.saturating_add(1);
        }
        Ok(verified)
    }
}
