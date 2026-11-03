//! \[EXTENSION\] State transfer, an extension point (RULES.md D57,
//! R-SAFE-3): how an export of a database crosses to a replica the log
//! alone can no longer bring up to date ([VRR §5.2]: "Should a gap occur,
//! the slow replica first brings itself almost up to date using
//! application state … to get to a recent checkpoint, and then completes
//! the job by obtaining the log forward from the point"). Over the group's
//! own transport, in frames: the replica fetching asks, a request at a
//! time, and the replica serving answers each out of the export, so the
//! transport's queues never hold more than one answer; a request lost or
//! answered late is asked again by the fetcher's ticks. The frames are
//! told apart from the consensus's by their first byte (`REQUEST`,
//! `ANSWER`), which the driver reads before either sees them. Each
//! answer names the fetch it answers and the export it is of, the driver
//! numbering each export it serves: an answer of another export, taken
//! as the export served was let go for want of requests and another taken,
//! restarts the fetch, whose pieces must be of one export.
//!
//! Built in: [`WholeFiles`], the export's image and files whole, in
//! pieces of `PIECE` bytes, each file staged by the fetcher in a directory
//! of its own (the database's `transfer/`, cleared as it opens) until the
//! whole export is there; and [`PageDifferences`] (`transfer/pages.rs`),
//! which takes of each file only the blocks that differ from the
//! fetcher's own, as [VRR §5.1] suggests, the node's transfer.

mod pages;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use turborel::bytes::{Put, Reader};
use turborel::db::{ExportedFile, StateSource};
use turborel::disk::io::{DiskFile, Durability, FileSystem, OpenMode};
use turborel::error::{Code, Result, SqlError};
use turborel::files::FileOwner;

pub use self::pages::{BLOCK, PageDifferences};
use crate::ReplicaId;

/// \[EXTENSION\] The first byte of a transfer's request frame.
pub const REQUEST: u8 = 0xF0;
/// \[EXTENSION\] The first byte of a transfer's answer frame.
pub const ANSWER: u8 = 0xF1;

/// \[EXTENSION\] Moves an export of a database between replicas, never
/// decoding it: the fetcher's side ([`Fetching`]) and the server's.
///
/// Contract (for the contract tests of every implementation):
/// * A fetch begun gives requests (`Fetching::request`), each answered by
///   `serve` out of the source, and takes the answers (`Fetching::take`)
///   until it is done; what it then gives (`Fetching::fetched`) has the
///   same image, the same files and the same bytes of each as the source.
/// * A request is answered with one frame, or none if it is no request
///   of this transfer; a request lost, or an answer lost, damaged or of
///   another fetch, is asked again (`Fetching::request` after `Fetching::waited`),
///   never installed; an answer of another export than the fetch began
///   with restarts the fetch.
/// * Frames begin with `REQUEST` or `ANSWER`.
pub trait StateTransfer: Send + Sync {
    /// Its name, for the operator.
    fn name(&self) -> &'static str;
    /// Begins fetching an export from `from`.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if where it stages the export cannot be made.
    fn begin(&self, from: ReplicaId) -> Result<Box<dyn Fetching>>;
    /// Answers `request`, a frame from the first of `to_export`, out of
    /// `source`, the export the driver numbered the second: the answer to
    /// send, or none if the frame is no request of this transfer.
    ///
    /// # Errors
    ///
    /// The source's errors reading.
    fn serve(&self, source: &mut dyn StateSource, to_export: (ReplicaId, u64), request: &[u8]) -> Result<Option<Vec<u8>>>;
}

/// \[EXTENSION\] A fetch of an export under way.
pub trait Fetching: Send {
    /// The next request to send, if one is due: the first, or the one
    /// waited on again.
    fn request(&mut self) -> Option<Vec<u8>>;
    /// A tick passed waiting for an answer: the request is due again once
    /// enough have.
    fn waited(&mut self);
    /// An answer frame arrived: whether the fetch is done.
    ///
    /// # Errors
    ///
    /// SQLCODE -919 if the frame is malformed; -915 if the export cannot be
    /// staged.
    fn take(&mut self, frame: &[u8]) -> Result<bool>;
    /// The export fetched, once done.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it is not done.
    fn fetched(self: Box<Self>) -> Result<Box<dyn StateSource>>;
}

/// \[DESIGN\] The bytes of a file an answer carries at most.
pub const PIECE: usize = 1 << 20;
/// \[DESIGN\] The ticks a fetch waits for an answer before it asks again.
const ASK_AGAIN_TICKS: u64 = 5;
/// What a frame is called in errors.
const FRAME: &str = "state transfer frame";
/// A request for the manifest; or for a piece of a file.
const MANIFEST: u8 = 1;
const PIECE_OF: u8 = 2;

/// The manifest, as an answer carries it: the image, then each file's
/// path within the database's directory, length and owner.
fn put_manifest(out: &mut Vec<u8>, source: &dyn StateSource) {
    out.put_bytes(source.image());
    out.put_u32(u32::try_from(source.files().len()).unwrap_or(u32::MAX));
    for listed in source.files() {
        out.put_str(&listed.within.to_string_lossy().replace('\\', "/"));
        out.put_u64(listed.length);
        out.put_str(&owner_name(&listed.owner));
    }
}

