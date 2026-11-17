//! Unit tests of the full-text index: counts of versions, postings written
//! once and taken back at the last count, places, stop words, term and
//! stem statistics, totals, and the projection of documents to hashes
//! (RULES.md R-TEST-1, R-TEST-2). Searches are in `tests/search.rs`; the
//! method, bulk loads, locators, threads and dropped indexes in
//! `tests/method.rs`; blocks of postings and cursors in `tests/blocks.rs`.

mod blocks;
mod method;
mod search;
mod size;
mod stop;
mod streams;

use std::ops::Bound;

use super::keys::{POSTING, STEM, stem_key};
use super::*;
use crate::engine::memory_file;
use crate::engine::tree::Direction;
use crate::fulltext::is_stop;
use crate::memory::MemoryBudget;
use crate::value::Value;
use crate::value::lob::Lob;

/// An index reading English, in memory.
fn index() -> Arc<FullTextIndex> {
    let (pool, file) = memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 4_i32), "a test").unwrap();
    Arc::new(FullTextIndex::with(BLinkTree::create(pool, file).unwrap(), (english(), Weights::default()), spill(TEST_AREA)))
}

/// How a test's index reads documents by default: in English, its stop
/// words the language's.
fn english() -> Reading {
    Reading { language: Language::English, stop: StopWords::Language }
}

/// The work area of a test's index: small, so documents of a few hundred
/// thousand words spill.
const TEST_AREA: Area = Area { bytes: 64 << 10, batch: 16 << 10 };

/// Where a test's index spills: to areas of its own budget, made when
/// first asked for, in an area of `area`.
fn spill(area: Area) -> Spill {
    Spill { work: Work::default(), budget: MemoryBudget::new(usize::MAX), area }
}

/// The key of the document `text`.
fn doc(text: &str) -> Key {
    Key(vec![Value::Char(text.into())])
}

/// Every entry of `index`'s tree, in key order.
fn dump(index: &FullTextIndex) -> Vec<(Vec<u8>, Vec<u8>)> {
    index.tree.scan(Bound::Unbounded, Bound::Unbounded, Direction::Forward).collect::<Result<_>>().unwrap()
}

/// The entries of `index`'s tree but its totals' entry while it counts no
/// document: what an index holds once every document is taken back, its
/// next document number kept.
fn left(index: &FullTextIndex) -> Vec<(Vec<u8>, Vec<u8>)> {
    let counted = |(key, value): &(Vec<u8>, Vec<u8>)| {
        key.as_slice() != keys::COUNTS_KEY || keys::Counts::decode(value).unwrap().totals != Totals::default()
    };
    dump(index).into_iter().filter(counted).collect()
}

/// The entries of `index`'s tree with tag `tag`.
fn tagged(index: &FullTextIndex, tag: u8) -> Vec<(Vec<u8>, Vec<u8>)> {
    dump(index).into_iter().filter(|(key, _)| key.first() == Some(&tag)).collect()
}

/// The count of `key`'s document in `row`, from its entry.
fn versions(index: &FullTextIndex, key: &Key, row: RowId) -> u32 {
    let hash = document::hash_in(key, &index.reading).unwrap().unwrap();
    let found = index.tree.get(&keys::document_key(keys::Doc { row, hash })).unwrap();
    found.map_or(0, |value| keys::DocumentValue::decode(&value).unwrap().count)
}

/// Verifies: [DESIGN] RULES.md D86, D29, `Index` counts: inserting one
/// document of a row twice, as two versions do, counts it twice and
/// writes its postings once; the first remove leaves them, the second
/// takes every entry back, and a remove with none left changes nothing.
#[test]
fn versions_are_counted_and_postings_written_once() {
    let index = index();
    let key = doc("apple banana apple");
    index.insert(key.clone(), 7).unwrap();
    let once = dump(&index);
    index.insert(key.clone(), 7).unwrap();
    assert_eq!(versions(&index, &key, 7), 2);
    let twice = dump(&index);
    assert_eq!(once.len(), twice.len(), "no entry added by a second version");
    assert_eq!(tagged(&index, POSTING), once.iter().filter(|(k, _)| k.first() == Some(&POSTING)).cloned().collect::<Vec<_>>());
    index.remove(&key, 7).unwrap();
    assert_eq!(versions(&index, &key, 7), 1);
    assert_eq!(tagged(&index, POSTING).len(), 2, "postings kept while a version holds them");
    index.remove(&key, 7).unwrap();
    assert_eq!(left(&index), Vec::new(), "every entry taken back with the last count");
    index.remove(&key, 7).unwrap();
    assert_eq!(left(&index), Vec::new());
}

