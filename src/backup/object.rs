//! \[EXTENSION\] A backup's and the archive's objects written and read
//! (RULES.md D161): each a block file of format version 1 as a stream
//! (`disk/block/stream.rs`), uploaded in pieces of `PIECE_BYTES` and read
//! back in ranges. A block that fails its digest, or a decryption that
//! fails to authenticate, makes the object damaged (SQLSTATE TBB01).

use std::path::Path;
use std::sync::Arc;

use super::target::BackupTarget;
use super::{PIECE_BYTES, damaged};
use crate::disk::block::{BlockReader, BlockWriter, STREAM_HEADER};
use crate::disk::encoding::{Encoding, Pipeline};
use crate::error::{Code, Result, SqlError};
use crate::hash::HashFunction;

/// \[EXTENSION\] The formats objects are written in: the database's
/// checksum and encodings, compression first if asked (RULES.md D161).
#[derive(Clone)]
pub struct Writing {
    /// The hash function every digest is made with.
    pub hash: Arc<dyn HashFunction>,
    /// The encodings every block passes through.
    pub pipeline: Pipeline,
}

/// \[EXTENSION\] What objects may be read with: the database's registered
/// hash functions and encodings, its keyring's keys bound in.
#[derive(Clone)]
pub struct Reading {
    /// The hash functions an object may name.
    pub hashes: Vec<Arc<dyn HashFunction>>,
    /// The encodings an object may name.
    pub encodings: Vec<Arc<dyn Encoding>>,
}

/// An object being written: its upload, unfinished until `finish`, and
/// abandoned if dropped before.
pub(crate) struct ObjectOut<'t> {
    target: &'t dyn BackupTarget,
    upload: Option<u64>,
    writer: BlockWriter,
    held: Vec<u8>,
    /// The bytes sent and held.
    bytes: u64,
}

impl<'t> ObjectOut<'t> {
    /// Begins object `name` in `target`, in `writing`'s formats.
    ///
    /// # Errors
    ///
    /// The target's.
    pub(crate) fn begin(target: &'t dyn BackupTarget, name: &str, writing: &Writing) -> Result<ObjectOut<'t>> {
        let upload = target.begin(name)?;
        let writer = BlockWriter::new(Arc::clone(&writing.hash), writing.pipeline.clone());
        let held = writer.header();
        let bytes = u64::try_from(held.len()).unwrap_or(u64::MAX);
        Ok(ObjectOut { target, upload: Some(upload), writer, held, bytes })
    }

    /// Appends a block holding `payload`.
    ///
    /// # Errors
    ///
    /// An encoding's; the target's.
    pub(crate) fn block(&mut self, payload: &[u8]) -> Result<()> {
        let block = self.writer.block(payload)?;
        self.bytes = self.bytes.saturating_add(u64::try_from(block.len()).unwrap_or(u64::MAX));
        self.held.extend_from_slice(&block);
        if self.held.len() >= usize::try_from(PIECE_BYTES).unwrap_or(usize::MAX) {
            self.send()?;
        }
        Ok(())
    }

    /// Sends what is held.
    fn send(&mut self) -> Result<()> {
        let upload = self.upload.ok_or_else(|| super::target::no_upload(0))?;
        self.target.write(upload, &self.held)?;
        self.held.clear();
        Ok(())
    }

    /// Finishes the object, durably: the bytes it holds.
    ///
    /// # Errors
    ///
    /// The target's.
    pub(crate) fn finish(mut self) -> Result<u64> {
        self.send()?;
        let upload = self.upload.take().ok_or_else(|| super::target::no_upload(0))?;
        self.target.finish(upload)?;
        Ok(self.bytes)
    }
}

impl Drop for ObjectOut<'_> {
    /// An object not finished is abandoned, leaving nothing.
    fn drop(&mut self) {
        if let Some(upload) = self.upload.take() {
            // Abandoning is tidying up after a failure already reported,
            // and an upload a target fails to abandon is never an object.
            let _ = self.target.abandon(upload);
        }
    }
}

/// An object being read, block by block.
pub(crate) struct ObjectIn {
    target: Arc<dyn BackupTarget>,
    name: String,
    reader: BlockReader,
    held: Vec<u8>,
    /// Where in `held` the next block begins.
    at: usize,
    /// The object's offset after `held`.
    offset: u64,
    ended: bool,
}

impl ObjectIn {
    /// Opens object `name` of `target`, read with `reading`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 (SQLSTATE TBB01) if its header is damaged; -910 if it
    /// is not there, or names a function, encoding or key not given; the
    /// target's.
    pub(crate) fn open(target: &Arc<dyn BackupTarget>, name: &str, reading: &Reading) -> Result<ObjectIn> {
        let held = target.read(name, 0, PIECE_BYTES)?;
        let header = held.get(..STREAM_HEADER).ok_or_else(|| damaged(name, "it is shorter than a header"))?;
        let reader = BlockReader::new(header, Path::new(name), (&reading.hashes, &reading.encodings)).map_err(|e| marked(name, e))?;
        let offset = u64::try_from(held.len()).unwrap_or(u64::MAX);
        Ok(ObjectIn { target: Arc::clone(target), name: name.to_owned(), reader, held, at: STREAM_HEADER, offset, ended: false })
    }

    /// The next block's payload, or none at the object's end.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 (SQLSTATE TBB01) if a block is damaged or the object
    /// ends within one; the target's.
    pub(crate) fn next(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            let rest = self.held.get(self.at..).unwrap_or_default();
            if let Some((payload, used)) = self.reader.next(rest).map_err(|e| marked(&self.name, e))? {
                self.at = self.at.saturating_add(used);
                return Ok(Some(payload));
            }
            if self.ended {
                return if rest.is_empty() { Ok(None) } else { Err(damaged(&self.name, "it ends within a block")) };
            }
            let more = self.target.read(&self.name, self.offset, PIECE_BYTES)?;
            self.ended = more.is_empty();
            self.offset = self.offset.saturating_add(u64::try_from(more.len()).unwrap_or(u64::MAX));
            self.held.drain(..self.at);
            self.at = 0;
            self.held.extend_from_slice(&more);
        }
    }

    /// The one block object `name` of `target` holds.
    ///
    /// # Errors
    ///
    /// As `open` and `next`; SQLCODE -916 (SQLSTATE TBB01) unless it holds
    /// exactly one block.
    pub(crate) fn only(target: &Arc<dyn BackupTarget>, name: &str, reading: &Reading) -> Result<Vec<u8>> {
        let mut object = ObjectIn::open(target, name, reading)?;
        let first = object.next()?.ok_or_else(|| damaged(name, "it holds no block"))?;
        if object.next()?.is_some() {
            return Err(damaged(name, "it holds more than one block"));
        }
        Ok(first)
    }
}

/// `e`, met reading object `name`: damage, if it is the format's.
fn marked(name: &str, e: SqlError) -> SqlError {
    if e.code == Code::Corrupt { damaged(name, &e.msg) } else { e }
}

/// Writes object `name` of one block, `payload`, to `target`: the bytes
/// it holds.
///
/// # Errors
///
/// As `ObjectOut`'s.
pub(crate) fn write_one(target: &dyn BackupTarget, name: &str, writing: &Writing, payload: &[u8]) -> Result<u64> {
    let mut object = ObjectOut::begin(target, name, writing)?;
    object.block(payload)?;
    object.finish()
}
