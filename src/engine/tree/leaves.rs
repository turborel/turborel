//! \[DESIGN\] (RULES.md D858) Copies of leaves that are read far more than
//! they are written, kept per thread. A read of a leaf took its frame's
//! latch, two writes of a word every reader of the leaf shares, so the
//! leaves every thread reads, a small table's or an index's of rows never
//! changed, moved one cache line between all of them. A leaf's copy is
//! read as an inner node's is (`inner.rs`, D502): while the leaf's frame
//! is at the version the copy was taken at (`disk/buffer/versions.rs`),
//! the copy is exactly the leaf's content, and the read takes no latch
//! and writes no shared word.
//!
//! A copy is safe by the version alone. The version moves on as the
//! frame's latch is taken exclusively, before a byte of the page changes
//! or the frame is given another page, and a copy is taken under the
//! latch held shared. So a copy whose version still holds was read whole,
//! between two writes, and no write has begun since. A read from it is a
//! read under the latch let go at that moment.
//!
//! A leaf is copied only once the thread has read it latched and found it
//! unchanged since its last read of it, `SEEN` times in a row. A leaf
//! found changed counts a strike, and each strike doubles the reads in a
//! row it then needs, so a leaf written as often as it is read is never
//! copied, and one written now and then is copied again only after many
//! reads that would each have used the copy.
//!
//! A thread keeps at most `KEPT` leaves, each in the place its page's
//! hash gives it, so a copy is found with one hash and no map. A leaf new
//! to a place takes it from another, but a copy read since the last leaf
//! asked for its place is spared once.

use std::cell::RefCell;
use std::rc::Rc;

use super::inner::Read;
use super::node::{Node, Parsed};
use crate::disk::buffer::{BufferPool, FileId, Stamp, Tally};
use crate::error::Result;

/// \[DESIGN\] The leaves a thread keeps track of, copied or not, across
/// every tree it reads: at most 1 MiB of copies of 8 KiB pages. A power of
/// two, as a place is the top bits of a hash.
pub(super) const KEPT: usize = 128;
/// The bits of a hash that are not a place.
const SHIFT: u32 = u64::BITS - KEPT.trailing_zeros();
/// \[DESIGN\] The latched reads in a row that must find a leaf unchanged
/// before it is copied, with no strike against it: the third read of a
/// leaf no one wrote copies it, as a scan's does (`near.rs`).
const SEEN: u32 = 2;
/// \[DESIGN\] The most strikes counted against a leaf: with them it needs
/// 128 unchanged reads in a row, which only a leaf read some hundred
/// times for each write passes.
const MOST_STRIKES: u32 = 6;

/// What a thread knows of one leaf: its page, the stamp of its last
/// latched read, the latched reads in a row that found that stamp, the
/// times it was found changed, and its copy with its header as parsed, if
/// it has one, with whether the copy was read since its place was asked
/// for.
struct Leaf {
    file: FileId,
    id: u64,
    stamp: Stamp,
    seen: u32,
    strikes: u32,
    copy: Option<(Rc<[u8]>, Parsed)>,
    used: bool,
}

impl Leaf {
    /// Notes a latched read of it with `stamp`, its content `page`: copied
    /// if it has been found unchanged often enough.
    fn read_latched(&mut self, stamp: Stamp, page: &[u8]) {
        if self.stamp == stamp {
            self.seen = self.seen.saturating_add(1);
        } else {
            // Written, or moved to another frame, since it was read.
            (self.stamp, self.seen, self.copy) = (stamp, 0, None);
            self.strikes = self.strikes.saturating_add(1).min(MOST_STRIKES);
        }
        let needed = SEEN.checked_shl(self.strikes).unwrap_or(u32::MAX);
        if self.seen < needed || self.copy.is_some() {
            return;
        }
        // Only a leaf in its tree is worth a copy; any other page a read
        // may be sent to is read latched.
        if let Ok(node) = Node::parse(page)
            && node.level == 0
            && !node.deleted
        {
            (self.copy, self.used) = (Some((Rc::from(page), node.parsed())), true);
        }
    }
}

thread_local! {
    static LEAVES: RefCell<Vec<Option<Leaf>>> = const { RefCell::new(Vec::new()) };
}

