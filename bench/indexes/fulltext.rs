//! How fast a full-text index (`index/fulltext.rs`, RULES.md D86) takes
//! documents and answers TEXT_MATCHES: documents of twelve words each,
//! drawn so a few words are in most documents and most in few, added on
//! one and on several threads, then searched for a rare word, a common
//! one and both.

use std::sync::Arc;

use super::harness::{self, Bench, Case, on_threads, scatter};
use turborel::fulltext::matches::TextMatches;
use turborel::functions::ScalarRef;
use turborel::index::{FullTextMethod, Index, IndexMethod, Key, KeyOrder, Predicate, Probe};
use turborel::value::{DataType, Value};

/// The words of the `n`th search of a kind.
type Words = fn(u64) -> String;

/// Documents per thread.
const DOCUMENTS: u64 = 25_000;
/// Words of a document.
const WORDS: u64 = 12;
/// Distinct words.
const VOCABULARY: u64 = 4096;
/// Searches of each kind.
const SEARCHES: u64 = 2_000;
/// Frames: enough for every index.
const FRAMES: usize = 32_768;
/// Thread counts.
const THREADS: [usize; 3] = [1, 4, 8];

/// The threads that fill the index the searches read, of `CORPUS_THREADS`
/// times `DOCUMENTS` documents whatever the threads searching.
const CORPUS_THREADS: usize = 4;

/// The workloads, on each thread count: inserts into an empty index, then
/// searches of one index of a fixed size, so the work of a search does not
/// grow with the threads.
pub fn workloads(bench: &Bench) {
    for threads in THREADS {
        bench.measure_fresh(
            Case::new("insert, documents of 12 words, fulltext", "documents").threads(threads),
            || index(bench),
            |index| insert(&*index, threads, threads as u64 * DOCUMENTS),
        );
    }
    let total = CORPUS_THREADS as u64 * DOCUMENTS;
    let full: Arc<dyn Index> = Arc::from(index(bench));
    insert(&*full, CORPUS_THREADS, total);
    let kinds: [(&str, Words); 3] = [
        ("a rare word", |n| format!("w{}", VOCABULARY - 1 - n % 512)),
        ("a common word", |n| format!("w{}", n % 8)),
        ("a rare and a common word", |n| format!("w{} w{}", VOCABULARY - 1 - n % 512, n % 8)),
    ];
    for (kind, words) in kinds {
        for threads in THREADS {
            let searched = |t: usize| {
                for n in 0..SEARCHES {
                    let probe = matches(&words(t as u64 * SEARCHES + n));
                    assert!(Arc::clone(&full).search(&probe, KeyOrder::Ascending).map(Result::unwrap).count() <= total as usize);
                }
            };
            bench.measure(Case::new(format!("search, {kind}, 100,000 documents, fulltext"), "searches").threads(threads), || {
                on_threads(threads, searched);
                threads as u64 * SEARCHES
            });
        }
    }
}

/// A new, empty full-text index, in a new pager file and a pool that holds it.
fn index(bench: &Bench) -> Box<dyn Index> {
    let pool = harness::pool(FRAMES);
    let file = pool.add_pager(Arc::new(bench.pager("fulltext")));
    FullTextMethod::new(pool, file).create(&[DataType::varying(400)]).unwrap()
}

/// Inserts `DOCUMENTS` documents from each of `threads` threads, of rows
/// scattered among `total`; the documents inserted.
fn insert(index: &dyn Index, threads: usize, total: u64) -> u64 {
    on_threads(threads, |t| {
        for n in 0..DOCUMENTS {
            let row = scatter(t as u64 * DOCUMENTS + n, total);
            index.insert(Key(vec![Value::Char(document(row))]), row).unwrap();
        }
    });
    total
}

/// Document `row`: twelve words, word `w0` the most frequent and the
/// frequency falling as the number grows.
fn document(row: u64) -> String {
    let words: Vec<String> = (0..WORDS)
        .map(|w| {
            let drawn = ((row * WORDS + w).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32_u32) % VOCABULARY;
            format!("w{}", drawn * drawn / VOCABULARY)
        })
        .collect();
    words.join(" ")
}

/// The TEXT_MATCHES lookup of `words`.
fn matches(words: &str) -> Probe {
    let predicate = Box::new(Predicate { function: ScalarRef(Arc::new(TextMatches)), column: 0, argument: Some(0) });
    Probe::Predicate { predicate, value: Value::Char(words.to_owned()) }
}
