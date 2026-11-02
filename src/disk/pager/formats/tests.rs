//! Unit tests of the formats a pager file's pages are in, and the file
//! settled in them (RULES.md R-TEST-1, R-TEST-2). Split from
//! pager/formats.rs (RULES.md R-SIZE-2).

use std::path::Path;
use std::sync::Arc;

use super::super::named::tests::{Keyed, Shift, shift};
use super::super::{Extent, PageId, Pager};
use super::{Behind, Formats};
use crate::disk::encoding::{Encoding, EncodingId, LivePipeline, Pipeline};
use crate::disk::io::Durability;
use crate::disk::memory::MemoryFileSystem;
use crate::error::Code;

/// The formats of the pipeline `shift(id)` and the key `key`.
fn formats(id: u32, key: &str) -> Formats {
    Formats { encodings: vec![EncodingId(id)], key: key.to_owned() }
}

/// Verifies: [DESIGN] RULES.md D48: a file's pages in other formats than
/// the current are found by reading them raw, in runs: a data page in the
/// current ones, a page never written, a free page and the pager's own
/// pages are not listed, and the run ends at the file's end; the header
/// settled in the formats every page is in, the file reopens by the new
/// key alone, keyed by none other, and names the encodings settled; the
/// header is settled only in the current formats.
#[test]
fn pages_behind_the_formats_are_found_and_the_file_settled() {
    let (fs, path) = (MemoryFileSystem::new(), Path::new("/formats"));
    let pager = Pager::create(&fs, path, Keyed::by("k1"), Pipeline::none(), 4096).unwrap();
    assert_eq!(pager.settled_formats(), Formats { encodings: Vec::new(), key: "k1".into() });
    pager.allocate(6);
    for id in [0, 1, 2, 4] {
        pager.write(PageId(id), format!("page {id}").as_bytes()).unwrap();
    }
    // Page 3 is never written; page 4 is freed, so zeroed; page 5 is one
    // of the pager's own once the free list is recorded.
    pager.free(&[Extent { start: PageId(4), count: 1 }]).unwrap();
    pager.sync(Durability::None).unwrap();
    pager.set_pipeline(shift(5)).unwrap();
    pager.set_hash(Keyed::by("k2")).unwrap();
    assert_eq!(pager.current_formats(), formats(5, "k2"));
    pager.write(PageId(1), b"page 1, again").unwrap();
    let count = pager.page_count();
    assert_eq!(pager.behind(0, 4).unwrap(), Behind { again: vec![0, 2], next: 4 });
    assert_eq!(pager.behind(4, 100).unwrap(), Behind { again: Vec::new(), next: count });
    assert_eq!(pager.behind(count, 100).unwrap(), Behind { again: Vec::new(), next: count });
    assert!(!pager.settle(&formats(5, "k1")).unwrap(), "not the current formats");
    pager.write(PageId(0), b"page 0, again").unwrap();
    pager.write(PageId(2), b"page 2, again").unwrap();
    assert_eq!(pager.behind(0, 100).unwrap().again, Vec::<u64>::new());
    assert!(pager.settle(&formats(5, "k2")).unwrap());
    assert_eq!(pager.settled_formats(), formats(5, "k2"));
    pager.sync(Durability::None).unwrap();
    drop(pager);
    let with: Vec<Arc<dyn Encoding>> = vec![Arc::new(Shift(5))];
    let (again, _) = Pager::open(&fs, path, &[Keyed::holding("k2", &["k2"])], &with).unwrap();
    assert_eq!(again.settled_formats(), formats(5, "k2"));
    let mut out = Vec::new();
    again.read(PageId(2), &mut out).unwrap();
    assert_eq!(out, b"page 2, again");
    assert_eq!(Pager::open(&fs, path, &[Keyed::holding("k2", &["k2"])], &[]).err().map(|e| e.code), Some(Code::Api), "names shift");
}

/// Verifies: [DESIGN] RULES.md D48: a scratch file's pages name no
/// encodings a file settles in, its formats the key alone, and it is
/// never settled (SQLCODE -910): nothing opens it again.
#[test]
fn a_scratch_file_is_never_settled() {
    let fs = MemoryFileSystem::new();
    let live = Arc::new(LivePipeline::default());
    let scratch = Pager::create_scratch(&fs, Path::new("/scratch"), Keyed::by("k2"), &live, 4096).unwrap();
    assert_eq!(scratch.current_formats(), Formats { encodings: Vec::new(), key: "k2".into() });
    assert_eq!(scratch.settle(&scratch.current_formats()).err().map(|e| e.code), Some(Code::Api));
}
