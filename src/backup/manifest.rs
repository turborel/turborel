//! \[EXTENSION\] A backup's manifest (RULES.md D161, D162), format version
//! 1: what the backup is, where in the log it stands, and each file's
//! length and chunks, each a digest and whether this backup holds it. It
//! is written last, so a backup without one was never finished.
//!
//! ```text
//! manifest:  format u8 (MANIFEST_FORMAT) | id | kind u8 (1 full,
//!            2 incremental) | parent (empty for a full one) |
//!            timeline u32 | from u64 | through u64 | stop u64 |
//!            started i128 | ended i128 | label | records u64 | bytes u64 |
//!            files u32, each: within | owner | length u64 |
//!              chunks u32, each: digest (CHUNK_DIGEST bytes) | present u8
//! ```
//!
//! `from` is the first record the backup's log holds, `through` its
//! checkpoint's position, `stop` the position after its last record.
//! Restored, it holds every transaction committed before `stop`. Times
//! are microseconds from 1970-01-01 00:00 UTC. Strings are a length u32
//! and UTF-8 bytes, integers little-endian (`bytes.rs`).

use super::BackupKind;
use crate::bytes::{Put, Reader};
use crate::error::{Code, Result, SqlError};
use crate::hash::murmur::murmur3_x64_128;
use crate::log::Lsn;

/// \[EXTENSION\] The format of a manifest this code writes and reads.
pub const MANIFEST_FORMAT: u8 = 1;

/// \[EXTENSION\] The bytes of a chunk's digest: MurmurHash3 x64 128, so two
/// different chunks share one by accident with odds of 2^-128 (RULES.md
/// D162).
pub const CHUNK_DIGEST: usize = 16;

/// \[EXTENSION\] The seed of the chunks' digests: none.
const SEED: u32 = 0;

/// The digest of chunk `bytes`.
pub fn chunk_digest(bytes: &[u8]) -> [u8; CHUNK_DIGEST] {
    let (high, low) = murmur3_x64_128(bytes, SEED);
    let mut out = [0; CHUNK_DIGEST];
    for (into, byte) in out.iter_mut().zip(high.to_le_bytes().into_iter().chain(low.to_le_bytes())) {
        *into = byte;
    }
    out
}

/// \[EXTENSION\] A chunk of a file, as a manifest lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    /// Its digest (`chunk_digest`).
    pub digest: [u8; CHUNK_DIGEST],
    /// Whether this backup holds it; if not, a backup it follows does.
    pub present: bool,
}

/// \[EXTENSION\] A file of a backup, as its manifest lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFile {
    /// Where it is within the database's directory, its parts separated
    /// by `/`.
    pub within: String,
    /// Whose it is, as INFORMATION_SCHEMA.FILES names it.
    pub owner: String,
    /// Its length in bytes.
    pub length: u64,
    /// Its chunks, in order.
    pub chunks: Vec<Chunk>,
}

/// \[EXTENSION\] A backup's manifest: see the module's documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// The backup's id.
    pub id: String,
    /// Full or incremental.
    pub kind: BackupKind,
    /// The id of the backup it follows, empty if full.
    pub parent: String,
    /// The timeline of the database it was taken of (RULES.md D165).
    pub timeline: u32,
    /// The first record its log holds.
    pub from: Lsn,
    /// Its checkpoint's position.
    pub through: Lsn,
    /// The position after its last record.
    pub stop: Lsn,
    /// When it began.
    pub started: i64,
    /// When it ended.
    pub ended: i64,
    /// Its label, given as it was taken.
    pub label: String,
    /// The records its log holds.
    pub records: u64,
    /// The bytes of its objects but the manifest.
    pub bytes: u64,
    /// Its files.
    pub files: Vec<ManifestFile>,
}