/// Verifies: [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.1.5]; [DESIGN]
/// RULES.md D86, places: a term's postings give its frequency and each
/// place, ordinal, sentence, paragraph and offset, as the tokenizer gave
/// them, in order; a word the document lacks has none.
#[test]
fn places_round_trip() {
    let index = index();
    let text = "Apple banana apple.\n\nCherry apple";
    let key = doc(text);
    index.insert(key.clone(), 3).unwrap();
    let want: Vec<Position> = crate::fulltext::tokenize(text.chars().map(Ok), Language::English)
        .map(Result::unwrap)
        .filter(|t| t.word == "apple")
        .map(|t| Position { ordinal: t.ordinal, sentence: t.sentence, paragraph: t.paragraph, offset: t.offset })
        .collect();
    assert_eq!(want.len(), 3);
    let (frequency, places) = index.postings("apple", &key, 3).unwrap().unwrap();
    assert_eq!((frequency, places), (3, want));
    let projected = document::hash_key(&document::hash_of(&key.0[0], &index.reading).unwrap().unwrap());
    assert_eq!(index.postings("apple", &projected, 3).unwrap().unwrap().0, 3, "found by the projected key too");
    assert_eq!(index.postings("durian", &key, 3).unwrap(), None);
    assert_eq!(index.postings("apple", &key, 4).unwrap(), None, "another row");
}

/// Verifies: [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §4.4.2]; [DESIGN]
/// RULES.md D86: a stop word is not posted and has no statistics or stem,
/// but keeps its ordinal, so the word after it is second; the document's
/// length counts only words that are not stop words.
#[test]
fn stop_words_keep_ordinals_but_are_not_posted() {
    assert!(is_stop(Language::English, "the"), "the English stop words hold 'the'");
    let index = index();
    let key = doc("the apple");
    index.insert(key.clone(), 1).unwrap();
    assert_eq!(index.postings("the", &key, 1).unwrap(), None);
    assert_eq!(index.statistics("the").unwrap(), TermStatistics::default());
    let (_, places) = index.postings("apple", &key, 1).unwrap().unwrap();
    assert_eq!(places.iter().map(|p| p.ordinal).collect::<Vec<_>>(), [2]);
    assert_eq!(index.totals().unwrap(), Totals { documents: 1, length: 1 });
    assert_eq!(tagged(&index, POSTING).len(), 1);
}

/// Verifies: [DESIGN] RULES.md D86, statistics: a term's document
/// frequency and its stem's count rise with each document holding it and
/// fall as each is taken back, their entries gone at none; the most
/// frequency in one document is kept, an upper bound, after the document
/// holding it goes; versions count once.
#[test]
fn term_and_stem_counts_rise_and_fall() {
    let index = index();
    let (one, two) = (doc("apples apples apples"), doc("apples pie"));
    index.insert(one.clone(), 1).unwrap();
    index.insert(two.clone(), 2).unwrap();
    index.insert(two.clone(), 2).unwrap();
    assert_eq!(index.statistics("apples").unwrap(), TermStatistics { documents: 2, most: 3 });
    assert_eq!(index.statistics("pie").unwrap(), TermStatistics { documents: 1, most: 1 });
    let stem = stem_key(&index.stem_of(b"apples"), b"apples");
    let count = |of: &FullTextIndex| of.tree.get(&stem).unwrap().map(|v| keys::count_of(&v).unwrap());
    assert_eq!(count(&index), Some(2));
    index.remove(&one, 1).unwrap();
    assert_eq!(index.statistics("apples").unwrap(), TermStatistics { documents: 1, most: 3 }, "the most is an upper bound");
    assert_eq!(count(&index), Some(1));
    index.remove(&two, 2).unwrap();
    assert_eq!(count(&index), Some(1), "a version still holds it");
    index.remove(&two, 2).unwrap();
    assert_eq!(index.statistics("apples").unwrap(), TermStatistics::default());
    assert_eq!((count(&index), tagged(&index, STEM)), (None, Vec::new()));
}

