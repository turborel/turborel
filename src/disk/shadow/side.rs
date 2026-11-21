//! \[DESIGN\] The side files of shadow files (RULES.md D30): the blocks
//! they hold, and how a file is recovered from them as it is opened (see
//! `disk/shadow.rs`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::GRANULE;
use crate::bytes::{Put, Reader};
use crate::disk::block::{BlockFile, BlockId, BlockOptions, damaged};
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
const KIND_STALE: u8 = 5;

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
    /// The content of a run of whole granules, from the first's number.
    Granule(u64, Vec<u8>),
    Size(u64),
    Commit,
    Limit(u64),
    /// The file is removed.
    Remove,
    /// A granule's blocks before this one no longer count: a cut dropped it
    /// and nothing rewrote it.
    Stale(u64),
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
        Op::Stale(number) => {
            out.put_u8(KIND_STALE);
            out.put_u64(*number);
        }
    }
    out
}

pub(super) fn decode(bytes: &[u8]) -> Result<Op> {
    let mut r = Reader::new(bytes, "shadow block");
    let op = match r.u8()? {
        KIND_GRANULE => {
            let first = r.u64()?;
            let content = r.rest().to_vec();
            if content.is_empty() || content.len().checked_rem(GRANULE) != Some(0) {
                return Err(r.corrupt("its granules are not whole"));
            }
            Op::Granule(first, content)
        }
        KIND_SIZE => Op::Size(r.u64()?),
        KIND_COMMIT => Op::Commit,
        KIND_LIMIT => Op::Limit(r.u64()?),
        KIND_REMOVE => Op::Remove,
        KIND_STALE => Op::Stale(r.u64()?),
        _ => return Err(r.corrupt("its kind is not known")),
    };
    r.finish()?;
    Ok(op)
}

/// The error for a granule held shorter than a granule.
fn short(number: u64) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] granule {number} of a shadow file is held short"))
}

/// The granule `number`'s offset in the file.
pub(super) fn offset(number: u64) -> u64 {
    number.saturating_mul(GRANULE_BYTES)
}

/// The granule a byte's `position` is in, and where in it.
pub(super) fn granule_of(position: u64) -> (u64, usize) {
    (position >> GRANULE_BITS, usize::try_from(position & (GRANULE_BYTES - 1)).unwrap_or_default())
}

/// The bytes before a granule block's content: its kind and first number.
const GRANULE_HEAD: usize = 9;

/// Where a granule's newest content is in a side file: the block of the
/// run of granules written with it, the run's first granule, and how many
/// it has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Spot {
    pub(super) block: BlockId,
    first: u64,
    count: u64,
}

impl Spot {
    /// Where granule `number`'s content starts in the block's payload.
    fn within(&self, number: u64) -> usize {
        let index = usize::try_from(number.saturating_sub(self.first)).unwrap_or(usize::MAX);
        GRANULE_HEAD.saturating_add(index.saturating_mul(GRANULE))
    }

    /// The length of the block's payload.
    pub(super) fn payload_len(&self) -> usize {
        self.within(self.first.saturating_add(self.count))
    }
}

/// A side file and the granules whose newest content it holds, in it or
/// in memory. The file is made when the first block is appended.
pub(super) struct Side {
    pub(super) blocks: Option<Arc<BlockFile>>,
    pub(super) granules: HashMap<u64, Spot>,
    /// Granules written while held: newer than those of `granules`.
    pub(super) held: HashMap<u64, Vec<u8>>,
    /// Granules with a block in the file that a cut left no longer the
    /// file's, so copying the side file after a crash skips them.
    pub(super) stale: HashSet<u64>,
    /// Which of the two side files it is.
    pub(super) which: usize,
    /// The least size the file had while this side file was written to.
    pub(super) limit: u64,
}

impl Side {
    /// Side file `which`, not yet made, of a file of size `limit`.
    pub(super) fn new(which: usize, limit: u64) -> Side {
        Side { blocks: None, granules: HashMap::new(), held: HashMap::new(), stale: HashSet::new(), which, limit }
    }

    /// Whether it holds no granule.
    pub(super) fn is_empty(&self) -> bool {
        self.granules.is_empty() && self.held.is_empty()
    }

    /// The bytes of its blocks.
    pub(super) fn bytes(&self) -> u64 {
        self.blocks.as_ref().map_or(0, |b| b.end().0.saturating_sub(b.first().0))
    }

    /// Appends `payload`, an encoded operation, the side file made first
    /// if it is not yet.
    fn append_encoded(&mut self, at: &At, payload: &[u8]) -> Result<BlockId> {
        let blocks = match self.blocks.take() {
            Some(blocks) => blocks,
            None => Arc::new(at.fresh_side(self.which)?),
        };
        let appended = blocks.append(payload);
        self.blocks = Some(blocks);
        appended
    }

