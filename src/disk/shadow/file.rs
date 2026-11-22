//! \[DESIGN\] A shadowed file (RULES.md D30): what it has written since its
//! last commit, and how a batch of it is sealed and committed (see
//! `disk/shadow.rs`).

use std::mem;
use std::sync::{Arc, RwLock, RwLockWriteGuard};

use super::GRANULE;
use super::flight::Flight;
use super::side::{At, Op, Side, encode, granule_of, offset, recover, side_path};
use crate::disk::block::BlockFile;
use crate::disk::io::{DiskFile, Durability, OpenMode};
use crate::error::{Code, Result, SqlError};
use crate::sync::{read, write};

/// A batch sealed for its commit: its side file, the file's size when
/// sealed, and whether the file was removed then.
pub(super) struct Sealed {
    pub(super) side: Side,
    pub(super) size: u64,
    removed: bool,
}

/// Where a granule's content is (`Shadow::source`).
enum Source<'a> {
    /// Written since the last commit, in this side file.
    Written(&'a Side),
    /// The file's, to read.
    Base(Arc<dyn DiskFile>),
    /// Zeros.
    Zeros,
}

/// A run of a read's buffer that is the file's: the file, where in it the
/// run starts, and the run.
type BaseRun = (Arc<dyn DiskFile>, u64, std::ops::Range<usize>);

/// What a shadowed file has written since its last commit.
pub(super) struct Pending {
    pub(super) active: Side,
    pub(super) sealed: Option<Arc<Sealed>>,
    /// The file's size as written.
    size: u64,
    /// Whether writes are held in memory until the next seal.
    pub(super) holding: bool,
    /// Whether the file was removed since it was last made.
    removed: bool,
    /// The size the file had at its last commit, or as it was opened;
    /// none while it is not there.
    committed: Option<u64>,
}

impl Pending {
    /// Makes `content`, whole granules from `first`, their newest: in
    /// memory if held, else in the side file as one block (`Side::place`).
    fn put(&mut self, at: &At, first: u64, content: Vec<u8>) -> Result<()> {
        if self.holding {
            for (number, granule) in (first..).zip(content.chunks(GRANULE)) {
                self.active.held.insert(number, granule.to_vec());
            }
            Ok(())
        } else {
            self.active.place(at, first, content)
        }
    }

    /// Whether the next commit would change nothing: the file removed and
    /// gone, or neither removed nor written to since its last commit.
    fn unchanged(&self) -> bool {
        let gone = self.removed && self.committed.is_none();
        let same = !self.removed && self.active.is_empty() && self.committed == Some(self.size) && self.active.limit >= self.size;
        gone || same
    }
}

/// \[DESIGN\] A shadowed file and its side files. The file itself is made
/// at the first commit of one made through the shadowing, and removed at
/// the commit after its removal.
pub(super) struct Shadow {
    base: RwLock<Option<Arc<dyn DiskFile>>>,
    at: At,
    pub(super) pending: RwLock<Pending>,
    /// Granules written over outside `pending` (RULES.md D472).
    flight: Flight,
}

/// The error for a file found missing, or there, against what `what` needs.
fn refused(at: &At, what: &str) -> SqlError {
    SqlError::new(Code::Io, format!("[DESIGN] could not {what} {}", at.path.display()))
}

impl Shadow {
    /// Shadows the file at `at`, opened as `mode` says, recovered first
    /// (`side::recover`); one to be made is not made until its first
    /// commit. One there is opened for writing, whatever `mode`, as its
    /// commits write it, and a later opening may write. Writes are held if
    /// `holding`.
    pub(super) fn open(at: At, mode: OpenMode, holding: bool) -> Result<Shadow> {
        recover(&at)?;
        let exists = at.fs.exists(&at.path)?;
        let opening = match mode {
            OpenMode::ReadOnly | OpenMode::ReadWrite => OpenMode::ReadWrite,
            OpenMode::CreateNew => OpenMode::CreateNew,
        };
        let base: Option<Arc<dyn DiskFile>> =
            if opening == OpenMode::CreateNew && !exists { None } else { Some(Arc::from(at.fs.open(&at.path, opening)?)) };
        let size = match &base {
            Some(file) => file.size()?,
            None => 0,
        };
        let committed = base.as_ref().map(|_| size);
        let pending = Pending { active: Side::new(0, size), sealed: None, size, holding, removed: false, committed };
        Ok(Shadow { base: RwLock::new(base), at, pending: RwLock::new(pending), flight: Flight::new() })
    }

