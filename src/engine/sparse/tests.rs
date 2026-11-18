//! Sparse inverted index requirements: top-k and range searches equal to
//! brute force for every quantization, filtered, rescored or not; counted
//! versions; bulk loads equal to inserts; persistence through the pager;
//! maintenance; block codec bounds; and the compression it achieves.

use std::path::Path;

use super::block::{Header, Postings};
use super::*;
use crate::disk::encoding::Pipeline;
use crate::disk::io::Durability;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::hash::HashFunction;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::memory::MemoryBudget;
use crate::spill::{Area, WorkAreas};

mod codec;
mod search;

/// A page size small enough that a few thousand postings span many leaves.
const PAGE: u32 = 4096;

/// The database files a test's index lives in.
struct Files {
    fs: MemoryFileSystem,
    budget: Arc<MemoryBudget>,
}

impl Files {
    fn new() -> Files {
        let budget = MemoryBudget::new(usize::MAX);
        Files { fs: MemoryFileSystem::within(Arc::clone(&budget), "sparse test pages"), budget }
    }

    fn pool(&self, pager: Pager) -> (Arc<BufferPool>, FileId) {
        let pool = Arc::new(BufferPool::in_budget(&self.budget, (8 << 20_u32, 4096), "sparse test pool").unwrap());
        let file = pool.add_pager(Arc::new(pager));
        (pool, file)
    }

    fn create(&self, layout: Layout) -> (SparseTree, Arc<BufferPool>, FileId) {
        let pager = Pager::create(&self.fs, Path::new("/sparse"), crc(), Pipeline::none(), PAGE).unwrap();
        let (pool, file) = self.pool(pager);
        (SparseTree::create(Arc::clone(&pool), file, layout).unwrap(), pool, file)
    }

    fn reopen(&self, meta: PageId, layout: Layout) -> SparseTree {
        let (pager, _) = Pager::open(&self.fs, Path::new("/sparse"), &[crc()], &[]).unwrap();
        let (pool, file) = self.pool(pager);
        SparseTree::open(pool, file, meta, layout).unwrap()
    }
}

fn crc() -> Arc<dyn HashFunction> {
    Arc::new(Crc::new(CRC32_ISCSI).unwrap())
}

/// SplitMix64, from a fixed seed (R-TEST-5).
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next().checked_rem(n).unwrap()
    }

    fn unit(&mut self) -> f32 {
        f32::from(u16::try_from(self.next() >> 48).unwrap()) / 65_536.0
    }
}

/// Dimensions of a vocabulary of 3,000 terms, the common ones far more
/// often, as SPLADE's are.
const VOCABULARY: u32 = 3_000;

/// A sparse vector of 8 to 40 terms; with `signed`, some weights negative.
fn vector(random: &mut Random, signed: bool) -> SparseVector {
    let terms = 8_u64.wrapping_add(random.below(32));
    let mut dims = std::collections::BTreeMap::new();
    for _ in 0..terms {
        let skewed = random
            .below(u64::from(VOCABULARY))
            .wrapping_mul(random.below(u64::from(VOCABULARY)))
            .checked_div(u64::from(VOCABULARY))
            .unwrap();
        let mut weight = 0.05 + random.unit() * 3.0;
        if signed && random.below(4) == 0 {
            weight = -weight;
        }
        dims.insert(u32::try_from(skewed).unwrap(), weight);
    }
    SparseVector::new(VOCABULARY, dims).unwrap()
}

/// `n` vectors, the row of each its place times 3, so rows have gaps.
fn corpus(seed: u64, n: usize, signed: bool) -> Vec<(u64, SparseVector)> {
    let mut random = Random(seed);
    (0..n).map(|i| (u64::try_from(i).unwrap().wrapping_mul(3), vector(&mut random, signed))).collect()
}

fn insert_all(index: &SparseTree, corpus: &[(u64, SparseVector)]) {
    for (row, v) in corpus {
        index.insert(v, *row).unwrap();
    }
}

