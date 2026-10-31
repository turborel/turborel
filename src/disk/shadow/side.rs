//! \[DESIGN\] The side files of shadow files (RULES.md D30): the blocks
//! they hold, and how a file is recovered from them as it is opened (see
//! `disk/shadow.rs`). Split from shadow.rs (RULES.md R-SIZE-4).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::GRANULE;
use crate::bytes::{Put, Reader};
use crate::disk::block::{BlockFile, BlockId, BlockOptions};
use crate::disk::encoding::Pipeline;
use crate::disk::io::{Durability, FileSystem, OpenMode};
use crate::error::{Code, Result, SqlError};
use crate::hash::HashFunction;

/// The bits of a byte's position within its granule.
pub(super) const GRANULE_BITS: u32 = 12;
const GRANULE_BYTES: u64 = 1 << GRANULE_BITS;
pub(super) const KIND_GRANULE: u8 = 0;
pub(super) const KIND_SIZE: u8 = 1;
const KIND_COMMIT: u8 = 2;
const KIND_LIMIT: u8 = 3;
const KIND_REMOVE: u8 = 4;

/// \[DESIGN\] The endings of the names of a file's two side files: its own
/// name and one of these.
pub(super) const SIDES: [&str; 2] = [".shadow0", ".shadow1"];

/// Side file `which` (0 or 1) of the file at `path`.
pub(super) fn side_path(path: &Path, which: usize) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(SIDES.get(which).copied().unwrap_or_default());
    PathBuf::from(name)
}

/// The file whose side file is at `side`, if `side` is a side file's path:
/// its ending compared in either case, as a file system may.
pub(super) fn base_of(side: &Path) -> Option<PathBuf> {
    let name = side.file_name()?.to_str()?;
    let cut = name.len().checked_sub(SIDES.first()?.len())?;
    let (base, ending) = (name.get(..cut)?, name.get(cut..)?);
    let is_side = !base.is_empty() && SIDES.iter().any(|s| s.eq_ignore_ascii_case(ending));
    is_side.then(|| side.with_file_name(base))
}

/// Where a shadowed file is: the file system under the shadowing, its
/// path, and the hash its side files' blocks are checked with.
pub(super) struct At {
    pub(super) fs: Arc<dyn FileSystem>,
    pub(super) path: PathBuf,
    pub(super) hash: Arc<dyn HashFunction>,
}

impl At {
    /// Makes the creation or removal of the file durable in its directory.
    pub(super) fn sync_dir(&self) -> Result<()> {
        match self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            Some(dir) => self.fs.sync_dir(dir),
            None => Ok(()),
        }
    }

    /// An empty side file `which`, replacing any there: made, synced and
    /// synced in its directory (`BlockFile::create`).
    pub(super) fn fresh_side(&self, which: usize) -> Result<BlockFile> {
        let path = side_path(&self.path, which);
        if self.fs.exists(&path)? {
            self.fs.remove(&path)?;
        }
        BlockFile::create(&*self.fs, &path, Arc::clone(&self.hash), Pipeline::none(), BlockOptions::default())
    }
}

/// What a side file's block says.
pub(super) enum Op {
    Granule(u64, Vec<u8>),
    Size(u64),
    Commit,
    Limit(u64),
    /// The file is removed.
    Remove,
}

pub(super) fn encode(op: &Op) -> Vec<u8> {
    let mut out = Vec::new();
    match op {
        Op::Granule(number, content) => {
            out.put_u8(KIND_GRANULE);
            out.put_u64(*number);
            out.extend_from_slice(content);
        }
        Op::Size(size) => {
            out.put_u8(KIND_SIZE);
            out.put_u64(*size);
        }
        Op::Commit => out.put_u8(KIND_COMMIT),
        Op::Limit(limit) => {
            out.put_u8(KIND_LIMIT);
            out.put_u64(*limit);
        }
        Op::Remove => out.put_u8(KIND_REMOVE),
    }
    out
}

pub(super) fn decode(bytes: &[u8]) -> Result<Op> {
    let mut r = Reader::new(bytes, "shadow block");
    let op = match r.u8()? {
        KIND_GRANULE => Op::Granule(r.u64()?, r.take(GRANULE)?.to_vec()),
        KIND_SIZE => Op::Size(r.u64()?),
        KIND_COMMIT => Op::Commit,
        KIND_LIMIT => Op::Limit(r.u64()?),
        KIND_REMOVE => Op::Remove,
        _ => return Err(r.corrupt("its kind is not known")),
    };
    r.finish()?;
    Ok(op)
}

/// The granule `number`'s offset in the file.
pub(super) fn offset(number: u64) -> u64 {
    number.saturating_mul(GRANULE_BYTES)
}

/// The granule a byte's `position` is in, and where in it.
pub(super) fn granule_of(position: u64) -> (u64, usize) {
    (position >> GRANULE_BITS, usize::try_from(position & (GRANULE_BYTES - 1)).unwrap_or_default())
}

