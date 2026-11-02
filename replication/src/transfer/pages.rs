//! \[EXTENSION\] State transfer by page differences (RULES.md D57), after
//! [VRR §5.1]'s suggestion of Merkle trees to find what differs: a
//! fetcher that holds a database already, behind, takes from the export
//! only the blocks of each file that differ from its own. The manifest
//! first, as with whole files; then, for each file, the export's summary,
//! the hash of each block of `BLOCK` bytes, in chunks of `SUMMARY`
//! hashes; a block of the fetcher's own file hashing the same is copied
//! into the staged file, the rest are asked for, in runs of up to `PIECE`
//! bytes; a file the fetcher lacks is fetched whole. Blocks, not the
//! database's pages: a page's size is the database's to choose, and a
//! block of several pages hashes as fast. Split from transfer.rs
//! (RULES.md R-SIZE-2).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use turborel::bytes::{Put, Reader};
use turborel::db::{ExportedFile, StateSource};
use turborel::disk::io::{DiskFile, Durability, FileSystem, OpenMode};
use turborel::error::{Code, Result, SqlError};

use super::{
    ANSWER, ASK_AGAIN_TICKS, FRAME, Fetching, MANIFEST, PIECE, REQUEST, Staged, StateTransfer, manifest_read, put_manifest, stamp,
};
use crate::ReplicaId;

/// \[DESIGN\] The bytes of a block, hashed as one.
pub const BLOCK: usize = 1 << 16;
/// \[DESIGN\] The hashes a summary answer carries at most.
const SUMMARY: usize = 1024;
/// A request for a summary of a file's blocks from one; or for a run of
/// bytes of a file.
const SUMMARY_OF: u8 = 3;
const RUN_OF: u8 = 4;

/// \[EXTENSION\] The built-in transfer by page differences: the fetcher's
/// database in `dir` of `fs`, its files compared block by block with the
/// export's, the result staged in `dir`'s `transfer/`.
pub struct PageDifferences {
    fs: Arc<dyn FileSystem>,
    dir: PathBuf,
    staging: PathBuf,
    begun: AtomicU64,
}

impl PageDifferences {
    /// A transfer for the database kept in `dir` of `fs`.
    pub fn new(fs: Arc<dyn FileSystem>, dir: &Path) -> PageDifferences {
        PageDifferences { fs, dir: dir.to_path_buf(), staging: super::staging_dir(dir), begun: AtomicU64::new(0) }
    }
}

/// FNV-1a of `bytes`: what a block hashes to.
fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// A request, as a frame: the tag, the fetch's number, what for, of which
/// file, from where, how much.
fn request(fetch: u64, what: u8, file: u32, (at, length): (u64, u32)) -> Vec<u8> {
    let mut out = vec![REQUEST];
    out.put_u64(fetch);
    out.put_u8(what);
    out.put_u32(file);
    out.put_u64(at);
    out.put_u32(length);
    out
}

