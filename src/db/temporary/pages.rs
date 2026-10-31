//! \[DESIGN\] Where an instance of a temporary table keeps its pages
//! (RULES.md D49): its rows and its entries of each index share one
//! scratch file of the database's temporary files (`ScratchFile`), as a
//! spill area does (D38), in a buffer pool of the session's
//! MEMORY.WORK_BYTES reserved of the memory budget; so what the pool does
//! not hold goes to the file, through the encodings spill files take,
//! encrypted while the database encrypts anything (D48). The file is made
//! when first needed, and removed once the instance is emptied or gone; a
//! crash leaves it among the temporary files the database clears as it
//! opens. Split from db/temporary/storage.rs (RULES.md R-SIZE-2).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::disk::buffer::{BufferPool, FileId};
use crate::error::Result;
use crate::memory::MemoryBudget;
use crate::spill::WorkAreas;
use crate::spill::paged::ScratchFile;
use crate::sync::lock_builtin;

/// \[DESIGN\] What an instance's pages are made of: the budget their pool
/// is reserved of, the work areas whose temporary files hold their file
/// and its spills, and the bytes of their pool, until resized.
#[derive(Clone)]
pub(crate) struct Place {
    pub(crate) budget: Arc<MemoryBudget>,
    pub(crate) work: Arc<WorkAreas>,
    pub(crate) bytes: usize,
}

/// \[DESIGN\] An instance's pages: none until first needed, then a
/// scratch file in a pool of its own, of the bytes last given, named for
/// the table `what`.
pub(super) struct Pages {
    pub(super) place: Place,
    bytes: AtomicUsize,
    what: String,
    made: Mutex<Option<Arc<ScratchFile>>>,
}

impl Pages {
    /// None yet, to be made at `place`, for the instance of table `what`.
    pub(super) fn new(place: &Place, what: &str) -> Pages {
        let what = format!("the pages of temporary table {what}");
        Pages { place: place.clone(), bytes: AtomicUsize::new(place.bytes), what, made: Mutex::default() }
    }

    /// Their pool made of `bytes`, if made after: its session's
    /// MEMORY.WORK_BYTES as its statement begins.
    pub(super) fn resize(&self, bytes: usize) {
        self.bytes.store(bytes, Ordering::Release);
    }

    /// The pool and file, made if they are not.
    ///
    /// # Errors
    ///
    /// SQLCODE -918 if the budget cannot grant the pool; SQLCODE -915 if
    /// the file cannot be made.
    pub(super) fn file(&self) -> Result<(Arc<BufferPool>, FileId)> {
        let mut made = lock_builtin(&self.made);
        if let Some(file) = made.as_ref() {
            return Ok((Arc::clone(file.pool()?), file.id()));
        }
        let temp = self.place.work.scratch();
        let file = Arc::new(ScratchFile::create(&self.place.budget, &temp, self.bytes.load(Ordering::Acquire), &self.what)?);
        let pool = Arc::clone(file.pool()?);
        let id = file.id();
        *made = Some(file);
        Ok((pool, id))
    }

    /// None again: the file removed, as nothing reads it once the rows and
    /// entries made in it are dropped.
    pub(super) fn emptied(&self) {
        *lock_builtin(&self.made) = None;
    }
}
