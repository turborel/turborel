//! Unit tests of full-text searches: TEXT_MATCHES's rows by intersecting
//! postings, words of stop words only, null words and documents, every
//! entry, the rows of a document, estimates, and probes refused (RULES.md
//! R-TEST-1, R-TEST-2).

use super::*;
use crate::fulltext::matches::TextMatches;
use crate::functions::ScalarRef;
use crate::index::searched;

/// The probe of TEXT_MATCHES(column, `words`).
fn matching(words: Value) -> Probe {
    let function = ScalarRef(Arc::new(TextMatches));
    Probe::Predicate { predicate: Box::new(Predicate { function, column: 0, argument: Some(0) }), value: words }
}

/// The rows `probe` finds, in row order: an index gives them in its
/// documents' numbers' order.
fn found(index: &Arc<FullTextIndex>, probe: &Probe) -> Vec<RowId> {
    let mut rows: Vec<RowId> = searched(index, probe).unwrap().into_iter().map(|(_, row)| row).collect();
    rows.sort_unstable();
    rows
}

/// An index of four rows' documents and a null one.
fn fruit() -> Arc<FullTextIndex> {
    let index = index();
    for (row, text) in [(4, "apple banana"), (2, "banana cherry"), (3, "Cherry, apple and banana"), (1, "durian")] {
        index.insert(doc(text), row).unwrap();
    }
    index.insert(doc("banana cherry"), 2).unwrap();
    index.insert(Key(vec![Value::Null]), 5).unwrap();
    index
}

/// Verifies: [EXTENSION] TEXT_MATCHES; [DESIGN] RULES.md D86, D34: the
/// rows found hold every word asked, in any case, by intersecting the
/// words' postings, each once, a row of two versions once;
/// a word no document holds finds none.
#[test]
fn every_word_is_intersected() {
    let index = fruit();
    let text = |s: &str| Value::Char(s.into());
    assert_eq!(found(&index, &matching(text("banana APPLE"))), [3, 4]);
    assert_eq!(found(&index, &matching(text("banana"))), [2, 3, 4]);
    assert_eq!(found(&index, &matching(text("cherry banana cherry"))), [2, 3]);
    assert_eq!(found(&index, &matching(text("apple elderberry"))), Vec::<RowId>::new());
    assert_eq!(found(&index, &matching(text("elderberry"))), Vec::<RowId>::new());
    assert_eq!(found(&index, &matching(Value::Lob(Lob::of_text("durian")))), [1], "words in a large object");
}

/// Verifies: [EXTENSION] TEXT_MATCHES's rows: each entry's key is its
/// document's projection, as a reader compares a fetched row's; so every
/// key found equals the projection of its row's document.
#[test]
fn entries_carry_projected_keys() {
    let index = fruit();
    let projection = index.key_projection().unwrap();
    for (key, row) in searched(&index, &matching(Value::Char("banana".into()))).unwrap() {
        let text =
            [(4, "apple banana"), (2, "banana cherry"), (3, "Cherry, apple and banana")].into_iter().find(|(r, _)| *r == row).unwrap().1;
        assert_eq!(key, projection.project(&doc(text)).unwrap(), "row {row}");
    }
}

/// Verifies: [EXTENSION] TEXT_MATCHES: words with no word that is not a
/// stop word, or none at all, find every document, as the function is
/// true of each; null words find none, as it is null; a null document is
/// never found.
#[test]
fn stop_words_only_find_every_document_and_null_finds_none() {
    let index = fruit();
    assert_eq!(found(&index, &matching(Value::Char("the and".into()))), [1, 2, 3, 4]);
    assert_eq!(found(&index, &matching(Value::Char(String::new()))), [1, 2, 3, 4]);
    assert_eq!(found(&index, &matching(Value::Null)), Vec::<RowId>::new());
    assert_eq!(found(&index, &Probe::All), [1, 2, 3, 4]);
}

