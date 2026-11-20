//! \[DESIGN\] A forward scan's entries read where they lie (RULES.md
//! D350). Each entry is shown to a visitor under its leaf's latch,
//! borrowed, and none is copied unless the visitor asks, so a scan that
//! reads most entries in place copies almost none of them. An entry the
//! visitor cannot read under the latch, as reading it would take other
//! pages or call a plug-in, is copied and given back once the latch is
//! released; the scan goes on past it from the same leaf, as it goes on
//! past the last entry it returned (`scan.rs`). A leaf read again after a
//! split still leads to every key past it by its right link (Lehman and
//! Yao 1981, §4).

use std::ops::{Bound, Deref};

use super::super::node::Node;
use super::super::{BLinkTree, Path, Target, corrupt};
use super::{Direction, Entry, Next, Scan, below, further_of, start_of};
use crate::disk::pager::PageId;
use crate::error::Result;

/// What a visitor does with an entry it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visit {
    /// It read the entry where it lies, and the scan goes on past it.
    Taken,
    /// It wants the entry out of the leaf. The scan copies it and gives
    /// it back (`Visited::Later`), going on past it.
    Later,
    /// It reads no more for now. The scan stops before the entry, to show
    /// it first at the next visit.
    Stop,
}

/// What a visit ended with.
#[derive(Debug, PartialEq, Eq)]
pub enum Visited {
    /// A leaf's entries were shown, or the visitor stopped, and more may
    /// follow.
    More,
    /// The entry the visitor wanted out, copied.
    Later(Entry),
    /// The scan has no entry left.
    Done,
}

/// What a visit read of a leaf.
struct Shown {
    /// The key to go on past, of the last entry taken or wanted out.
    resume: Option<Vec<u8>>,
    /// The entry wanted out, if one was.
    later: Option<Entry>,
    /// Whether every entry within the bounds was shown.
    whole: bool,
    /// Its right neighbour, if keys past its high key may be within the
    /// upper bound.
    further: Option<PageId>,
}

/// Shows `f` the entries of `leaf` within the bounds, from the first.
fn shown(leaf: &Node<'_>, (lower, upper): (&Bound<Vec<u8>>, &Bound<Vec<u8>>), f: &mut impl FnMut(&[u8]) -> Result<Visit>) -> Result<Shown> {
    if leaf.level != 0 {
        return Err(corrupt("a leaf's neighbour is not a leaf"));
    }
    let mut shown = Shown { resume: None, later: None, whole: true, further: leaf.right };
    // Taken out of the tree, its range is in the node its link leads to.
    if leaf.deleted {
        return Ok(shown);
    }
    let mut taken = None;
    for at in start_of(leaf, lower)?..leaf.len() {
        let (key, value) = leaf.entry(at)?;
        if !below(key, upper) {
            break;
        }
        match f(value)? {
            Visit::Taken => taken = Some(key),
            Visit::Later => {
                shown.later = Some((key.to_vec(), value.to_vec()));
                taken = Some(key);
                shown.whole = false;
                break;
            }
            Visit::Stop => {
                shown.whole = false;
                break;
            }
        }
    }
    shown.resume = taken.map(<[u8]>::to_vec);
    shown.further = further_of(leaf, upper);
    Ok(shown)
}

impl<T: Deref<Target = BLinkTree>> Scan<T> {
    /// Shows `f` the values of the entries it would give next, in order,
    /// a leaf at a time, each borrowed under its leaf's latch; `f` says
    /// what it does with each (`Visit`). Entries it copied already, or
    /// those of a scan backward, are each given back copied instead.
    ///
    /// # Errors
    ///
    /// As reading a leaf's, or `f`'s; the scan then ends.
    pub fn visit(&mut self, mut f: impl FnMut(&[u8]) -> Result<Visit>) -> Result<Visited> {
        if !self.ready.is_empty() || self.direction == Direction::Backward {
            return Ok(self.next().transpose()?.map_or(Visited::Done, Visited::Later));
        }
        let read = self.visit_leaf(&mut f);
        let (leaf, shown) = match read {
            Ok(Some(read)) => read,
            Ok(None) => return Ok(Visited::Done),
            Err(e) => {
                self.next = Next::Done;
                return Err(e);
            }
        };
        self.last = Some(leaf);
        if let Some(key) = shown.resume {
            self.lower = Bound::Excluded(key);
        }
        // Stopped inside it, it is read again from where it stopped.
        self.next = if shown.whole { shown.further.map_or(Next::Done, Next::Leaf) } else { Next::Leaf(leaf) };
        Ok(shown.later.map_or(Visited::More, Visited::Later))
    }

    /// Shows `f` the entries of the next leaf: the leaf, and what was
    /// shown; none if no leaf is left.
    fn visit_leaf(&mut self, f: &mut impl FnMut(&[u8]) -> Result<Visit>) -> Result<Option<(PageId, Shown)>> {
        let tree = &*self.tree;
        let bounds = (&self.lower, &self.upper);
        match self.next {
            Next::Done => Ok(None),
            Next::Leaf(id) => {
                if self.last.and_then(|last| last.0.checked_add(1)) == Some(id.0) {
                    tree.pool.ahead_of(&mut self.ahead, tree.file, id.0);
                }
                let page = tree.pool.read(tree.file, id.0)?;
                Ok(Some((id, shown(&Node::parse(&page)?, bounds, f)?)))
            }
            Next::Descend => {
                let target = match &self.lower {
                    Bound::Included(key) | Bound::Excluded(key) => Target::Key(key),
                    Bound::Unbounded => Target::First,
                };
                // A lookup is its first descent, to a key.
                let tallied = self.last.is_none() && matches!(target, Target::Key(_));
                let mut path = Path { tallied, ..Path::default() };
                tree.descend(target, 0, &mut path, |leaf| shown(leaf, bounds, f)).map(Some)
            }
        }
    }
}