/// The best `k` of `corpus` for `query` kept by `keep`, by exact dot
/// product, ties to the lower row: what a search must find.
fn brute(corpus: &[(u64, SparseVector)], query: &SparseVector, k: usize, keep: &dyn Fn(u64) -> bool) -> Vec<(u64, f64)> {
    let mut all: Vec<(u64, f64)> = corpus.iter().filter(|(r, _)| keep(*r)).map(|(r, v)| (*r, v.dot(query))).collect();
    all.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    all.truncate(k);
    all
}

fn layouts() -> [Layout; 3] {
    [Quantization::Float32, Quantization::Float16, Quantization::Uint8].map(|quantization| Layout { quantization, block: DEFAULT_BLOCK })
}

fn work() -> WorkAreas {
    WorkAreas::new(MemoryBudget::new(usize::MAX))
}

/// Verifies: [DESIGN] A vector inserted twice is counted: one remove
/// leaves it found, the second takes its postings. Two versions of a row
/// with different vectors are one hit: unrescored, by the most of each
/// dimension over them, not exact; rescored, by the score the rescoring
/// gives. Removing one leaves the other's.
#[test]
fn counted_versions_are_found_until_their_last_remove() {
    let files = Files::new();
    let (index, ..) = files.create(Layout::default());
    let a = SparseVector::new(10, [(1, 2.0), (4, 1.0)]).unwrap();
    let b = SparseVector::new(10, [(1, 0.5), (7, 3.0)]).unwrap();
    let query = SparseVector::new(10, [(1, 1.0), (7, 1.0)]).unwrap();
    let rows = |tree: &SparseTree| {
        let found = tree.top_k(&query, 5, Ask::default(), &files.budget).unwrap();
        found.hits.iter().map(|h| h.row).collect::<Vec<_>>()
    };
    index.insert(&a, 7).unwrap();
    index.insert(&a, 7).unwrap();
    assert_eq!(index.documents(), Some(1));
    index.remove(&a, 7).unwrap();
    assert_eq!(rows(&index), [7]);
    index.insert(&b, 7).unwrap();
    assert_eq!(index.rows(&b.hash()).unwrap(), [7]);
    let found = index.top_k(&query, 5, Ask::default(), &files.budget).unwrap();
    assert_eq!(found.hits, [Hit { row: 7, score: 5.0, exact: false }], "a row is one hit, whatever its versions");
    let mut rescore = |row: u64| Ok(Some(f64::from(u32::try_from(row).unwrap()) / 2.0_f64));
    let rescored = index.top_k(&query, 5, Ask { filter: None, rescore: Some(&mut rescore) }, &files.budget).unwrap();
    assert_eq!(rescored.hits, [Hit { row: 7, score: 3.5, exact: true }]);
    index.remove(&b, 7).unwrap();
    assert_eq!(rows(&index), [7]);
    index.remove(&a, 7).unwrap();
    assert_eq!(rows(&index), Vec::<u64>::new());
    index.remove(&a, 7).unwrap();
    assert_eq!(index.documents(), Some(0));
    assert_eq!(index.entries().count(), 0);
}

/// Verifies: [DESIGN] An index written, synced and opened again from its
/// meta page in a new pager and pool holds the same entries and answers
/// the same searches (persistence through the pager, format version 1).
#[test]
fn an_index_reopened_from_its_pages_answers_alike() {
    let files = Files::new();
    let data = corpus(7, 800, false);
    let (index, pool, file) = files.create(Layout::default());
    insert_all(&index, &data);
    let query = vector(&mut Random(99), false);
    let before = index.top_k(&query, 10, Ask::default(), &files.budget).unwrap();
    let meta = index.meta();
    pool.sync(file, Durability::Full).unwrap();
    drop((index, pool));
    let again = files.reopen(meta, Layout::default());
    assert_eq!(again.documents(), None, "counted by maintenance");
    assert!(again.maintenance_due());
    assert!(again.maintain().unwrap());
    assert_eq!(again.documents(), Some(800));
    assert_eq!(again.top_k(&query, 10, Ask::default(), &files.budget).unwrap(), before);
    let mut entries: Vec<u64> = again.entries().map(|e| e.unwrap().1).collect();
    entries.sort_unstable();
    assert_eq!(entries, data.iter().map(|(r, _)| *r).collect::<Vec<_>>());
}