/// Verifies: [DESIGN] RULES.md D86, `Index::rows` and `key_rows`: the
/// rows of a document, by the document or its projection, each once, a
/// row of two versions once; none for a null document or one not held.
#[test]
fn rows_of_a_document() {
    let index = index();
    for row in [3, 1, 2] {
        index.insert(doc("same text"), row).unwrap();
    }
    index.insert(doc("same text"), 1).unwrap();
    index.insert(doc("other text"), 4).unwrap();
    let projected = index.key_projection().unwrap().project(&doc("Same TEXT")).unwrap();
    for key in [doc("same text"), projected] {
        assert_eq!(index.rows(&key).unwrap(), [1, 2, 3]);
        assert_eq!(Arc::clone(&index).key_rows(&key).collect::<Result<Vec<_>>>().unwrap(), [1, 2, 3]);
    }
    assert_eq!(index.rows(&Key(vec![Value::Null])).unwrap(), Vec::<RowId>::new());
    assert_eq!(Arc::clone(&index).key_rows(&Key(vec![Value::Null])).count(), 0);
    assert_eq!(index.rows(&doc("missing")).unwrap(), Vec::<RowId>::new());
    assert_eq!(index.rows(&Key(Vec::new())).unwrap_err().code, Code::Api);
    assert!(Arc::clone(&index).key_rows(&Key(Vec::new())).next().unwrap().is_err());
}

/// Verifies: [DESIGN] RULES.md D86, estimates: of TEXT_MATCHES, the
/// fewest documents of a word asked, none of a word not held, every
/// document with no word but stop words, none for null words; of every
/// entry, the documents; not known for a comparison. The pages a probe
/// reads are the tree's levels.
#[test]
fn estimates_come_from_statistics() {
    let index = fruit();
    let estimate = |words: &str| index.estimate(&matching(Value::Char(words.into())));
    assert_eq!(estimate("banana"), Some(3));
    assert_eq!(estimate("banana durian"), Some(1));
    assert_eq!(estimate("elderberry banana"), Some(0));
    assert_eq!(estimate("the"), Some(4));
    assert_eq!(index.estimate(&matching(Value::Null)), Some(0));
    assert_eq!(index.estimate(&Probe::All), Some(4));
    assert_eq!(index.estimate(&Probe::Compare { op: CompOp::Eq, value: Value::Null }), None);
    assert_eq!(index.probe_pages(&Probe::All), Some(f64::from(index.tree.height().unwrap())));
}

/// Verifies: [EXTENSION] the fulltext method answers no comparison: a
/// comparison, prefix, range or columns probe gives SQLCODE -900, as an
/// entry; words not of a character string do too.
#[test]
fn comparison_probes_are_refused() {
    let index = fruit();
    let probes = [
        Probe::Compare { op: CompOp::Eq, value: Value::Char("apple".into()) },
        Probe::Prefix { equal: vec![Value::Char("apple".into())], then: None },
        Probe::Range { equal: Vec::new(), low: Box::new((CompOp::Gt, Value::Null)), high: Box::new((CompOp::Lt, Value::Null)) },
        Probe::Columns(Vec::new()),
        matching(Value::Bool(true)),
    ];
    for probe in &probes {
        let error = searched(&index, probe).unwrap_err();
        let want = if matches!(probe, Probe::Predicate { .. }) { Code::Api } else { Code::Unsupported };
        assert_eq!(error.code, want, "{probe:?}");
    }
}

/// Verifies: [DESIGN] RULES.md D86, a search reads a block at a time: of
/// many documents, over several levels, a search of two words finds just
/// those holding both.
#[test]
fn searches_span_many_leaves() {
    let index = index();
    for row in 0..3_000_u64 {
        let text = format!("{} {} filler{row}", if row % 2 == 0 { "even" } else { "odd" }, if row % 3 == 0 { "third" } else { "rest" });
        index.insert(doc(&text), row).unwrap();
    }
    assert!(index.tree.height().unwrap() > 1);
    let want: Vec<RowId> = (0..3_000).filter(|row| row % 6 == 0).collect();
    assert_eq!(found(&index, &matching(Value::Char("third even".into()))), want);
    assert_eq!(found(&index, &matching(Value::Char("filler42 even".into()))), [42]);
}
