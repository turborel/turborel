//! \[DESIGN\] (RULES.md D502) Copies of inner nodes, kept per thread: a
//! descent reads an inner node from its copy while the node's frame is at
//! the version the copy was taken at (`disk/buffer/versions.rs`), and so
//! takes no latch and writes no shared word on the way down. Leis et al.,
//! "The ART of Practical Synchronization", 2016, section 3, validate a
//! node's version the same way, there over the node itself.
//!
//! A copy read is exactly the node's content now, as the version moves on
//! before any change, so a descent from it goes where one through the
//! latch would. Leaves are never copied: a read of one takes its latch.

use std::cell::RefCell;
use std::ops::Deref;
use std::rc::Rc;

use crate::disk::buffer::{BufferPool, FileId, PageRef, Stamp, Tally};
use crate::error::Result;

/// The copies a thread keeps: the root and the levels just below it of the
/// trees it reads most.
const KEPT: usize = 16;

/// A copy of page `id` of `file`, read with `stamp`.
struct Copy {
    file: FileId,
    id: u64,
    stamp: Stamp,
    bytes: Rc<[u8]>,
}

/// A thread's copies, and the next one to give way.
#[derive(Default)]
struct Copies {
    kept: Vec<Copy>,
    next: usize,
}

thread_local! {
    static COPIES: RefCell<Copies> = RefCell::default();
}

/// A node's page as a descent reads it: latched in its frame, or a copy.
pub(super) enum Read<'p> {
    Held(PageRef<'p>, Stamp),
    Copied(Rc<[u8]>),
}

impl Deref for Read<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Read::Held(page, _) => page,
            Read::Copied(bytes) => bytes,
        }
    }
}

/// Page `id` of `file` in `pool`: from a copy still current, if `copied`
/// and the thread keeps one; else latched, as `read_tallied` reads it.
pub(super) fn read<'p>(pool: &'p BufferPool, (file, id): (FileId, u64), tally: Option<&Tally>, copied: bool) -> Result<Read<'p>> {
    if let Some(bytes) = copied.then(|| current(pool, file, id)).flatten() {
        return Ok(Read::Copied(bytes));
    }
    let (page, stamp) = pool.read_stamped(file, id, tally)?;
    Ok(Read::Held(page, stamp))
}

/// The thread's copy of page `id` of `file`, if it is still current.
fn current(pool: &BufferPool, file: FileId, id: u64) -> Option<Rc<[u8]>> {
    COPIES.with(|copies| {
        let copies = copies.try_borrow().ok()?;
        let copy = copies.kept.iter().find(|c| c.file == file && c.id == id && pool.unchanged(file, id, c.stamp))?;
        Some(Rc::clone(&copy.bytes))
    })
}

/// Keeps a copy of `read`, page `id` of `file`, an inner node read
/// latched; one kept already of the page gives way to it.
pub(super) fn keep(read: &Read<'_>, (file, id): (FileId, u64)) {
    let Read::Held(page, stamp) = read else { return };
    let copy = Copy { file, id, stamp: *stamp, bytes: Rc::from(&page[..]) };
    COPIES.with(|copies| {
        let Ok(mut held) = copies.try_borrow_mut() else { return };
        let Copies { kept, next } = &mut *held;
        if let Some(old) = kept.iter_mut().find(|c| c.file == file && c.id == id) {
            *old = copy;
        } else if kept.len() < KEPT {
            kept.push(copy);
        } else if let Some(old) = kept.get_mut(*next) {
            *old = copy;
            *next = next.checked_add(1).map_or(0, |n| n.checked_rem(KEPT).unwrap_or(0));
        }
    });
}
