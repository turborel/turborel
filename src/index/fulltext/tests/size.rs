//! A unit test of the space the index takes: postings of many short
//! documents of a few common words (RULES.md R-TEST-1).

use super::*;

/// The documents of the size test.
const DOCUMENTS: u64 = 100_000;
/// The most bytes of pages a posting may take, measured at 35.6 with
/// blocks of postings, numbered documents and their entries; one row of
/// entries for each posting took 72.4.
const MOST_BYTES_PER_POSTING: u64 = 38;

/// Verifies: [DESIGN] RULES.md D86, space: 100,000 short documents of a
/// few common words take at most `MOST_BYTES_PER_POSTING` bytes of pages
/// for each posting, a term in a document.
#[test]
fn postings_are_dense() {
    let (pool, file) = memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 8_i32), "a test").unwrap();
    let index = FullTextIndex::with(BLinkTree::create(Arc::clone(&pool), file).unwrap(), (english(), Weights::default()), spill(TEST_AREA));
    let words = ["apple", "banana", "cherry", "grape", "lemon", "mango", "peach", "plum"];
    let mut postings = 0_u64;
    for row in 0..DOCUMENTS {
        let text: Vec<&str> = (0..5_u64).map(|at| words[usize::try_from((row * 7 + at * 3) % 8).unwrap()]).collect();
        let mut distinct = text.clone();
        distinct.sort_unstable();
        distinct.dedup();
        postings += u64::try_from(distinct.len()).unwrap();
        index.insert(doc(&text.join(" ")), row).unwrap();
    }
    let bytes = pool.pages_in_use(file).unwrap() * u64::try_from(pool.capacity(file).unwrap()).unwrap();
    assert!(bytes <= MOST_BYTES_PER_POSTING * postings, "{bytes} bytes of pages for {postings} postings");
}