impl StateTransfer for PageDifferences {
    fn name(&self) -> &'static str {
        "page differences"
    }

    fn begin(&self, _from: ReplicaId) -> Result<Box<dyn Fetching>> {
        self.fs.create_dir_all(&self.staging)?;
        for stale in self.fs.read_dir(&self.staging)? {
            self.fs.remove(&stale)?;
        }
        let number = stamp().wrapping_add(self.begun.fetch_add(1, Ordering::Relaxed));
        Ok(Box::new(Fetch {
            fs: Arc::clone(&self.fs),
            dir: self.dir.clone(),
            staging: self.staging.clone(),
            number,
            export: None,
            image: None,
            files: Vec::new(),
            file: 0,
            phase: Phase::Manifest,
            needed: Vec::new(),
            open: None,
            done: false,
            waited: ASK_AGAIN_TICKS,
        }))
    }

    fn serve(&self, source: &mut dyn StateSource, (_, export): (ReplicaId, u64), frame: &[u8]) -> Result<Option<Vec<u8>>> {
        let mut r = Reader::received(frame, FRAME);
        if r.u8()? != REQUEST {
            return Ok(None);
        }
        let (fetch, what, file, at, length) = (r.u64()?, r.u8()?, r.u32()?, r.u64()?, r.u32()?);
        r.finish()?;
        let mut out = vec![ANSWER];
        out.put_u64(fetch);
        out.put_u64(export);
        out.put_u8(what);
        let index = usize::try_from(file).unwrap_or(usize::MAX);
        match what {
            MANIFEST => put_manifest(&mut out, source),
            SUMMARY_OF => {
                let size = source.files().get(index).map_or(0, |f| f.length);
                let mut block = vec![0; BLOCK];
                let mut hashes = Vec::new();
                let mut from = at;
                while from < size && hashes.len() < SUMMARY {
                    let read = source.read(index, from, &mut block)?;
                    hashes.push(hash(block.get(..read).unwrap_or_default()));
                    from = from.saturating_add(u64::try_from(BLOCK).unwrap_or(u64::MAX));
                }
                out.put_u32(file);
                out.put_u64(at);
                out.put_u32(u32::try_from(hashes.len()).unwrap_or(u32::MAX));
                for h in hashes {
                    out.put_u64(h);
                }
            }
            RUN_OF => {
                let size = source.files().get(index).map_or(0, |f| f.length);
                let wanted = usize::try_from(size.saturating_sub(at))
                    .unwrap_or(usize::MAX)
                    .min(PIECE)
                    .min(usize::try_from(length).unwrap_or(usize::MAX));
                let mut run = vec![0; wanted];
                let read = source.read(index, at, &mut run)?;
                run.truncate(read);
                out.put_u32(file);
                out.put_u64(at);
                out.put_bytes(&run);
            }
            _ => return Ok(None),
        }
        Ok(Some(out))
    }
}

/// Where a fetch stands with the file it is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The manifest asked for.
    Manifest,
    /// The summary asked for, from a position.
    Summary(u64),
    /// The runs the summary found differing fetched: the next from a
    /// position.
    Runs,
}

/// The staged file being written, and the fetcher's own of the same name,
/// if it has one.
type Opened = (Box<dyn DiskFile>, Option<Box<dyn DiskFile>>);

/// A fetch by page differences under way.
struct Fetch {
    fs: Arc<dyn FileSystem>,
    dir: PathBuf,
    staging: PathBuf,
    number: u64,
    export: Option<u64>,
    image: Option<Vec<u8>>,
    files: Vec<ExportedFile>,
    /// The file being fetched.
    file: usize,
    phase: Phase,
    /// The runs of the file still to fetch: each from a position, its
    /// length; the next first.
    needed: Vec<(u64, u32)>,
    /// The staged file being written, and the fetcher's own of the same
    /// name, if it has one.
    open: Option<Opened>,
    done: bool,
    waited: u64,
}

