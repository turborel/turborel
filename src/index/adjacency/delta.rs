//! \[DESIGN\] What an adjacency index wrote since its compacted adjacency
//! was built (RULES.md D141): the delta, kept as the vertices
//! whose lists were written, by direction. A reader of a vertex not in it
//! reads the compacted adjacency, which holds that vertex's list as the
//! tree does; of a vertex in it, the tree, which holds the compacted
//! entries and the delta's alike. So every read gives exactly the tree's
//! entries, the compacted ones and those written since merged in the
//! tree's order.
//!
//! The vertices are kept in stripes by their key's hash, each with the
//! compacted adjacency its readers use, so a reader takes one stripe's
//! lock, shared, and finds both at once. A writer marks a vertex after it
//! writes the tree: a reader in between may read the list the compacted
//! adjacency holds, without an entry not yet written or with one being
//! taken out, as a search may meet an entry written while it reads
//! (`Index`), and each edge row is checked by its reader (D34).
//!
//! A build marks in a second set from its start, before it reads the
//! tree, so a vertex written while it reads is marked in it; the build
//! then puts itself and that set in each stripe's place, one stripe at a
//! time, so a stripe's readers never see one without the other.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use super::csr::Csr;
use super::keys::{corrupt, tag};
use crate::error::Result;
use crate::graph::adjacency::Direction;
use crate::sync::{read, write};

/// \[DESIGN\] Stripes of vertices, so writers of different vertices meet
/// on one lock seldom: fixed, whatever the index's size.
const STRIPES: u64 = 64;
/// \[DESIGN\] Vertices marked past those of the compacted adjacency, at
/// which it is dropped: nearly every list is read from the tree by then,
/// and the marks would grow without bound until maintenance builds again.
pub const LEAST_MARKS: usize = 1 << 10;

/// The vertices of one stripe written since the compacted adjacency its
/// readers use was built, and since a build began, each as its
/// direction's tag and its key's bytes.
#[derive(Default)]
struct Stripe {
    csr: Option<Arc<Csr>>,
    written: HashSet<Vec<u8>>,
    building: Option<HashSet<Vec<u8>>>,
}

/// The delta of an index: its stripes, and how many vertices they mark.
pub struct Delta {
    stripes: Vec<RwLock<Stripe>>,
    marked: AtomicUsize,
}

/// A vertex's mark: its direction's tag, then its key's bytes.
fn mark_of(direction: Direction, vertex: &[u8]) -> Vec<u8> {
    let mut mark = Vec::with_capacity(vertex.len().saturating_add(1));
    mark.push(tag(direction));
    mark.extend_from_slice(vertex);
    mark
}

impl Delta {
    /// A delta of no compacted adjacency.
    pub fn new() -> Delta {
        let stripes = (0..STRIPES).map(|_| RwLock::new(Stripe::default())).collect();
        Delta { stripes, marked: AtomicUsize::new(0) }
    }

    /// The stripe of `vertex`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is missing, which it never is.
    fn stripe(&self, vertex: &[u8]) -> Result<&RwLock<Stripe>> {
        let hash = crate::hash::xxhash::xxh3_64(vertex);
        let at = usize::try_from(hash.checked_rem(STRIPES).unwrap_or_default()).unwrap_or_default();
        self.stripes.get(at).ok_or_else(corrupt)
    }

    /// Marks `vertex` written in `direction`; drops the compacted
    /// adjacency if the marks pass `LEAST_MARKS` more than its vertices.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn mark(&self, direction: Direction, vertex: &[u8]) -> Result<()> {
        let mark = mark_of(direction, vertex);
        let most = {
            let mut stripe = write(self.stripe(vertex)?)?;
            if let Some(building) = stripe.building.as_mut() {
                building.insert(mark.clone());
            }
            let Some(vertices) = stripe.csr.as_ref().map(|c| c.vertices()) else { return Ok(()) };
            if !stripe.written.insert(mark) {
                return Ok(());
            }
            vertices.saturating_add(LEAST_MARKS)
        };
        if self.marked.fetch_add(1, Ordering::AcqRel) >= most {
            self.drop_csr()?;
        }
        Ok(())
    }

    /// The compacted adjacency to read `vertex`'s list in `direction` of,
    /// if one is built and the vertex was not written since.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn csr(&self, direction: Direction, vertex: &[u8]) -> Result<Option<Arc<Csr>>> {
        let stripe = read(self.stripe(vertex)?)?;
        let Some(csr) = stripe.csr.as_ref() else { return Ok(None) };
        Ok((!stripe.written.contains(&mark_of(direction, vertex))).then(|| Arc::clone(csr)))
    }

    /// The compacted adjacency built, if any, for what it holds.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn current(&self) -> Result<Option<Arc<Csr>>> {
        let Some(first) = self.stripes.first() else { return Ok(None) };
        Ok(read(first)?.csr.clone())
    }

    /// The vertices marked since the compacted adjacency was built.
    pub fn marked(&self) -> usize {
        self.marked.load(Ordering::Acquire)
    }

    /// Begins a build: from now, a vertex written is marked for it too.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn begin(&self) -> Result<()> {
        for stripe in &self.stripes {
            write(stripe)?.building = Some(HashSet::new());
        }
        Ok(())
    }

    /// Puts `csr`, built, in each stripe's place, with the vertices marked
    /// since its build began: none if no build began, as of a load, which
    /// nothing writes meanwhile.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn install(&self, csr: &Arc<Csr>) -> Result<()> {
        let mut marked = 0_usize;
        for stripe in &self.stripes {
            let mut stripe = write(stripe)?;
            stripe.csr = Some(Arc::clone(csr));
            stripe.written = stripe.building.take().unwrap_or_default();
            marked = marked.saturating_add(stripe.written.len());
        }
        self.marked.store(marked, Ordering::Release);
        Ok(())
    }

    /// Ends a build that did not finish, its marks forgotten.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn abandon(&self) -> Result<()> {
        for stripe in &self.stripes {
            write(stripe)?.building = None;
        }
        Ok(())
    }

    /// Drops the compacted adjacency: every list is read from the tree.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn drop_csr(&self) -> Result<()> {
        for stripe in &self.stripes {
            let mut stripe = write(stripe)?;
            stripe.csr = None;
            stripe.written = HashSet::new();
        }
        self.marked.store(0, Ordering::Release);
        Ok(())
    }
}