    /// Opened again, as `mode` says: one removed is made again by
    /// `CreateNew`, empty, and refused otherwise; one there is refused by
    /// `CreateNew`.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the mode needs the file there and it is not, or not
    /// there and it is.
    pub(super) fn reopen(&self, mode: OpenMode) -> Result<()> {
        let mut pending = self.settled()?;
        match (mode, pending.removed) {
            (OpenMode::CreateNew, true) => {
                pending.removed = false;
                Ok(())
            }
            (OpenMode::CreateNew, false) => Err(refused(&self.at, "create, as it exists,")),
            (OpenMode::ReadWrite | OpenMode::ReadOnly, true) => Err(refused(&self.at, "open, as it does not exist,")),
            (OpenMode::ReadWrite | OpenMode::ReadOnly, false) => Ok(()),
        }
    }

    /// Removes it, from its next commit: it reads as empty meanwhile. Whether
    /// it must stay known until then, as a commit made it or a sealed batch
    /// will.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if it is removed already.
    pub(super) fn remove(&self) -> Result<bool> {
        let mut pending = self.settled()?;
        if pending.removed {
            return Err(refused(&self.at, "remove, as it does not exist,"));
        }
        pending.removed = true;
        pending.size = 0;
        pending.active.cut(0, 0);
        Ok(pending.committed.is_some() || pending.sealed.is_some())
    }

    /// Stops holding writes in memory: what was held goes to the side file.
    pub(super) fn release(&self) -> Result<()> {
        let mut pending = self.settled()?;
        pending.holding = false;
        let held: Vec<(u64, Vec<u8>)> = pending.active.held.drain().collect();
        held.into_iter().try_for_each(|(number, content)| pending.put(&self.at, number, content))
    }

    /// Whether it is removed, not to be made again before its next commit.
    pub(super) fn removed(&self) -> bool {
        read(&self.pending).is_ok_and(|p| p.removed)
    }

    /// Whether it holds nothing to commit, sealed or not.
    pub(super) fn clean(&self) -> bool {
        read(&self.pending).is_ok_and(|p| p.sealed.is_none() && p.unchanged())
    }

    /// The bytes of the side file written to.
    pub(super) fn pending_bytes(&self) -> u64 {
        read(&self.pending).map_or(0, |p| p.active.bytes())
    }