/// Verifies: [DESIGN] A bulk load leaves the index as inserting each entry
/// would: the same searches, entries and counts, an entry given twice
/// counted twice and its postings written once.
#[test]
fn a_bulk_load_answers_as_inserts_do() {
    let files = Files::new();
    let data = corpus(11, 1_500, true);
    let (inserted, ..) = files.create(Layout::default());
    insert_all(&inserted, &data);
    inserted.insert(&data[0].1, data[0].0).unwrap();
    let other = Files::new();
    let (loaded, ..) = other.create(Layout::default());
    let mut entries = data.iter().map(|(r, v)| Ok((v.clone(), *r))).chain(std::iter::once(Ok((data[0].1.clone(), data[0].0))));
    let areas = work();
    loaded.load(&mut entries, (&areas, Area { bytes: 64 << 10_u32, batch: 16 << 10_u32 })).unwrap();
    assert_eq!(loaded.documents(), Some(1_500));
    let mut random = Random(5);
    for _ in 0_u32..20 {
        let query = vector(&mut random, true);
        let mut exact = |row: u64| Ok(data.iter().find(|(r, _)| *r == row).map(|(_, v)| v.dot(&query)));
        let a = inserted.top_k(&query, 15, Ask { filter: None, rescore: Some(&mut exact) }, &files.budget).unwrap();
        let mut again = |row: u64| Ok(data.iter().find(|(r, _)| *r == row).map(|(_, v)| v.dot(&query)));
        let b = loaded.top_k(&query, 15, Ask { filter: None, rescore: Some(&mut again) }, &other.budget).unwrap();
        assert_eq!(a, b);
    }
    loaded.remove(&data[0].1, data[0].0).unwrap();
    assert_eq!(loaded.rows(&data[0].1.hash()).unwrap(), [data[0].0], "loaded twice, counted twice");
    let (a, b) = (inserted.footprint().unwrap(), loaded.footprint().unwrap());
    assert_eq!(a.postings, b.postings);
    assert!(b.blocks <= a.blocks, "a load fills its blocks: {a:?} {b:?}");
}

/// Verifies: [DESIGN] After most entries are removed, maintenance merges
/// the blocks they left small and narrows the dimensions' bounds, and
/// searches still equal brute force over the entries left.
#[test]
fn maintenance_merges_blocks_removes_leave_small() {
    let files = Files::new();
    let data = corpus(13, 3_000, false);
    let (index, ..) = files.create(Layout { quantization: Quantization::Uint8, block: 64 });
    insert_all(&index, &data);
    let before = index.footprint().unwrap();
    for (row, v) in data.iter().filter(|(r, _)| r % 4 != 0) {
        index.remove(v, *row).unwrap();
    }
    assert!(index.maintenance_due());
    assert!(index.maintain().unwrap());
    let after = index.footprint().unwrap();
    // Adjacent runs merged hold more than a block together, so a
    // dimension keeps at most one block under half full.
    let dimensions = u64::try_from(index.dimensions().unwrap().len()).unwrap();
    assert!(after.blocks < before.blocks, "{before:?} {after:?}");
    assert!(after.blocks <= dimensions.wrapping_add(after.postings.checked_div(32).unwrap()), "{after:?} {dimensions}");
    let left: Vec<_> = data.iter().filter(|(r, _)| r % 4 == 0).cloned().collect();
    let mut random = Random(17);
    for _ in 0_u32..10 {
        let query = vector(&mut random, false);
        let mut rescore = |row: u64| Ok(left.iter().find(|(r, _)| *r == row).map(|(_, v)| v.dot(&query)));
        let ask = Ask { filter: None, rescore: Some(&mut rescore) };
        let found = index.top_k(&query, 10, ask, &files.budget).unwrap();
        let got: Vec<_> = found.hits.iter().map(|h| (h.row, h.score)).collect();
        assert_eq!(got, brute(&left, &query, 10, &|_| true));
    }
}

