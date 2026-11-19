//! \[EXTENSION\] A state transfer served by a host plug-in (kind 34,
//! RULES.md D272). The program makes and reads the frames; the host keeps
//! the export and the staging. Serving, the export is told the program
//! once (EXPORT) and each request answered by SERVE, which asks for the
//! pieces of the export's files it needs, one a call. Fetching, the
//! program's requests are sent and its answers taken (BEGIN, TAKE), and
//! once done the export is copied out of the program in chunks (FETCHED,
//! CHUNK), each checked by its digest and staged in the database's
//! `transfer/` before the database takes any of it. A program that ends
//! mid-chunk stops the replica, which, started again, clears `transfer/`
//! and fetches anew, so nothing half fetched is installed.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use turborel::bytes::{Put, Reader};
use turborel::db::{ExportedFile, StateSource};
use turborel::disk::io::{Durability, FileSystem, OpenMode};
use turborel::error::{Code, Result, SqlError};
use turborel::files::FileOwner;
use turborel::plugin::errors::violation;
use turborel::plugin::payload::replication::{
    Manifest, Served, TRANSFER_BEGIN, TRANSFER_CHUNK, TRANSFER_EXPORT, TRANSFER_FETCHED, TRANSFER_SERVE, TRANSFER_TAKE, chunk_digest,
    manifest, optional, put_given, put_manifest, served,
};

use super::{Pipe, Stopping};
use crate::transfer::{ANSWER, ASK_AGAIN_TICKS, Fetching, PIECE, REQUEST, StateTransfer, Staged, owner_name};
use crate::ReplicaId;

/// \[DESIGN\] The pieces SERVE may ask for one answer at most: a program
/// asking more is taken as looping, and breaks the protocol.
const PIECES_AN_ANSWER: usize = 64;

/// \[EXTENSION\] A host plug-in's state transfer, staging what it fetches
/// in `dir` of `fs`.
pub struct PluginTransfer {
    pipe: Arc<Stopping>,
    fs: Arc<dyn FileSystem>,
    dir: PathBuf,
    /// The number of the export last told the program.
    told: Mutex<Option<u64>>,
}

impl PluginTransfer {
    /// The transfer over `pipe`, staging in `dir` of `fs`.
    pub fn new(pipe: Box<dyn Pipe>, fs: Arc<dyn FileSystem>, dir: PathBuf) -> PluginTransfer {
        PluginTransfer { pipe: Arc::new(Stopping::new(pipe, "state transfer")), fs, dir, told: Mutex::new(None) }
    }

    /// Its failure, if it failed.
    pub fn failed(&self) -> Option<SqlError> {
        self.pipe.failed()
    }

    /// `source` told the program as export `export`, unless it was last.
    fn tell(&self, source: &dyn StateSource, export: u64) -> Result<()> {
        let mut told = self.told.lock().unwrap_or_else(PoisonError::into_inner);
        if *told == Some(export) {
            return Ok(());
        }
        let files = source.files().iter().map(|f| (f.within.to_string_lossy().replace('\\', "/"), f.length, owner_name(&f.owner)));
        let mut payload = Vec::new();
        payload.put_u64(export);
        put_manifest(&mut payload, &Manifest { image: source.image().to_vec(), files: files.collect() });
        self.pipe.ask(TRANSFER_EXPORT, payload, |_| Ok(()))?;
        *told = Some(export);
        Ok(())
    }

    /// `frame`, a frame of the program's, if it begins with `first`.
    fn framed(&self, frame: Vec<u8>, first: u8) -> Result<Vec<u8>> {
        if frame.first() == Some(&first) {
            return Ok(frame);
        }
        Err(self.pipe.fail(&violation(&format!("its state transfer made a frame not beginning with {first}"))))
    }
}

impl StateTransfer for PluginTransfer {
    fn name(&self) -> &'static str {
        "plug-in"
    }

    fn begin(&self, from: ReplicaId) -> Result<Box<dyn Fetching>> {
        clear(&*self.fs, &self.dir)?;
        let mut payload = Vec::new();
        payload.put_u32(from.0);
        let (fetch, first) = self.pipe.ask(TRANSFER_BEGIN, payload, |r| Ok((r.u64()?, optional(r)?)))?;
        let first = first.map(|f| self.framed(f, REQUEST)).transpose()?;
        let (fs, dir, pipe) = (Arc::clone(&self.fs), self.dir.clone(), Arc::clone(&self.pipe));
        Ok(Box::new(PluginFetch { pipe, fs, dir, fetch, last: first.clone(), due: first, waited: 0, done: false }))
    }

    fn serve(&self, source: &mut dyn StateSource, (to, export): (ReplicaId, u64), request: &[u8]) -> Result<Option<Vec<u8>>> {
        self.tell(source, export)?;
        let mut given: Option<(u32, u64, Vec<u8>)> = None;
        for _ in 0..PIECES_AN_ANSWER {
            let mut payload = Vec::new();
            payload.put_u32(to.0);
            payload.put_u64(export);
            payload.put_bytes(request);
            put_given(&mut payload, given.as_ref().map(|(f, a, b)| (*f, *a, b.as_slice())));
            match self.pipe.ask(TRANSFER_SERVE, payload, served)? {
                Served::Nothing => return Ok(None),
                Served::Answer(answer) => return self.framed(answer, ANSWER).map(Some),
                Served::Need(file, at, length) => given = Some((file, at, self.piece(source, (file, at), length)?)),
            }
        }
        Err(self.pipe.fail(&violation(&format!("its state transfer asked more than {PIECES_AN_ANSWER} pieces for one answer"))))
    }
}