    /// Its pending writes, the lock written, once no granule is in flight.
    fn settled(&self) -> Result<RwLockWriteGuard<'_, Pending>> {
        let pending = write(&self.pending)?;
        self.flight.wait_all();
        Ok(pending)
    }

    /// Writes `data` at `at` over the block of its run with no lock held,
    /// if it is whole granules within the file's size, each still in that
    /// run's block in the side file written to, none held or in flight:
    /// a page written back again (RULES.md D472). Whether it did.
    fn write_over(&self, data: &[u8], at: u64) -> Result<bool> {
        let (first, within) = granule_of(at);
        let whole = within == 0 && !data.is_empty() && data.len().checked_rem(GRANULE) == Some(0);
        let count = u64::try_from(data.len().checked_div(GRANULE).unwrap_or_default()).unwrap_or_default();
        let end = at.saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
        let numbers = first..first.saturating_add(count);
        let (spot, blocks) = {
            let pending = write(&self.pending)?;
            let found = pending.active.same_run(first, count).zip(pending.active.blocks.clone());
            let Some(found) = found.filter(|_| whole && !pending.holding && end <= pending.size) else { return Ok(false) };
            if !self.flight.take_off(numbers.clone()) {
                return Ok(false);
            }
            found
        };
        let payload = encode(&Op::Granule(first, data.to_vec()));
        let written = blocks.overwrite(spot.block, &payload, spot.payload_len());
        self.flight.land(numbers);
        written.map(|()| true)
    }

    /// The content of granule `number`, as written: from the side file
    /// written to, or the sealed one, or the file, zeros past the end or a
    /// limit.
    fn granule(&self, pending: &Pending, number: u64) -> Result<Vec<u8>> {
        let mut content = vec![0; GRANULE];
        match self.source(pending, number)? {
            Source::Written(side) => {
                self.flight.wait_for(number);
                side.copy_held(number, 0, &mut content)?;
            }
            Source::Base(base) => {
                let _ = base.read_at(&mut content, offset(number))?;
            }
            Source::Zeros => {}
        }
        Ok(content)
    }

    /// Where the content of granule `number` is, as written: the side file
    /// written to, or the sealed one, read here; the file, read by the
    /// caller; or nowhere, zeros past the end or a limit.
    fn source<'a>(&self, pending: &'a Pending, number: u64) -> Result<Source<'a>> {
        let start = offset(number);
        if pending.active.holds(number) {
            return Ok(Source::Written(&pending.active));
        }
        let mut limit = pending.active.limit;
        if let Some(sealed) = pending.sealed.as_ref().filter(|_| start < limit) {
            if sealed.side.holds(number) {
                return Ok(Source::Written(&sealed.side));
            }
            limit = limit.min(sealed.side.limit);
        }
        // The file holds zeros past its size at the last commit, or the
        // sealed batch's granules, found above, so it is read only below
        // that size, kept rather than asked for each read; while a batch
        // is committed a read past the file's end gives zeros too.
        let within = start < limit && pending.committed.is_some_and(|size| start < size);
        Ok(read(&self.base)?.as_ref().filter(|_| within).map_or(Source::Zeros, |base| Source::Base(Arc::clone(base))))
    }

    /// Fills `buf`'s first `available` bytes, read from `at`, with what was
    /// written since the last commit and zeros, under the lock: the runs
    /// of `buf` that are the file's left to read, each with where in the
    /// file it starts.
    fn placed(&self, pending: &Pending, buf: &mut [u8], (at, available): (u64, usize)) -> Result<Vec<BaseRun>> {
        let mut runs: Vec<BaseRun> = Vec::new();
        let mut done = 0_usize;
        while done < available {
            let position = at.saturating_add(u64::try_from(done).unwrap_or(u64::MAX));
            let (number, within) = granule_of(position);
            let n = (GRANULE.saturating_sub(within)).min(available.saturating_sub(done));
            let to = done..done.saturating_add(n);
            match self.source(pending, number)? {
                Source::Written(side) => {
                    self.flight.wait_for(number);
                    side.copy_held(number, within, buf.get_mut(to).unwrap_or_default())?;
                }
                Source::Zeros => buf.get_mut(to).unwrap_or_default().fill(0),
                Source::Base(base) => {
                    if let Some(last) = runs.last_mut().filter(|last| last.2.end == done && Arc::ptr_eq(&last.0, &base)) {
                        last.2.end = to.end;
                    } else {
                        runs.push((base, position, to));
                    }
                }
            }
            done = done.saturating_add(n);
        }
        Ok(runs)
    }

    /// Writes `data` at `at` with the pending writes locked: whole granules
    /// as one run, one block for a page; a granule written in part read,
    /// changed and put alone.
    fn write_locked(&self, pending: &mut Pending, data: &[u8], at: u64) -> Result<()> {
        let mut done = 0_usize;
        while done < data.len() {
            let position = at.saturating_add(u64::try_from(done).unwrap_or(u64::MAX));
            let (number, within) = granule_of(position);
            let left = data.len().saturating_sub(done);
            let whole = if within == 0 { left.checked_div(GRANULE).unwrap_or_default().saturating_mul(GRANULE) } else { 0 };
            let n = if whole > 0 { whole } else { GRANULE.saturating_sub(within).min(left) };
            let from = data.get(done..done.saturating_add(n)).unwrap_or_default();
            let content = if whole > 0 { from.to_vec() } else { self.granule(pending, number)? };
            let content = if whole > 0 { content } else { patched(content, within, from) };
            pending.put(&self.at, number, content)?;
            done = done.saturating_add(n);
        }
        let end = at.saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
        pending.size = pending.size.max(end);
        Ok(())
    }

    /// Seals what was written for its commit, writes going to the other
    /// side file from now on; a batch sealed before and not committed is
    /// committed first. Nothing is sealed if nothing would change, unless
    /// `always`, for a capture to write into (RULES.md D590), and the
    /// file is there.
    pub(super) fn seal(&self, always: bool) -> Result<()> {
        if read(&self.pending)?.sealed.is_some() {
            self.commit_sealed()?;
        }
        let mut pending = self.settled()?;
        pending.holding = false;
        if pending.unchanged() && (!always || pending.removed) {
            return Ok(());
        }
        let which = 1_usize.saturating_sub(pending.active.which);
        let now = pending.size;
        let side = mem::replace(&mut pending.active, Side::new(which, now));
        let removed = pending.removed;
        pending.sealed = Some(Arc::new(Sealed { side, size: now, removed }));
        Ok(())
    }

    /// Commits the sealed batch, durably, then forgets it and removes its
    /// side file. Writers go on meanwhile.
    pub(super) fn commit_sealed(&self) -> Result<()> {
        let Some(sealed) = read(&self.pending)?.sealed.clone() else { return Ok(()) };
        let committed = if sealed.removed {
            self.commit_removal(&sealed)?;
            None
        } else {
            self.commit_content(&sealed)?;
            Some(sealed.size)
        };
        let which = sealed.side.which;
        {
            let mut pending = self.settled()?;
            pending.sealed = None;
            pending.committed = committed;
        }
        drop(sealed);
        let path = side_path(&self.at.path, which);
        if self.at.fs.exists(&path)? {
            self.at.fs.remove(&path)?;
        }
        Ok(())
    }

    /// Ends the side file of `sealed` with `last`, synced, then a commit
    /// block, synced: from then on the batch is the file's, after a crash
    /// too. Synced first, a commit block found says every block before it
    /// is whole, whichever were written over.
    pub(super) fn end_side(&self, sealed: &Sealed, last: &[Op]) -> Result<()> {
        let side = &sealed.side;
        let made: BlockFile;
        let blocks = if let Some(blocks) = &side.blocks {
            &**blocks
        } else {
            made = self.at.fresh_side(side.which)?;
            &made
        };
        for op in last {
            blocks.append(&encode(op))?;
        }
        blocks.sync(Durability::Full)?;
        blocks.append(&encode(&Op::Commit))?;
        blocks.sync(Durability::Full)
    }

    /// Commits a batch of content: its side file ended, then the file,
    /// made if it is not there, cut to the limit, every granule copied
    /// into it, its size set, synced.
    fn commit_content(&self, sealed: &Sealed) -> Result<()> {
        let side = &sealed.side;
        self.end_side(sealed, &closing_ops(sealed))?;
        let (base, made) = self.base_to_write()?;
        base.set_size(side.limit.min(base.size()?))?;
        let mut numbers: Vec<u64> = side.granules.keys().chain(side.held.keys()).copied().collect();
        numbers.sort_unstable();
        numbers.dedup();
        write_runs(side, &numbers, &*base)?;
        base.set_size(sealed.size)?;
        base.sync(Durability::Full)?;
        if made { self.at.sync_dir() } else { Ok(()) }
    }

    /// The file, to write a commit into, and whether it was made for it.
    fn base_to_write(&self) -> Result<(Arc<dyn DiskFile>, bool)> {
        if let Some(base) = read(&self.base)?.clone() {
            return Ok((base, false));
        }
        let base: Arc<dyn DiskFile> = Arc::from(self.at.fs.open(&self.at.path, OpenMode::CreateNew)?);
        *write(&self.base)? = Some(Arc::clone(&base));
        Ok((base, true))
    }

    /// Commits a removal: its side file ended, then the file removed, in
    /// its directory durably.
    fn commit_removal(&self, sealed: &Sealed) -> Result<()> {
        self.end_side(sealed, &[Op::Remove])?;
        drop(write(&self.base)?.take());
        if self.at.fs.exists(&self.at.path)? {
            self.at.fs.remove(&self.at.path)?;
            self.at.sync_dir()?;
        }
        Ok(())
    }
}