/// Verifies: [DESIGN] RULES.md D86, totals: the documents and their
/// lengths summed rise with each document's first count and fall with
/// its last; versions add nothing; a null document indexes nothing.
#[test]
fn totals_count_documents_and_lengths() {
    let index = index();
    index.insert(doc("kiwi mango papaya"), 1).unwrap();
    index.insert(doc("lemon grape"), 2).unwrap();
    index.insert(doc("lemon grape"), 2).unwrap();
    index.insert(Key(vec![Value::Null]), 3).unwrap();
    assert_eq!(index.totals().unwrap(), Totals { documents: 2, length: 5 });
    index.remove(&doc("kiwi mango papaya"), 1).unwrap();
    assert_eq!(index.totals().unwrap(), Totals { documents: 1, length: 2 });
    index.remove(&Key(vec![Value::Null]), 3).unwrap();
    index.remove(&doc("lemon grape"), 2).unwrap();
    assert_eq!(index.totals().unwrap(), Totals { documents: 1, length: 2 });
    index.remove(&doc("lemon grape"), 2).unwrap();
    assert_eq!(index.totals().unwrap(), Totals::default());
    assert_eq!(left(&index), Vec::new());
}

/// Verifies: [DESIGN] RULES.md D85, D86, the projection: documents whose
/// tokens are alike, as by case or trailing spaces, project to one hash,
/// a large object as its string does; other documents to others; a null
/// document to null; the hash is 16 bytes, a binary string, so a reader
/// compares a fetched row's projection with an entry's.
#[test]
fn documents_project_to_the_hash_of_their_tokens() {
    let index = index();
    let projection = index.key_projection().unwrap();
    let project = |key: &Key| projection.project(key).unwrap();
    let apple = project(&doc("Apple pie"));
    assert_eq!(apple, project(&doc("apple pie   ")));
    assert_eq!(apple, project(&Key(vec![Value::Lob(Lob::of_text("Apple pie"))])));
    assert_ne!(apple, project(&doc("apple tart")));
    assert_ne!(apple, project(&doc("pie apple")));
    assert!(matches!(apple.0.as_slice(), [Value::Binary(b)] if b.bytes().len() == keys::HASH_BYTES));
    assert!(project(&Key(vec![Value::Null])).0[0].is_null());
    assert_eq!(projection.project(&Key(vec![Value::Bool(true)])).unwrap_err().code, Code::Api);
    assert_eq!(projection.project(&Key(Vec::new())).unwrap_err().code, Code::Api);
}

/// Verifies: [DESIGN] RULES.md D86: a word longer than a term keeps its
/// first bytes, cut where a character begins, and before any U+0000; two
/// long words alike in those share a term, so a search for either finds
/// both, as candidates; every key fits the tree.
#[test]
fn long_words_are_cut_at_a_character() {
    assert_eq!(document::term_of(&"é".repeat(300), MOST_TERM_BYTES).len(), 254);
    assert_eq!(document::term_of("ab\0cd", MOST_TERM_BYTES), b"ab");
    assert_eq!(document::term_of("abc", 2), b"ab");
    let index = index();
    let (long, longer) = ("a".repeat(1_000), format!("{}b", "a".repeat(999)));
    index.insert(doc(&long), 1).unwrap();
    index.insert(doc(&longer), 2).unwrap();
    assert!(dump(&index).iter().all(|(k, v)| k.len() <= index.tree.max_key() && k.len() + v.len() <= index.tree.max_entry()));
    assert_eq!(index.statistics(&long).unwrap().documents, 2);
    index.remove(&doc(&longer), 2).unwrap();
    index.remove(&doc(&long), 1).unwrap();
    assert_eq!(left(&index), Vec::new());
}

/// Verifies: [DESIGN] RULES.md D86, chunks: a term occurring 100,000
/// times in one document keeps its places past its block, in chunks, each
/// entry within the tree's limit, read back whole and in order, and taken
/// back whole.
#[test]
fn a_frequent_term_is_posted_in_chunks() {
    let index = index();
    let key = doc(&"word ".repeat(100_000));
    index.insert(key.clone(), 9).unwrap();
    let chunks = tagged(&index, keys::OVERFLOW);
    assert!(chunks.len() > 100, "{} chunks", chunks.len());
    assert!(chunks.iter().all(|(k, v)| k.len() + v.len() <= index.tree.max_entry()));
    assert_eq!(tagged(&index, POSTING).len(), 1);
    let (frequency, places) = index.postings("word", &key, 9).unwrap().unwrap();
    assert_eq!(frequency, 100_000);
    assert!(places.iter().map(|p| p.ordinal).eq(1..=100_000));
    assert_eq!(index.statistics("word").unwrap(), TermStatistics { documents: 1, most: 100_000 });
    index.remove(&key, 9).unwrap();
    assert_eq!(left(&index), Vec::new());
}