/// A side file and the granules whose newest content it holds, in it or
/// in memory. The file is made when the first block is appended.
pub(super) struct Side {
    pub(super) blocks: Option<BlockFile>,
    pub(super) granules: HashMap<u64, BlockId>,
    /// Granules written while held: newer than those of `granules`.
    pub(super) held: HashMap<u64, Vec<u8>>,
    /// Which of the two side files it is.
    pub(super) which: usize,
    /// The least size the file had while this side file was written to.
    pub(super) limit: u64,
}

impl Side {
    /// Side file `which`, not yet made, of a file of size `limit`.
    pub(super) fn new(which: usize, limit: u64) -> Side {
        Side { blocks: None, granules: HashMap::new(), held: HashMap::new(), which, limit }
    }

    /// Whether it holds no granule.
    pub(super) fn is_empty(&self) -> bool {
        self.granules.is_empty() && self.held.is_empty()
    }

    /// The bytes appended to it.
    pub(super) fn bytes(&self) -> u64 {
        self.blocks.as_ref().map_or(0, |b| b.end().0.saturating_sub(b.first().0))
    }

    /// Appends `op`, the side file made first if it is not yet.
    pub(super) fn append(&mut self, at: &At, op: &Op) -> Result<BlockId> {
        let blocks = match self.blocks.take() {
            Some(blocks) => blocks,
            None => at.fresh_side(self.which)?,
        };
        let appended = blocks.append(&encode(op));
        self.blocks = Some(blocks);
        appended
    }

    /// The content of granule `number` as it holds it, if it does.
    pub(super) fn held(&self, number: u64) -> Result<Option<Vec<u8>>> {
        if let Some(content) = self.held.get(&number) {
            return Ok(Some(content.clone()));
        }
        let (Some(&at), Some(blocks)) = (self.granules.get(&number), self.blocks.as_ref()) else { return Ok(None) };
        let Op::Granule(_, content) = decode(&blocks.read(at)?)? else {
            return Err(SqlError::new(Code::Corrupt, "[DESIGN] a shadow block of a granule holds something else"));
        };
        Ok(Some(content))
    }

    /// Forgets the granules wholly past `size`, of those before granule
    /// `kept` and it, and lowers the limit to `size`.
    pub(super) fn cut(&mut self, kept: u64, size: u64) {
        self.granules.retain(|&n, _| n <= kept && offset(n) < size);
        self.held.retain(|&n, _| n <= kept && offset(n) < size);
        self.limit = self.limit.min(size);
    }
}

/// Recovers the file at `at` from its side files: one ending in a commit
/// block is copied into it, the file made if it is not there, or removed
/// if the batch removed it; the side files are then thrown away.
pub(super) fn recover(at: &At) -> Result<()> {
    for which in 0..SIDES.len() {
        let side = side_path(&at.path, which);
        if !at.fs.exists(&side)? {
            continue;
        }
        let (blocks, _) = BlockFile::open(&*at.fs, &side, &[Arc::clone(&at.hash)], &[], BlockOptions::default())?;
        let (ops, committed) = read_ops(&blocks)?;
        drop(blocks);
        if committed {
            apply(at, ops)?;
        }
        at.fs.remove(&side)?;
    }
    Ok(())
}

/// Copies the operations of a committed side file into the file at `at`:
/// removed, if they say so; else made if missing, cut to the limit, the
/// granules in order, the size.
fn apply(at: &At, ops: Vec<Op>) -> Result<()> {
    let exists = at.fs.exists(&at.path)?;
    if ops.iter().any(|op| matches!(op, Op::Remove)) {
        if exists {
            at.fs.remove(&at.path)?;
            at.sync_dir()?;
        }
        return Ok(());
    }
    let base = at.fs.open(&at.path, if exists { OpenMode::ReadWrite } else { OpenMode::CreateNew })?;
    if let Some(limit) = ops.iter().find_map(|op| if let Op::Limit(limit) = op { Some(*limit) } else { None }) {
        base.set_size(limit)?;
    }
    for op in ops {
        match op {
            Op::Granule(number, content) => base.write_all_at(&content, offset(number))?,
            Op::Size(to) => base.set_size(to)?,
            Op::Commit | Op::Limit(_) | Op::Remove => {}
        }
    }
    base.sync(Durability::Full)?;
    if exists { Ok(()) } else { at.sync_dir() }
}

/// The operations of a side file up to its commit, and whether it has one.
pub(super) fn read_ops(blocks: &BlockFile) -> Result<(Vec<Op>, bool)> {
    let mut cursor = blocks.cursor(blocks.first());
    let mut payload = Vec::new();
    let mut ops = Vec::new();
    while cursor.next(&mut payload)?.is_some() {
        match decode(&payload)? {
            Op::Commit => return Ok((ops, true)),
            op @ (Op::Granule(..) | Op::Size(_) | Op::Limit(_) | Op::Remove) => ops.push(op),
        }
    }
    Ok((ops, false))
}
