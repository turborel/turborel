//! \[DESIGN\] A shadowed file (RULES.md D30): what it has written since its
//! last commit, and how a batch of it is sealed and committed (see
//! `disk/shadow.rs`). Split from shadow.rs (RULES.md R-SIZE-4).

use std::mem;
use std::sync::{Arc, RwLock};

use super::GRANULE;
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
    /// Makes `content` granule `number`'s newest, in memory if held, else
    /// appended to the side file.
    fn put(&mut self, at: &At, number: u64, content: Vec<u8>) -> Result<()> {
        if self.holding {
            self.active.held.insert(number, content);
        } else {
            let block = self.active.append(at, &Op::Granule(number, content))?;
            self.active.held.remove(&number);
            self.active.granules.insert(number, block);
        }
        Ok(())
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
        Ok(Shadow { base: RwLock::new(base), at, pending: RwLock::new(pending) })
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
        let mut pending = write(&self.pending)?;
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
        let mut pending = write(&self.pending)?;
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
        let mut pending = write(&self.pending)?;
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

    /// The bytes appended to the side file written to.
    pub(super) fn pending_bytes(&self) -> u64 {
        read(&self.pending).map_or(0, |p| p.active.bytes())
    }

    /// The content of granule `number`, as written: from the side file
    /// written to, or the sealed one, or the file, zeros past the end or a
    /// limit.
    fn granule(&self, pending: &Pending, number: u64) -> Result<Vec<u8>> {
        let start = offset(number);
        if let Some(content) = pending.active.held(number)? {
            return Ok(content);
        }
        let mut limit = pending.active.limit;
        if let Some(sealed) = pending.sealed.as_ref().filter(|_| start < limit) {
            if let Some(content) = sealed.side.held(number)? {
                return Ok(content);
            }
            limit = limit.min(sealed.side.limit);
        }
        let mut content = vec![0; GRANULE];
        if let Some(base) = read(&self.base)?.as_ref().filter(|_| start < limit)
            && start < base.size()?
        {
            let _ = base.read_at(&mut content, start)?;
        }
        Ok(content)
    }

    /// Seals what was written for its commit, writes going to the other
    /// side file from now on; a batch sealed before and not committed is
    /// committed first. Nothing is sealed if nothing would change.
    pub(super) fn seal(&self) -> Result<()> {
        if read(&self.pending)?.sealed.is_some() {
            self.commit_sealed()?;
        }
        let mut pending = write(&self.pending)?;
        pending.holding = false;
        if pending.unchanged() {
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
            let mut pending = write(&self.pending)?;
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

    /// Ends the side file of `sealed` with `last` and a commit block,
    /// synced: from then on the batch is the file's, after a crash too.
    pub(super) fn end_side(&self, sealed: &Sealed, last: &[Op]) -> Result<()> {
        let side = &sealed.side;
        let made: BlockFile;
        let blocks = if let Some(blocks) = &side.blocks {
            blocks
        } else {
            made = self.at.fresh_side(side.which)?;
            &made
        };
        for op in last.iter().chain([&Op::Commit]) {
            blocks.append(&encode(op))?;
        }
        blocks.sync(Durability::Full)
    }

    /// Commits a batch of content: its side file ended, then the file,
    /// made if it is not there, cut to the limit, every granule copied
    /// into it, its size set, synced.
    fn commit_content(&self, sealed: &Sealed) -> Result<()> {
        let side = &sealed.side;
        let mut kept: Vec<(&u64, &Vec<u8>)> = side.held.iter().collect();
        kept.sort_unstable_by_key(|(number, _)| **number);
        let mut last: Vec<Op> = kept.into_iter().map(|(number, content)| Op::Granule(*number, content.clone())).collect();
        last.extend([Op::Limit(side.limit), Op::Size(sealed.size)]);
        self.end_side(sealed, &last)?;
        let (base, made) = self.base_to_write()?;
        base.set_size(side.limit.min(base.size()?))?;
        let mut numbers: Vec<u64> = side.granules.keys().chain(side.held.keys()).copied().collect();
        numbers.sort_unstable();
        numbers.dedup();
        for number in numbers {
            if let Some(content) = side.held(number)? {
                base.write_all_at(&content, offset(number))?;
            }
        }
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
    fn read_at(&self, buf: &mut [u8], at: u64) -> Result<usize> {
        let pending = read(&self.0.pending)?;
        let wanted = u64::try_from(buf.len()).unwrap_or(u64::MAX);
        let available = usize::try_from(pending.size.saturating_sub(at).min(wanted)).unwrap_or_default();
        let mut done = 0_usize;
        while done < available {
            let position = at.saturating_add(u64::try_from(done).unwrap_or(u64::MAX));
            let (number, within) = granule_of(position);
            let content = self.0.granule(&pending, number)?;
            let n = (GRANULE.saturating_sub(within)).min(available.saturating_sub(done));
            if let (Some(to), Some(from)) = (buf.get_mut(done..done.saturating_add(n)), content.get(within..within.saturating_add(n))) {
                to.copy_from_slice(from);
            }
            done = done.saturating_add(n);
        }
        Ok(available)
    }

    fn write_all_at(&self, data: &[u8], at: u64) -> Result<()> {
        let mut pending = write(&self.0.pending)?;
        let mut done = 0_usize;
        while done < data.len() {
            let position = at.saturating_add(u64::try_from(done).unwrap_or(u64::MAX));
            let (number, within) = granule_of(position);
            let n = GRANULE.saturating_sub(within).min(data.len().saturating_sub(done));
            let mut content = if within == 0 && n == GRANULE { vec![0; GRANULE] } else { self.0.granule(&pending, number)? };
            if let (Some(to), Some(from)) = (content.get_mut(within..within.saturating_add(n)), data.get(done..done.saturating_add(n))) {
                to.copy_from_slice(from);
            }
            pending.put(&self.0.at, number, content)?;
            done = done.saturating_add(n);
        }
        let end = at.saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
        pending.size = pending.size.max(end);
        Ok(())
    }

    fn size(&self) -> Result<u64> {
        Ok(read(&self.0.pending)?.size)
    }

    /// Granules wholly past the new size are forgotten, and the limit
    /// lowered to it; the one it falls in has its bytes past it zeroed, so
    /// growing again reads zeros.
    fn set_size(&self, size: u64) -> Result<()> {
        let mut pending = write(&self.0.pending)?;
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