/// The manifest read from an answer: the image, and the files, each to be
/// staged as `0`, `1`, … in `staging`.
fn manifest_read(r: &mut Reader<'_>, staging: &Path) -> Result<(Vec<u8>, Vec<ExportedFile>)> {
    let image = r.bytes()?.to_vec();
    let count = r.u32()?;
    let mut files = Vec::new();
    for n in 0..count {
        let (within, length, owner) = (PathBuf::from(r.str()?), r.u64()?, r.str()?);
        let owner = if owner.is_empty() { FileOwner::Database } else { FileOwner::Plugin(owner) };
        files.push(ExportedFile { path: staging.join(n.to_string()), within, length, owner });
    }
    r.finish()?;
    Ok((image, files))
}

/// \[EXTENSION\] The built-in transfer: whole files, in pieces, staged in
/// `dir` of `fs` by the fetcher.
pub struct WholeFiles {
    fs: Arc<dyn FileSystem>,
    dir: PathBuf,
    /// The fetches begun, numbering each.
    begun: AtomicU64,
}

impl WholeFiles {
    /// A transfer staging what it fetches in `dir` of `fs`.
    pub fn new(fs: Arc<dyn FileSystem>, dir: PathBuf) -> WholeFiles {
        WholeFiles { fs, dir, begun: AtomicU64::new(0) }
    }
}

/// A number no fetch or export of this process, or of one before it in
/// this replica's place, has had: the clock's nanoseconds, to add a count
/// to.
pub(crate) fn stamp() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| u64::try_from(since.as_nanos()).unwrap_or(u64::MAX))
}

/// A request, as a frame: the tag, the fetch's number, what for.
fn request(fetch: u64, what: u8, file: u32, at: u64) -> Vec<u8> {
    let mut out = vec![REQUEST];
    out.put_u64(fetch);
    out.put_u8(what);
    out.put_u32(file);
    out.put_u64(at);
    out
}

impl StateTransfer for WholeFiles {
    fn name(&self) -> &'static str {
        "whole files"
    }

    fn begin(&self, _from: ReplicaId) -> Result<Box<dyn Fetching>> {
        self.fs.create_dir_all(&self.dir)?;
        for stale in self.fs.read_dir(&self.dir)? {
            self.fs.remove(&stale)?;
        }
        let number = stamp().wrapping_add(self.begun.fetch_add(1, Ordering::Relaxed));
        Ok(Box::new(Fetch {
            fs: Arc::clone(&self.fs),
            dir: self.dir.clone(),
            number,
            export: None,
            image: None,
            files: Vec::new(),
            at: 0,
            done: false,
            waited: ASK_AGAIN_TICKS,
            open: None,
        }))
    }

    fn serve(&self, source: &mut dyn StateSource, (_, export): (ReplicaId, u64), frame: &[u8]) -> Result<Option<Vec<u8>>> {
        let mut r = Reader::received(frame, FRAME);
        if r.u8()? != REQUEST {
            return Ok(None);
        }
        let (fetch, what, file, at) = (r.u64()?, r.u8()?, r.u32()?, r.u64()?);
        r.finish()?;
        let mut out = vec![ANSWER];
        out.put_u64(fetch);
        out.put_u64(export);
        out.put_u8(what);
        match what {
            MANIFEST => put_manifest(&mut out, source),
            PIECE_OF => {
                let index = usize::try_from(file).unwrap_or(usize::MAX);
                let length = source.files().get(index).map_or(0, |f| f.length);
                let wanted = usize::try_from(length.saturating_sub(at)).unwrap_or(usize::MAX).min(PIECE);
                let mut piece = vec![0; wanted];
                let read = source.read(index, at, &mut piece)?;
                piece.truncate(read);
                out.put_u32(file);
                out.put_u64(at);
                out.put_bytes(&piece);
            }
            _ => return Ok(None),
        }
        Ok(Some(out))
    }
}

/// A plug-in's name, as an owner is carried: empty for the database's own.
fn owner_name(owner: &FileOwner) -> String {
    match owner {
        FileOwner::Plugin(name) => name.clone(),
        FileOwner::Database | FileOwner::Log | FileOwner::Spill | FileOwner::Server | FileOwner::Unknown => String::new(),
    }
}

/// A fetch of whole files under way: the manifest first, then each file
/// piece by piece, staged as `0`, `1`, … in the directory.
struct Fetch {
    fs: Arc<dyn FileSystem>,
    dir: PathBuf,
    /// Its number, which its requests carry and its answers echo.
    number: u64,
    /// The export the manifest taken is of.
    export: Option<u64>,
    image: Option<Vec<u8>>,
    files: Vec<ExportedFile>,
    /// How far the file being fetched is staged.
    at: u64,
    done: bool,
    waited: u64,
    /// The staged file being written.
    open: Option<(usize, Box<dyn DiskFile>)>,
}