/// Verifies: [DESIGN] Changing the quantization in place keeps the blocks
/// written before readable by their own headers, and maintenance stores
/// them again as set; searches stay exact under a rescoring throughout.
#[test]
fn a_quantization_changed_in_place_reads_old_blocks() {
    let files = Files::new();
    let data = corpus(19, 600, false);
    let (index, ..) = files.create(Layout { quantization: Quantization::Float32, block: DEFAULT_BLOCK });
    insert_all(&index, &data);
    index.set_layout(Layout { quantization: Quantization::Uint8, block: DEFAULT_BLOCK });
    let query = vector(&mut Random(23), false);
    let exact = brute(&data, &query, 10, &|_| true);
    let check = |tree: &SparseTree| {
        let mut rescore = |row: u64| Ok(data.iter().find(|(r, _)| *r == row).map(|(_, v)| v.dot(&query)));
        let found = tree.top_k(&query, 10, Ask { filter: None, rescore: Some(&mut rescore) }, &files.budget).unwrap();
        assert_eq!(found.hits.iter().map(|h| (h.row, h.score)).collect::<Vec<_>>(), exact);
    };
    check(&index);
    let before = index.footprint().unwrap();
    assert!(index.maintenance_due());
    while index.maintain().unwrap() {}
    let after = index.footprint().unwrap();
    assert!(after.bytes < before.bytes, "{before:?} {after:?}");
    for entry in index.scan(keys::tag_range(keys::POSTINGS)) {
        assert_eq!(Header::read(&entry.unwrap().1).unwrap().quantization, Quantization::Uint8);
    }
    check(&index);
}

/// Verifies: [DESIGN] The compression the block codec achieves, reported
/// for each quantization: bytes of blocks against 12 bytes a posting raw
/// (a u64 row and an f32 weight).
#[test]
fn compression_ratio_is_reported_for_each_quantization() {
    let data = corpus(29, 4_000, false);
    for layout in layouts() {
        let files = Files::new();
        let (index, ..) = files.create(layout);
        let entries = &mut data.iter().map(|(r, v)| Ok((v.clone(), *r)));
        index.load(entries, (&work(), Area { bytes: 1 << 20_u32, batch: 64 << 10_u32 })).unwrap();
        let f = index.footprint().unwrap();
        let float = |n: u64| f64::from(u32::try_from(n).unwrap());
        let ratio = float(f.raw) / float(f.bytes);
        println!(
            "{:?}: {} postings in {} blocks, {} bytes against {} raw, {ratio:.2}x, {:.2} bytes a posting",
            layout.quantization,
            f.postings,
            f.blocks,
            f.bytes,
            f.raw,
            12.0_f64 / ratio
        );
        let floor = match layout.quantization {
            Quantization::Float32 => 1.8_f64,
            Quantization::Float16 => 2.8_f64,
            Quantization::Uint8 => 3.8_f64,
        };
        assert!(ratio > floor, "{layout:?}: {ratio}");
    }
}

/// Verifies: [DESIGN] A block's header round-trips, and a block cut at
/// its halfway point never parts two postings of one row.
#[test]
fn blocks_are_never_cut_between_versions_of_a_row() {
    let mut p = Postings::single(5, 1.0);
    for (row, w) in [(5, 2.0), (6, 1.0), (6, 3.0), (6, 0.5), (9, 1.0)] {
        p.insert(row, w);
    }
    let at = p.halfway().unwrap();
    assert_ne!(p.rows[at - 1], p.rows[at]);
    let header = Header::read(&p.encode(Quantization::Uint8).unwrap()).unwrap();
    assert_eq!((header.count, header.last), (6, 9));
    assert_eq!(Postings::single(3, 1.0).halfway(), None);
}