    /// Puts `content`, a run of whole granules from `first`, in one block:
    /// over the block of the same run if the run's granules are all still
    /// there, so a page written back again and again takes one block;
    /// else appended, as a granule of it written apart since has a later
    /// block, which a commit copied after a crash would take over it.
    /// The granules' content is then that block's.
    pub(super) fn place(&mut self, at: &At, first: u64, content: Vec<u8>) -> Result<()> {
        let count = u64::try_from(content.len().checked_div(GRANULE).unwrap_or_default()).unwrap_or_default();
        let numbers = first..first.saturating_add(count);
        let same = self.same_run(first, count);
        let payload = encode(&Op::Granule(first, content));
        let block = match (same, self.blocks.as_ref()) {
            (Some(spot), Some(blocks)) => blocks.overwrite(spot.block, &payload, spot.payload_len()).map(|()| spot.block)?,
            _ => self.append_encoded(at, &payload)?,
        };
        let spot = Spot { block, first, count };
        for number in numbers {
            self.held.remove(&number);
            self.stale.remove(&number);
            self.granules.insert(number, spot);
        }
        Ok(())
    }

    /// The spot of the run of `count` granules from `first`, if each of
    /// them is still in that run's block.
    pub(super) fn same_run(&self, first: u64, count: u64) -> Option<Spot> {
        let mut numbers = first..first.saturating_add(count);
        let spot = self.granules.get(&first).copied()?;
        let whole = spot.first == first && spot.count == count;
        (whole && numbers.all(|n| self.granules.get(&n) == Some(&spot))).then_some(spot)
    }

    /// Fills `out` from granule `number`'s content as it holds it, from
    /// byte `within` of it, if it does: in memory or with one read of its
    /// block, unchecked, as the file's own bytes are read (RULES.md D471).
    /// Whether it holds it.
    pub(super) fn copy_held(&self, number: u64, within: usize, out: &mut [u8]) -> Result<bool> {
        if let Some(content) = self.held.get(&number) {
            let to = within.saturating_add(out.len());
            out.copy_from_slice(content.get(within..to).ok_or_else(|| short(number))?);
            return Ok(true);
        }
        let (Some(spot), Some(blocks)) = (self.granules.get(&number), self.blocks.as_ref()) else { return Ok(false) };
        blocks.read_payload_at(spot.block, spot.within(number).saturating_add(within), out)?;
        Ok(true)
    }

    /// Whether it holds granule `number`'s content.
    pub(super) fn holds(&self, number: u64) -> bool {
        self.held.contains_key(&number) || self.granules.contains_key(&number)
    }

    /// The content of granule `number` as it holds it, if it does.
    pub(super) fn held(&self, number: u64) -> Result<Option<Vec<u8>>> {
        let mut content = vec![0; GRANULE];
        Ok(self.copy_held(number, 0, &mut content)?.then_some(content))
    }

    /// Forgets the granules wholly past `size`, of those before granule
    /// `kept` and it, and lowers the limit to `size`.
    pub(super) fn cut(&mut self, kept: u64, size: u64) {
        let stale = &mut self.stale;
        self.granules.retain(|&n, _| {
            let stays = n <= kept && offset(n) < size;
            if !stays {
                stale.insert(n);
            }
            stays
        });
        self.held.retain(|&n, _| n <= kept && offset(n) < size);
        self.limit = self.limit.min(size);
    }
}

/// Recovers the file at `at` from its side files: one ending in a commit
/// block is copied into it, the file made if it is not there, or removed
/// if the batch removed it; the side files are then thrown away. A crash
/// can tear a block written over, so invalid bytes before valid blocks are
/// no damage in a side file with no commit block. One with a commit block
/// was synced whole before it, so there they are damage, whatever they
/// look like, and it is refused (SQLCODE -916).
pub(super) fn recover(at: &At) -> Result<()> {
    for which in 0..SIDES.len() {
        let side = side_path(&at.path, which);
        if !at.fs.exists(&side)? {
            continue;
        }
        let formats = (&[Arc::clone(&at.hash)][..], &[][..]);
        let (blocks, found) = BlockFile::open_uncut(&*at.fs, &side, formats, BlockOptions::default(), BlockId(0))?;
        let (ops, committed) = read_ops(&blocks)?;
        if let Some(next) = found.follows
            && (committed || ends_committed(&blocks)?)
        {
            return Err(damaged(&side, &found, next));
        }
        drop(blocks);
        if committed {
            apply(at, ops)?;
        }
        at.fs.remove(&side)?;
    }
    Ok(())
}

/// Whether the block that ends `blocks` is a commit block.
fn ends_committed(blocks: &BlockFile) -> Result<bool> {
    Ok(blocks.last_block()?.is_some_and(|last| matches!(decode(&last), Ok(Op::Commit))))
}

/// Copies the operations of a committed side file into the file at `at`:
/// removed, if they say so; else made if missing, cut to the limit, each
/// granule's newest content but those a cut made stale, the size.
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
    let (mut newest, mut size) = (BTreeMap::new(), None);
    for op in ops {
        match op {
            Op::Granule(first, content) => {
                for (number, granule) in (first..).zip(content.chunks(GRANULE)) {
                    newest.insert(number, granule.to_vec());
                }
            }
            Op::Stale(number) => {
                newest.remove(&number);
            }
            Op::Size(to) => size = Some(to),
            Op::Commit | Op::Limit(_) | Op::Remove => {}
        }
    }
    for (number, content) in newest {
        base.write_all_at(&content, offset(number))?;
    }
    if let Some(to) = size {
        base.set_size(to)?;
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
            op @ (Op::Granule(..) | Op::Size(_) | Op::Limit(_) | Op::Remove | Op::Stale(_)) => ops.push(op),
        }
    }
    Ok((ops, false))
}