/// \[DESIGN\] A file of a `ShadowFileSystem`.
pub(super) struct ShadowFile(pub(super) Arc<Shadow>);

impl DiskFile for ShadowFile {
    /// \[DESIGN\] Read in two steps (RULES.md D409): under the lock,
    /// what was written since the last commit is copied and the rest found
    /// to be the file's; then, the lock let go, each run of the file's
    /// granules is read with one call. A commit writes into the file only
    /// granules the sealed batch holds, which are copied from it here, and
    /// cuts it no lower than the limit read below, so what is read of the
    /// file is what the lock saw.
    fn read_at(&self, buf: &mut [u8], at: u64) -> Result<usize> {
        let (available, runs) = {
            let pending = read(&self.0.pending)?;
            let wanted = u64::try_from(buf.len()).unwrap_or(u64::MAX);
            let available = usize::try_from(pending.size.saturating_sub(at).min(wanted)).unwrap_or_default();
            (available, self.0.placed(&pending, buf, (at, available))?)
        };
        for (base, from, range) in runs {
            if let Some(to) = buf.get_mut(range) {
                let _ = base.read_at(to, from)?;
            }
        }
        Ok(available)
    }

    /// Whole granules written go as one run, one block for a page;
    /// a granule written in part is read, changed and put alone.
    fn write_all_at(&self, data: &[u8], at: u64) -> Result<()> {
        if self.0.write_over(data, at)? {
            return Ok(());
        }
        let mut pending = self.0.settled()?;
        self.0.write_locked(&mut pending, data, at)
    }