impl PluginTransfer {
    /// The piece of `source`'s file `file` at `at`, of `length` bytes at
    /// most, and at most `PIECE`.
    fn piece(&self, source: &mut dyn StateSource, (file, at): (u32, u64), length: u32) -> Result<Vec<u8>> {
        let index = usize::try_from(file).unwrap_or(usize::MAX);
        if index >= source.files().len() {
            return Err(self.pipe.fail(&violation(&format!("its state transfer asked for file {file} of an export of fewer"))));
        }
        let mut piece = vec![0; usize::try_from(length).unwrap_or(usize::MAX).min(PIECE)];
        let read = source.read(index, at, &mut piece)?;
        piece.truncate(read);
        Ok(piece)
    }
}

/// Removes what `dir` stages, made if it is not there.
fn clear(fs: &dyn FileSystem, dir: &Path) -> Result<()> {
    fs.create_dir_all(dir)?;
    for stale in fs.read_dir(dir)? {
        fs.remove(&stale)?;
    }
    Ok(())
}

/// A fetch of a plug-in's under way: the requests it gives, sent again
/// after `ASK_AGAIN_TICKS` ticks without an answer.
struct PluginFetch {
    pipe: Arc<Stopping>,
    fs: Arc<dyn FileSystem>,
    dir: PathBuf,
    fetch: u64,
    /// The request last given, asked again once enough ticks pass.
    last: Option<Vec<u8>>,
    /// The request due to be sent.
    due: Option<Vec<u8>>,
    waited: u64,
    done: bool,
}

impl Fetching for PluginFetch {
    fn request(&mut self) -> Option<Vec<u8>> {
        self.due.take()
    }

    fn waited(&mut self) {
        if self.done {
            return;
        }
        self.waited = self.waited.saturating_add(1);
        if self.waited >= ASK_AGAIN_TICKS {
            self.waited = 0;
            self.due.clone_from(&self.last);
        }
    }

    fn take(&mut self, frame: &[u8]) -> Result<bool> {
        if frame.first() != Some(&ANSWER) || self.done {
            return Ok(self.done);
        }
        let mut payload = Vec::new();
        payload.put_u64(self.fetch);
        payload.put_bytes(frame);
        let (done, next) = self.pipe.ask(TRANSFER_TAKE, payload, |r| Ok((r.u8()? != 0, optional(r)?)))?;
        if let Some(next) = next {
            if next.first() != Some(&REQUEST) {
                return Err(self.pipe.fail(&violation("its state transfer made a request not beginning with its tag")));
            }
            self.last = Some(next.clone());
            self.due = Some(next);
            self.waited = 0;
        }
        self.done = done;
        Ok(done)
    }

    fn fetched(self: Box<Self>) -> Result<Box<dyn StateSource>> {
        if !self.done {
            return Err(SqlError::new(Code::Api, "[EXTENSION] RULES.md D57: the state transfer is not fetched yet"));
        }
        let mut payload = Vec::new();
        payload.put_u64(self.fetch);
        let got = self.pipe.ask(TRANSFER_FETCHED, payload, manifest)?;
        let mut files = Vec::new();
        for (index, (within, length, owner)) in got.files.into_iter().enumerate() {
            let path = self.dir.join(index.to_string());
            self.stage(index, &path, length)?;
            let owner = if owner.is_empty() { FileOwner::Database } else { FileOwner::Plugin(owner) };
            files.push(ExportedFile { path, within: PathBuf::from(within), length, owner });
        }
        Ok(Box::new(Staged::new(Arc::clone(&self.fs), got.image, files)))
    }
}

impl PluginFetch {
    /// The export's file `index`, `length` bytes, copied out of the
    /// program chunk by chunk to `path`, each checked by its digest, and
    /// made durable.
    fn stage(&self, index: usize, path: &Path, length: u64) -> Result<()> {
        let written = self.fs.open(path, OpenMode::CreateNew)?;
        let mut at = 0_u64;
        while at < length {
            let mut payload = Vec::new();
            payload.put_u64(self.fetch);
            payload.put_u32(u32::try_from(index).unwrap_or(u32::MAX));
            payload.put_u64(at);
            payload.put_u32(u32::try_from(PIECE).unwrap_or(u32::MAX));
            let (chunk, digest) = self.pipe.ask(TRANSFER_CHUNK, payload, chunk_read)?;
            if chunk.is_empty() || chunk_digest(&chunk) != digest {
                let why = format!("its state transfer gave a chunk of file {index} at {at} empty or not of its digest");
                return Err(self.pipe.fail(&violation(&why)));
            }
            written.write_all_at(&chunk, at)?;
            at = at.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
        }
        written.sync(Durability::Full)
    }
}

/// CHUNK's answer: the bytes and their digest.
fn chunk_read(r: &mut Reader<'_>) -> Result<(Vec<u8>, u64)> {
    Ok((r.bytes()?.to_vec(), r.u64()?))
}