/// The place among a thread's leaves of the page whose hash is `hash`
/// (`BufferPool::page_hash`, XXH3-64 of its file and number, RULES.md
/// D460): its top bits. The pool places the page by the same hash, so a
/// read of a leaf hashes its page once (D951).
fn place(hash: u64) -> usize {
    usize::try_from(hash.checked_shr(SHIFT).unwrap_or_default()).unwrap_or_default()
}

/// The thread's copy of page `id` of `file`, whose hash is `hash`, if it
/// has one that is still the page's content.
pub(super) fn current_at(pool: &BufferPool, (file, id): (FileId, u64), hash: u64) -> Option<(Rc<[u8]>, Parsed)> {
    LEAVES.with(|leaves| {
        let mut leaves = leaves.try_borrow_mut().ok()?;
        let leaf = leaves.get_mut(place(hash))?.as_mut().filter(|leaf| (leaf.file, leaf.id) == (file, id))?;
        let (bytes, parsed) = leaf.copy.as_ref().filter(|_| pool.unchanged(file, id, leaf.stamp))?;
        let found = (Rc::clone(bytes), *parsed);
        leaf.used = true;
        Some(found)
    })
}

/// Notes that the thread read page `id` of `file`, whose hash is `hash`,
/// latched, with `stamp`, its content `page`, and copies it once it is
/// due (`Leaf::read_latched`).
pub(super) fn seen_at((file, id, stamp): (FileId, u64, Stamp), page: &[u8], hash: u64) {
    LEAVES.with(|leaves| {
        let Ok(mut leaves) = leaves.try_borrow_mut() else { return };
        if leaves.is_empty() {
            leaves.resize_with(KEPT, || None);
        }
        let Some(slot) = leaves.get_mut(place(hash)) else { return };
        if let Some(leaf) = slot.as_mut() {
            if (leaf.file, leaf.id) == (file, id) {
                return leaf.read_latched(stamp, page);
            }
            // A copy read since its place was last asked for is spared once.
            if leaf.copy.is_some() && leaf.used {
                leaf.used = false;
                return;
            }
        }
        *slot = Some(Leaf { file, id, stamp, seen: 0, strikes: 0, copy: None, used: false });
    });
}

/// Notes `read`, page `id` of `file` whose hash is `hash`, a leaf a
/// descent read: if it was read latched, as `seen_at` notes it.
pub(super) fn noted(read: &Read<'_>, (file, id): (FileId, u64), hash: u64) {
    if let Read::Held(page, stamp) = read {
        seen_at((file, id, *stamp), page, hash);
    }
}

/// Page `id` of `file` in `pool`, where a leaf is looked for: the thread's
/// copy if it is current, else the page latched, as `read_tallied` reads
/// it, and noted as read.
pub(super) fn read<'p>(pool: &'p BufferPool, (file, id): (FileId, u64), tally: Option<&Tally>) -> Result<Read<'p>> {
    let hash = BufferPool::page_hash(file, id);
    if let Some((bytes, parsed)) = current_at(pool, (file, id), hash) {
        return Ok(Read::Copied(bytes, parsed));
    }
    let (page, stamp) = pool.read_stamped_at((file, id), hash, tally)?;
    seen_at((file, id, stamp), &page, hash);
    Ok(Read::Held(page, stamp))
}

/// As `read`, of a page whose number is only a guess: none if no frame
/// holds it, and nothing read from the file (`BufferPool::read_resident`).
/// A copy still current is of a page a frame holds.
pub(super) fn read_resident(pool: &BufferPool, (file, id): (FileId, u64)) -> Result<Option<Read<'_>>> {
    let hash = BufferPool::page_hash(file, id);
    if let Some((bytes, parsed)) = current_at(pool, (file, id), hash) {
        return Ok(Some(Read::Copied(bytes, parsed)));
    }
    let Some(page) = pool.read_resident_at(file, id, hash)? else { return Ok(None) };
    let stamp = pool.stamp_of(&page);
    seen_at((file, id, stamp), &page, hash);
    Ok(Some(Read::Held(page, stamp)))
}

/// The pages the thread has copies of, for tests.
#[cfg(test)]
pub(super) fn copied() -> Vec<(FileId, u64)> {
    LEAVES.with(|leaves| leaves.borrow().iter().flatten().filter(|leaf| leaf.copy.is_some()).map(|leaf| (leaf.file, leaf.id)).collect())
}