    /// \[DESIGN\] RULES.md D590: while held, into the batch held, as any
    /// write; once sealed, into the sealed batch, whose granules are
    /// newer than its blocks and older than what was written since. A
    /// capture into the sealed batch changes nothing a write in flight
    /// (D472) writes over or a read waits for, so it waits for none: the
    /// lock is held only to put its granules, copied before it is taken.
    fn write_captured_at(&self, data: &[u8], at: u64) -> Result<()> {
        let (first, within) = granule_of(at);
        let end = at.saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
        let refused = || SqlError::new(Code::Api, "[DESIGN] RULES.md D590: a capture is whole granules within the sealed batch");
        if within != 0 || data.is_empty() || data.len().checked_rem(GRANULE) != Some(0) {
            return Err(refused());
        }
        let granules: Vec<(u64, Vec<u8>)> = (first..).zip(data.chunks(GRANULE).map(<[u8]>::to_vec)).collect();
        let mut pending = write(&self.0.pending)?;
        if pending.holding {
            // Held, it is a write as any, which waits for those in flight;
            // looked at again once they land, as the batch may be sealed.
            drop(pending);
            pending = self.0.settled()?;
            if pending.holding {
                return self.0.write_locked(&mut pending, data, at);
            }
        }
        let sealed = pending.sealed.as_mut().and_then(Arc::get_mut).filter(|s| end <= s.size).ok_or_else(refused)?;
        sealed.side.held.extend(granules);
        Ok(())
    }

    fn size(&self) -> Result<u64> {
        Ok(read(&self.0.pending)?.size)
    }

