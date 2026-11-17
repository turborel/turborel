//! Unit tests of documents read as they stream, far larger than the work
//! area their places are sorted in, and of one row's document inserted
//! and removed by threads at once (RULES.md R-TEST-1, R-TEST-2, D38).

use std::fmt::Write;
use std::thread;

use super::*;
use crate::value::lob::Lob;

/// Words of the large document: each appears this many times over.
const VOCABULARY: usize = 500;
/// The large document's words.
const WORDS: usize = 1_000_000;

/// Verifies: [DESIGN] RULES.md D38, D86: a document of 9 MB, a character
/// large object of a million words, its places, about 21 MB of records,
/// over three hundred times the 64 KiB work area they are sorted in, is posted and taken
/// back whole: each word's frequency and places, its statistics, the
/// totals, then nothing left.
#[test]
fn a_document_far_larger_than_the_work_area_streams() {
    let mut text = String::with_capacity(WORDS * 9);
    for at in 0..WORDS {
        write!(text, "word{:04} ", at % VOCABULARY).unwrap();
    }
    assert!(text.len() > 100 * TEST_AREA.bytes);
    let key = Key(vec![Value::Lob(Lob::of_text(&text))]);
    drop(text);
    let (pool, file) = memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 6_i32), "a test").unwrap();
    let tree = BLinkTree::create(pool, file).unwrap();
    let index = FullTextIndex::with(tree, (english(), Weights::default()), spill(TEST_AREA));
    index.insert(key.clone(), 1).unwrap();
    let each = u32::try_from(WORDS.checked_div(VOCABULARY).unwrap()).unwrap();
    assert_eq!(index.totals().unwrap(), Totals { documents: 1, length: u64::try_from(WORDS).unwrap() });
    assert_eq!(index.statistics("word0007").unwrap(), TermStatistics { documents: 1, most: each });
    let (frequency, places) = index.postings("word0007", &key, 1).unwrap().unwrap();
    assert_eq!(frequency, each);
    let ordinals: Vec<u32> = places.iter().map(|p| p.ordinal).collect();
    let want: Vec<u32> = (0..each).map(|n| n * 500 + 8).collect();
    assert_eq!(ordinals, want);
    assert!(tagged(&index, POSTING).iter().all(|(k, v)| k.len() + v.len() <= index.tree.max_entry()));
    index.remove(&key, 1).unwrap();
    assert_eq!(left(&index), Vec::new());
}

/// The inserts and removes of one thread of the race below.
const ROUNDS: u32 = 300;

/// Verifies: [DESIGN] RULES.md D86, D29, the row stripes: two threads
/// inserting and removing one row's document at once, as a writer and
/// garbage collection may, a remove taking the last count while an insert
/// makes the first, never lose or leave a posting: once both are done,
/// nothing is left; one insert more leaves exactly what one insert into an
/// empty index does, its statistics and totals of one document.
#[test]
fn one_rows_inserts_and_removes_at_once_stay_consistent() {
    let index = index();
    let key = doc("apple banana apple cherry");
    let handles: Vec<_> = (0_i32..2_i32)
        .map(|_| {
            let (index, key) = (Arc::clone(&index), key.clone());
            thread::spawn(move || churn(&index, &key))
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(left(&index), Vec::new());
    index.insert(key.clone(), 5).unwrap();
    let alone = super::index();
    alone.insert(key.clone(), 5).unwrap();
    for tag in [keys::HASHED, keys::STATISTIC, STEM] {
        assert_eq!(tagged(&index, tag), tagged(&alone, tag));
    }
    for tag in [keys::DOCUMENT, keys::NUMBER, POSTING] {
        assert_eq!(tagged(&index, tag).len(), tagged(&alone, tag).len(), "numbered anew, so only counted");
    }
    for word in ["apple", "banana", "cherry"] {
        assert_eq!(index.postings(word, &key, 5).unwrap(), alone.postings(word, &key, 5).unwrap());
    }
    assert_eq!(index.statistics("apple").unwrap(), TermStatistics { documents: 1, most: 2 });
    assert_eq!(index.totals().unwrap(), Totals { documents: 1, length: 4 });
}

/// Inserts `key`'s document in row 5, then removes it, `ROUNDS` times.
fn churn(index: &FullTextIndex, key: &Key) {
    for _ in 0..ROUNDS {
        index.insert(key.clone(), 5).unwrap();
        index.remove(key, 5).unwrap();
    }
}