impl Fetching for Fetch {
    fn request(&mut self) -> Option<Vec<u8>> {
        if self.done || self.waited < ASK_AGAIN_TICKS {
            return None;
        }
        self.waited = 0;
        let file = u32::try_from(self.file).unwrap_or(u32::MAX);
        Some(match self.phase {
            Phase::Manifest => request(self.number, MANIFEST, 0, (0, 0)),
            Phase::Summary(from) => request(self.number, SUMMARY_OF, file, (from, 0)),
            Phase::Runs => {
                let (at, length) = self.needed.first().copied().unwrap_or_default();
                request(self.number, RUN_OF, file, (at, length))
            }
        })
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
        let what = r.u8()?;
        if self.image.is_some() && self.export != Some(export) && what != MANIFEST {
            self.restart()?;
        } else if what == MANIFEST && self.phase == Phase::Manifest {
            let (image, files) = manifest_read(&mut r, &self.staging)?;
            self.export = Some(export);
            self.image = Some(image);
            self.files = files;
            self.open_next(0)?;
        } else if what == SUMMARY_OF {
            self.take_summary(&mut r)?;
        } else if what == RUN_OF {
            self.take_run(&mut r)?;
        } else if what != MANIFEST {
            return Err(r.corrupt(&format!("{what} is the tag of no answer")));
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
    /// A summary answered: each block hashing as the fetcher's own is
    /// copied in, the rest asked for in runs; the next summary asked for,
    /// or, the file summarized, its runs.
    fn take_summary(&mut self, r: &mut Reader<'_>) -> Result<()> {
        let (file, at, count) = (usize::try_from(r.u32()?).unwrap_or(usize::MAX), r.u64()?, r.u32()?);
        let Phase::Summary(from) = self.phase else { return Ok(()) };
        if file != self.file || at != from {
            return Ok(());
        }
        let mut hashes = Vec::new();
        for _ in 0..count {
            hashes.push(r.u64()?);
        }
        r.finish()?;
        let size = self.files.get(file).map_or(0, |f| f.length);
        let block = u64::try_from(BLOCK).unwrap_or(u64::MAX);
        let mut position = at;
        for theirs in &hashes {
            let length = u32::try_from(size.saturating_sub(position).min(block)).unwrap_or(u32::MAX);
            if !self.copied_own(position, length, *theirs)? {
                self.need(position, length);
            }
            position = position.saturating_add(block);
        }
        if position < size && !hashes.is_empty() {
            self.phase = Phase::Summary(position);
        } else {
            self.phase = Phase::Runs;
            self.advance_runs()?;
        }
        Ok(())
    }

    /// The block at `position`, `length` long, to fetch: joined to the run
    /// before it if it follows it and the run is short of a piece.
    fn need(&mut self, position: u64, length: u32) {
        if let Some((start, run)) = self.needed.last_mut()
            && start.saturating_add(u64::from(*run)) == position
            && usize::try_from(*run).unwrap_or(usize::MAX) < PIECE
        {
            *run = run.saturating_add(length);
            return;
        }
        self.needed.push((position, length));
    }

    /// Whether the fetcher's own file has the block at `position` hashing
    /// `theirs`, and so copied it into the staged file.
    fn copied_own(&mut self, position: u64, length: u32, theirs: u64) -> Result<bool> {
        let Some((staged, Some(own))) = self.open.as_ref() else { return Ok(false) };
        let mut block = vec![0; usize::try_from(length).unwrap_or(usize::MAX)];
        let read = own.read_at(&mut block, position)?;
        if read != block.len() || hash(&block) != theirs {
            return Ok(false);
        }
        staged.write_all_at(&block, position)?;
        Ok(true)
    }

    /// A run answered: written where it belongs, and the next asked for.
    fn take_run(&mut self, r: &mut Reader<'_>) -> Result<()> {
        let (file, at) = (usize::try_from(r.u32()?).unwrap_or(usize::MAX), r.u64()?);
        let run = r.bytes()?;
        r.finish()?;
        if self.phase != Phase::Runs || file != self.file {
            return Ok(());
        }
        let Some(&(wanted, length)) = self.needed.first() else { return Ok(()) };
        if at != wanted {
            return Ok(());
        }
        let Some((staged, _)) = self.open.as_ref() else { return Ok(()) };
        staged.write_all_at(run, at)?;
        let got = u32::try_from(run.len()).unwrap_or(u32::MAX);
        if got >= length || run.is_empty() {
            self.needed.remove(0);
        } else if let Some(first) = self.needed.first_mut() {
            *first = (at.saturating_add(u64::from(got)), length.saturating_sub(got));
        }
        self.advance_runs()
    }

    /// The file's runs all fetched, it is synced and the next file begun.
    fn advance_runs(&mut self) -> Result<()> {
        if !self.needed.is_empty() {
            return Ok(());
        }
        if let Some((staged, _)) = self.open.take() {
            staged.sync(Durability::Full)?;
        }
        self.open_next(self.file.saturating_add(1))
    }

    /// Opens the staged file `index` for writing, and the fetcher's own of
    /// that name for reading, if it has one; or ends the fetch once every
    /// file is staged.
    fn open_next(&mut self, index: usize) -> Result<()> {
        self.file = index;
        if index >= self.files.len() {
            self.done = true;
            return Ok(());
        }
        let (path, within) = self.files.get(index).map(|f| (f.path.clone(), f.within.clone())).unwrap_or_default();
        let staged = self.fs.open(&path, OpenMode::CreateNew)?;
        let own = self.dir.join(within);
        let own = if self.fs.exists(&own)? { Some(self.fs.open(&own, OpenMode::ReadOnly)?) } else { None };
        self.open = Some((staged, own));
        self.needed.clear();
        self.phase = Phase::Summary(0);
        Ok(())
    }

    /// The fetch begun again, from the manifest: the export served changed
    /// under it.
    fn restart(&mut self) -> Result<()> {
        self.open = None;
        for staged in self.files.drain(..) {
            self.fs.remove(&staged.path)?;
        }
        self.export = None;
        self.image = None;
        self.needed.clear();
        self.phase = Phase::Manifest;
        Ok(())
    }
}