    /// Granules wholly past the new size are forgotten, and the limit
    /// lowered to it; the one it falls in has its bytes past it zeroed, so
    /// growing again reads zeros.
    fn set_size(&self, size: u64) -> Result<()> {
        let mut pending = self.0.settled()?;
        if size < pending.size {
            let (number, within) = granule_of(size);
            if within > 0 {
                let mut content = self.0.granule(&pending, number)?;
                if let Some(tail) = content.get_mut(within..) {
                    tail.fill(0);
                }
                pending.put(&self.0.at, number, content)?;
            }
            let kept = if within > 0 { number } else { number.saturating_sub(1) };
            pending.active.cut(kept, size);
        }
        pending.size = size;
        Ok(())
    }

    /// Nothing: what is written becomes durable only at a commit.
    fn sync(&self, _durability: Durability) -> Result<()> {
        Ok(())
    }
}

/// `content` with `from` copied over it from `within`.
fn patched(mut content: Vec<u8>, within: usize, from: &[u8]) -> Vec<u8> {
    if let Some(to) = content.get_mut(within..within.saturating_add(from.len())) {
        to.copy_from_slice(from);
    }
    content
}

/// \[DESIGN\] The most bytes of granules one op closing a side file, and
/// one write of a commit, carries (RULES.md D811): granules next to each
/// other go as one run, not one op, one frame and one write each.
pub(super) const RUN_BYTES: usize = 1 << 20;

/// What ends a batch's side file before its commit block: the granules a
/// cut made stale, then those held, newer than either, in runs, the limit
/// and the size.
pub(super) fn closing_ops(sealed: &Sealed) -> Vec<Op> {
    let side = &sealed.side;
    let mut last: Vec<Op> = side.stale.iter().map(|number| Op::Stale(*number)).collect();
    let mut kept: Vec<(&u64, &Vec<u8>)> = side.held.iter().collect();
    kept.sort_unstable_by_key(|(number, _)| **number);
    let mut run: Option<(u64, Vec<u8>)> = None;
    for (&number, content) in kept {
        if let Some((first, bytes)) = run.as_mut()
            && extends(*first, bytes, number)
        {
            bytes.extend_from_slice(content);
            continue;
        }
        last.extend(run.take().map(|(first, bytes)| Op::Granule(first, bytes)));
        run = Some((number, content.clone()));
    }
    last.extend(run.map(|(first, bytes)| Op::Granule(first, bytes)));
    last.extend([Op::Limit(side.limit), Op::Size(sealed.size)]);
    last
}

/// \[DESIGN\] Whether granule `number` extends the run of `bytes` from
/// granule `first` (RULES.md D811): it is the next one, and the run is
/// still short of `RUN_BYTES`.
fn extends(first: u64, bytes: &[u8], number: u64) -> bool {
    let next = u64::try_from(bytes.len().checked_div(GRANULE).unwrap_or_default()).ok().and_then(|count| first.checked_add(count));
    next == Some(number) && bytes.len() < RUN_BYTES
}

/// Writes into `base` the granules numbered `numbers`, in order, that
/// `side` holds, each run of them next to each other as one write of at
/// most `RUN_BYTES` (RULES.md D811).
fn write_runs(side: &Side, numbers: &[u64], base: &dyn DiskFile) -> Result<()> {
    let mut run: Vec<u8> = Vec::new();
    let mut first = 0_u64;
    for &number in numbers {
        if !run.is_empty() && !extends(first, &run, number) {
            base.write_all_at(&run, offset(first))?;
            run.clear();
        }
        let at = run.len();
        run.resize(at.saturating_add(GRANULE), 0);
        let found = side.copy_held(number, 0, run.get_mut(at..).unwrap_or_default())?;
        if !found {
            run.truncate(at);
        } else if at == 0 {
            first = number;
        }
    }
    if run.is_empty() { Ok(()) } else { base.write_all_at(&run, offset(first)) }
}
