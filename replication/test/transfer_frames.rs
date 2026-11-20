//! State transfer (RULES.md D57) robustness: each request a fetch makes
//! and each answer served, of the whole-files and the page-difference
//! transfers, cut short at any byte, is refused as a malformed frame, and
//! one of a part no transfer has is not answered or is refused.

use std::path::Path;
use std::sync::Arc;

use turborel::db::StateSource;
use turborel::disk::io::FileSystem;
use turborel::disk::memory::MemoryFileSystem;
use turborel::error::Code;
use turborel_replication::{Fetching, PageDifferences, ReplicaId, StateTransfer, WholeFiles};

use crate::transfer::filled;

/// The replica fetching.
const PRIMARY: ReplicaId = ReplicaId(1);
/// The replica serving.
const BACKUP: ReplicaId = ReplicaId(2);
/// The place of a request's part, after its tag and its fetch's number.
const ASKED_AT: usize = 9;
/// The place of an answer's part, after its tag, its fetch's number and
/// its export's.
const ANSWERED_AT: usize = 17;
/// The steps of a fetch taken, the manifest and the pieces after it.
const STEPS: usize = 6;

/// That `frame`, cut short at any byte, is refused by `read` as a
/// malformed frame received.
fn cut_short<T: std::fmt::Debug>(frame: &[u8], mut read: impl FnMut(&[u8]) -> turborel::error::Result<T>) {
    for end in 0..frame.len() {
        assert_eq!(read(&frame[..end]).err().map(|e| e.code), Some(Code::Connection), "cut at {end} of {}", frame.len());
    }
}

/// `STEPS` steps of `fetch` from `serving`, out of `export`: each request
/// and answer refused cut short, a request of no part not answered, and
/// an answer of no part refused.
fn each_step_refused_cut_short(fetch: &mut dyn Fetching, serving: &dyn StateTransfer, export: &mut dyn StateSource) {
    for _ in 0..STEPS {
        let Some(request) = fetch.request() else {
            fetch.waited();
            continue;
        };
        cut_short(&request, |cut| serving.serve(export, (BACKUP, 7), cut));
        let mut unknown = request.clone();
        unknown[ASKED_AT] = 99;
        assert_eq!(serving.serve(export, (BACKUP, 7), &unknown).unwrap(), None, "a request of no part");
        let answer = serving.serve(export, (BACKUP, 7), &request).unwrap().unwrap();
        cut_short(&answer, |cut| fetch.take(cut));
        let mut strange = answer.clone();
        strange[ANSWERED_AT] = 99;
        assert!(fetch.take(&strange).is_err(), "an answer of no part");
        if fetch.take(&answer).unwrap() {
            break;
        }
    }
}

/// Verifies: [DESIGN] RULES.md D57 robustness: the whole-files transfer
/// refuses a request or an answer cut short at any byte, of the manifest
/// and of pieces alike, answers no request of a part it has not, and
/// refuses an answer of one.
#[test]
fn whole_files_frames_cut_short_are_refused() {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    let source = filled(&fs, "/source", 100_i32);
    let mut export = source.export().unwrap();
    let serving = WholeFiles::new(Arc::clone(&fs), "/serving".into());
    let mut fetch = WholeFiles::new(Arc::clone(&fs), "/transfer".into()).begin(PRIMARY).unwrap();
    each_step_refused_cut_short(&mut *fetch, &serving, &mut *export);
}

/// Verifies: [DESIGN] RULES.md D57 robustness: the page-difference
/// transfer refuses a request or an answer cut short at any byte, of the
/// manifest, of summaries and of runs alike, answers no request of a part
/// it has not, and refuses an answer of one.
#[test]
fn page_difference_frames_cut_short_are_refused() {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    let source = filled(&fs, "/source", 100_i32);
    let mut export = source.export().unwrap();
    let serving = PageDifferences::new(Arc::clone(&fs), Path::new("/serving"));
    let mut fetch = PageDifferences::new(Arc::clone(&fs), Path::new("/nothing")).begin(PRIMARY).unwrap();
    each_step_refused_cut_short(&mut *fetch, &serving, &mut *export);
}