/// A count as a manifest writes it.
fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl Manifest {
    /// It encoded.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![MANIFEST_FORMAT];
        out.put_str(&self.id);
        out.put_u8(self.kind.code());
        out.put_str(&self.parent);
        out.put_u32(self.timeline);
        for position in [self.from, self.through, self.stop] {
            out.put_u64(position.0);
        }
        out.put_i128(i128::from(self.started));
        out.put_i128(i128::from(self.ended));
        out.put_str(&self.label);
        out.put_u64(self.records);
        out.put_u64(self.bytes);
        out.put_u32(count(self.files.len()));
        for file in &self.files {
            out.put_str(&file.within);
            out.put_str(&file.owner);
            out.put_u64(file.length);
            out.put_u32(count(file.chunks.len()));
            for chunk in &file.chunks {
                out.extend_from_slice(&chunk.digest);
                out.put_u8(u8::from(chunk.present));
            }
        }
        out
    }

    /// The manifest `bytes` encode.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if they are malformed, or of another format.
    pub fn decode(bytes: &[u8]) -> Result<Manifest> {
        let mut r = Reader::new(bytes, "a backup's manifest");
        let format = r.u8()?;
        if format != MANIFEST_FORMAT {
            return Err(SqlError::new(
                Code::Corrupt,
                format!("[EXTENSION] RULES.md D161: a manifest of format {format}, and only {MANIFEST_FORMAT} is known"),
            ));
        }
        let id = r.str()?;
        let kind = BackupKind::of(r.u8()?).ok_or_else(|| r.corrupt("its kind is unknown"))?;
        let (parent, timeline) = (r.str()?, r.u32()?);
        let (from, through, stop) = (Lsn(r.u64()?), Lsn(r.u64()?), Lsn(r.u64()?));
        let started = i64::try_from(r.i128()?).map_err(|_| r.corrupt("its start is out of range"))?;
        let ended = i64::try_from(r.i128()?).map_err(|_| r.corrupt("its end is out of range"))?;
        let (label, records, bytes_written) = (r.str()?, r.u64()?, r.u64()?);
        let mut files = Vec::new();
        for _ in 0..r.u32()? {
            files.push(file(&mut r)?);
        }
        r.finish()?;
        Ok(Manifest { id, kind, parent, timeline, from, through, stop, started, ended, label, records, bytes: bytes_written, files })
    }
}

/// A file of a manifest, read from `r`.
fn file(r: &mut Reader<'_>) -> Result<ManifestFile> {
    let (within, owner, length) = (r.str()?, r.str()?, r.u64()?);
    let mut chunks = Vec::new();
    for _ in 0..r.u32()? {
        let digest = <[u8; CHUNK_DIGEST]>::try_from(r.take(CHUNK_DIGEST)?).map_err(|_| r.corrupt("a chunk's digest is short"))?;
        let present = match r.u8()? {
            0 => false,
            1 => true,
            _ => return Err(r.corrupt("a chunk is neither present nor absent")),
        };
        chunks.push(Chunk { digest, present });
    }
    Ok(ManifestFile { within, owner, length, chunks })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] RULES.md D161, the manifest: version 1
    /// round trips, and another version or a bad presence flag is refused.
    #[test]
    fn manifests_round_trip_and_other_formats_are_refused() {
        let chunk = Chunk { digest: chunk_digest(b"page"), present: true };
        let file = ManifestFile { within: "data".into(), owner: "DATABASE".into(), length: 4, chunks: vec![chunk] };
        let m = Manifest {
            id: "B1".into(),
            kind: BackupKind::Incremental,
            parent: "B0".into(),
            timeline: 2,
            from: Lsn(5),
            through: Lsn(6),
            stop: Lsn(9),
            started: -1,
            ended: 7,
            label: "nightly".into(),
            records: 3,
            bytes: 100,
            files: vec![file],
        };
        let bytes = m.encode();
        assert_eq!(Manifest::decode(&bytes).unwrap(), m);
        let mut other = bytes.clone();
        other[0] = 2;
        assert_eq!(Manifest::decode(&other).err().map(|e| e.code), Some(Code::Corrupt));
        let mut flag = bytes.clone();
        let last = flag.len() - 1;
        flag[last] = 9;
        assert_eq!(Manifest::decode(&flag).err().map(|e| e.code), Some(Code::Corrupt));
        assert_ne!(chunk_digest(b"page"), chunk_digest(b"pagf"));
    }
}