impl Fetching for Fetch {
    fn request(&mut self) -> Option<Vec<u8>> {
        if self.done || self.waited < ASK_AGAIN_TICKS {
            return None;
        }
        self.waited = 0;
        if self.image.is_none() {
            return Some(request(self.number, MANIFEST, 0, 0));
        }
        let file = self.open.as_ref().map_or(0, |(i, _)| *i);
        Some(request(self.number, PIECE_OF, u32::try_from(file).unwrap_or(u32::MAX), self.at))
    }

    fn waited(&mut self) {
        self.waited = self.waited.saturating_add(1);
    }

    fn take(&mut self, frame: &[u8]) -> Result<bool> {
        let mut r = Reader::received(frame, FRAME);
        if r.u8()? != ANSWER || r.u64()? != self.number {
            return Ok(self.done);
        }
        let export = r.u64()?;
        match r.u8()? {
            MANIFEST if self.image.is_none() => {
                self.export = Some(export);
                let (image, files) = manifest_read(&mut r, &self.dir)?;
                self.image = Some(image);
                self.files = files;
                self.open_next(0)?;
            }
            PIECE_OF if self.image.is_some() && self.export != Some(export) => self.restart()?,
            PIECE_OF if self.image.is_some() => {
                let (file, at) = (usize::try_from(r.u32()?).unwrap_or(usize::MAX), r.u64()?);
                let piece = r.bytes()?;
                r.finish()?;
                let Some((open, written)) = self.open.as_ref().filter(|(o, _)| *o == file && at == self.at) else { return Ok(false) };
                written.write_all_at(piece, at)?;
                self.at = at.saturating_add(u64::try_from(piece.len()).unwrap_or(u64::MAX));
                if self.at >= self.files.get(*open).map_or(0, |f| f.length) || piece.is_empty() {
                    written.sync(Durability::Full)?;
                    let next = open.saturating_add(1);
                    self.open = None;
                    self.open_next(next)?;
                }
            }
            MANIFEST | PIECE_OF => {}
            other => return Err(r.corrupt(&format!("{other} is the tag of no answer"))),
        }
        self.waited = ASK_AGAIN_TICKS;
        Ok(self.done)
    }

    fn fetched(self: Box<Self>) -> Result<Box<dyn StateSource>> {
        if !self.done {
            return Err(SqlError::new(Code::Api, "[EXTENSION] RULES.md D57: the state transfer is not fetched yet"));
        }
        Ok(Box::new(Staged::new(self.fs, self.image.unwrap_or_default(), self.files)))
    }
}

impl Fetch {
    /// The fetch begun again, from the manifest: the export served changed
    /// under it, and its pieces must be of one.
    fn restart(&mut self) -> Result<()> {
        self.open = None;
        // Those staged so far: a file the fetch had not begun has none.
        for staged in self.files.drain(..) {
            if self.fs.exists(&staged.path)? {
                self.fs.remove(&staged.path)?;
            }
        }
        self.export = None;
        self.image = None;
        self.at = 0;
        Ok(())
    }

    /// Opens the staged file `index` for writing, or ends the fetch once
    /// every file is staged.
    fn open_next(&mut self, index: usize) -> Result<()> {
        if index >= self.files.len() {
            self.done = true;
            return Ok(());
        }
        let path = self.files.get(index).map(|f| f.path.clone()).unwrap_or_default();
        let written = self.fs.open(&path, OpenMode::CreateNew)?;
        self.at = 0;
        self.open = Some((index, written));
        Ok(())
    }
}

/// An export staged: its image, and its files where the fetch put them.
struct Staged {
    fs: Arc<dyn FileSystem>,
    image: Vec<u8>,
    files: Vec<ExportedFile>,
    open: BTreeMap<usize, Box<dyn DiskFile>>,
}

impl Staged {
    /// The export of `image` whose files `files` lists, staged in `fs`.
    fn new(fs: Arc<dyn FileSystem>, image: Vec<u8>, files: Vec<ExportedFile>) -> Staged {
        Staged { fs, image, files, open: BTreeMap::new() }
    }
}

impl StateSource for Staged {
    fn image(&self) -> &[u8] {
        &self.image
    }

    fn files(&self) -> &[ExportedFile] {
        &self.files
    }

    fn read(&mut self, file: usize, at: u64, into: &mut [u8]) -> Result<usize> {
        let path = self
            .files
            .get(file)
            .map(|f| f.path.as_path())
            .ok_or_else(|| SqlError::new(Code::Api, format!("[EXTENSION] RULES.md D57: the export has no file {file}")))?;
        if !self.open.contains_key(&file) {
            self.open.insert(file, self.fs.open(path, OpenMode::ReadOnly)?);
        }
        self.open.get(&file).map_or(Ok(0), |opened| opened.read_at(into, at))
    }
}

/// Where a transfer stages what it fetches: `dir`'s `transfer/`.
pub fn staging_dir(dir: &Path) -> PathBuf {
    dir.join(turborel::files::TRANSFER)
}
