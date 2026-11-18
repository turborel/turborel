//! Unit tests of CONTAINS answered by a full-text index: the rows found
//! are a superset of those Contains holds of on random corpora, a bad
//! pattern finds every row, masks read the dictionary within their
//! prefix, and FullText values in two languages share one index, their
//! projections differing by language (RULES.md R-TEST-1, R-TEST-2).

use super::*;
use crate::fulltext::types::{Contains, full_text};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::index::searched;
use crate::value::Decimal;

/// The probe of CONTAINS(column, `pattern`).
fn containing(pattern: &str) -> Probe {
    let function = ScalarRef(Arc::new(Contains));
    Probe::Predicate { predicate: Box::new(Predicate { function, column: 0, argument: Some(0) }), value: Value::Char(pattern.into()) }
}

/// The rows `pattern` finds in `index`, in row order.
fn found(index: &Arc<FullTextIndex>, pattern: &str) -> Vec<RowId> {
    let mut rows: Vec<RowId> = searched(index, &containing(pattern)).unwrap().into_iter().map(|(_, row)| row).collect();
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// Whether CONTAINS(`document`, `pattern`) is 1, as the recheck finds;
/// true if it raises an error, which the index leaves to the recheck.
fn holds(document: &Value, pattern: &str) -> bool {
    let Ok(Value::Exact(one)) = Contains.call(&[document.clone(), Value::Char(pattern.into())]) else { return true };
    one.cmp_value(&Decimal::new(1, 0)).is_eq()
}

/// A generator of the tests' corpora, the same each run.
struct Draw(u64);

impl Draw {
    /// The next number below `n`.
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13_u32;
        self.0 ^= self.0 >> 7_u32;
        self.0 ^= self.0 << 17_u32;
        usize::try_from(self.0.checked_rem(u64::try_from(n).unwrap()).unwrap()).unwrap()
    }
}

/// The words of the random documents: stop words, forms of one stem,
/// words alike by a prefix, and sentence and paragraph ends.
const WORDS: [&str; 18] = [
    "the", "and", "of", "jump", "jumped", "jumping", "jumps", "fox", "foxes", "brown", "quick", "quiet", "quilt", "dog", "lazy", ".",
    ".\n\n", "standard",
];

/// The patterns each corpus is searched by: words, masks, phrases with
/// stop words and optional words, stems, IS ABOUT, proximity, context,
/// NOT, `&` and `|`, and expansions answered as every row.
const PATTERNS: [&str; 26] = [
    r#""fox""#,
    r#""qui%""#,
    r#""qu_ck""#,
    r#""%o%""#,
    r#""the""#,
    r#""quick brown fox""#,
    r#""jumped the fox""#,
    r#""brown % fox""#,
    r#""the quick""#,
    r#"FORM OF "jumping""#,
    r#"FORM OF "jumping foxes""#,
    r#"IS ABOUT "the lazy dog""#,
    r#"("fox") NEAR ("dog") WITHIN 2 WORDS IN ORDER"#,
    r#"("fox") NEAR ("dog") WITHIN 1 WORDS"#,
    r#"("quick") NEAR ("lazy") WITHIN 0 SENTENCES"#,
    r#"("quick") NEAR ("lazy") WITHIN 1 PARAGRAPHS IN ORDER"#,
    r#"("fox") NEAR ("dog") WITHIN 6 CHARACTERS"#,
    r#""fox" IN SAME SENTENCE AS "dog""#,
    r#""quick" IN SAME PARAGRAPH AS "lazy""#,
    r#"NOT "fox""#,
    r#""dog" & NOT "fox""#,
    r#""dog" & "brown" | "quilt""#,
    r#"("dog" | "jump%") & "standard""#,
    r#"SOUNDS LIKE "fox""#,
    r#"FUZZY FORM OF "quick""#,
    r#""lazy" & NOT ("quick brown" | "the dog")"#,
];

/// A random document of `draw`, of up to twenty words.
fn random_text(draw: &mut Draw) -> String {
    let count = draw.below(20);
    let mut text = String::new();
    for _ in 0..count {
        text.push_str(WORDS.get(draw.below(WORDS.len())).unwrap());
        text.push(' ');
    }
    text
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.2]; [DESIGN] RULES.md D86, D85:
/// on random corpora of character strings and FullText values in two
/// languages, every row CONTAINS holds of is among the rows the index
/// finds, for words, masks, phrases, stems, IS ABOUT, proximity, context,
/// NOT, `&`, `|` and expansions; a row it finds and Contains does not is
/// left to the recheck.
#[test]
fn contains_finds_a_superset() {
    let mut draw = Draw(0x9E37_79B9_7F4A_7C15);
    for corpus in 0..4_u64 {
        let index = index();
        let mut documents = Vec::new();
        for row in 1..=60_u64 {
            let text = random_text(&mut draw);
            let document = match draw.below(3) {
                0 => Value::Char(text),
                1 => full_text(Language::English, &text).unwrap(),
                _ => full_text(Language::None, &text).unwrap(),
            };
            index.insert(Key(vec![document.clone()]), row).unwrap();
            documents.push((row, document));
        }
        for pattern in PATTERNS {
            let rows = found(&index, pattern);
            for (row, document) in &documents {
                if holds(document, pattern) {
                    assert!(rows.contains(row), "corpus {corpus}, {pattern}: row {row} {document:?} not found");
                }
            }
        }
    }
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.2]; [DESIGN] RULES.md D86: a
/// pattern that does not parse, of an unclosed literal or an unknown
/// language, or Contains refuses, of stop words only, finds every row, so the recheck raises its error as a scan
/// would; a null pattern finds none.
#[test]
fn a_bad_pattern_finds_every_row() {
    let index = index();
    for (row, text) in [(1, "a fox"), (2, "a dog"), (3, "")] {
        index.insert(doc(text), row).unwrap();
    }
    assert_eq!(found(&index, r#""fox"#), [1, 2, 3]);
    assert_eq!(found(&index, r#"FORM OF KLINGON "fox""#), [1, 2, 3]);
    assert_eq!(found(&index, ""), [1, 2, 3]);
    assert_eq!(found(&index, r#""the""#), [1, 2, 3], "of stop words only, 2FF03");
    assert_eq!(found(&index, r#""the" & "of""#), [1, 2, 3]);
    let null = Probe::Predicate {
        predicate: Box::new(Predicate { function: ScalarRef(Arc::new(Contains)), column: 0, argument: Some(0) }),
        value: Value::Null,
    };
    assert_eq!(searched(&index, &null).unwrap().len(), 0);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 5c]; [DESIGN]
/// RULES.md D86: a mask matches the dictionary's terms by LIKE, `_` one
/// character and `%` any; one with a literal prefix reads only the terms
/// of that prefix, so its rows are exact; a word no term matches finds
/// none.
#[test]
fn masks_read_terms_within_their_prefix() {
    let index = index();
    for (row, text) in [(1, "quick fox"), (2, "quiet dog"), (3, "quilt"), (4, "aquifer"), (5, "squid")] {
        index.insert(doc(text), row).unwrap();
    }
    assert_eq!(found(&index, r#""qui%""#), [1, 2, 3]);
    assert_eq!(found(&index, r#""qui_t""#), [2, 3]);
    assert_eq!(found(&index, r#""%qui%""#), [1, 2, 3, 4, 5]);
    assert_eq!(found(&index, r#""_qui%""#), [4, 5]);
    assert_eq!(found(&index, r#""zz%""#), Vec::<RowId>::new());
    let plan = index.plan(&crate::fulltext::pattern::parse(r#""qui%""#, Language::English).unwrap()).unwrap();
    let plan::Plan::Terms(terms) = plan else { panic!("{plan:?}") };
    assert_eq!(terms, [b"quick".to_vec(), b"quiet".to_vec(), b"quilt".to_vec()]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.9.5, §6.10.5]; [DESIGN] RULES.md
/// D86: SOUNDS LIKE finds the documents of a word equal to it or of its
/// Soundex code, FUZZY FORM OF those of a word within its edits, by the
/// dictionary, not every document; of a stop word, none; and Rank counts
/// the words they match.
#[test]
fn sounds_and_spellings_read_the_dictionary() {
    let index = index();
    for (row, text) in [(1, "a fox"), (2, "a dog"), (3, "the foxes"), (4, "fix it")] {
        index.insert(doc(text), row).unwrap();
    }
    assert_eq!(found(&index, r#"SOUNDS LIKE "fax""#), [1, 4], "fox and fix: F200");
    assert_eq!(found(&index, r#"FUZZY FORM OF "foxe""#), [1, 3]);
    assert_eq!(found(&index, r#"FUZZY FORM OF "dug""#), [2]);
    assert_eq!(found(&index, r#""fix" & SOUNDS LIKE "dug""#), Vec::<RowId>::new());
    let pattern = crate::fulltext::pattern::parse(r#"SOUNDS LIKE "fax""#, Language::English).unwrap();
    let rank = index.rank(&pattern, &Value::Char("fox fox".into()), &*index.snapshot().unwrap()).unwrap().unwrap();
    assert!(rank > 0.0_f64, "{rank}");
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.1, §5.1.9]; [DESIGN] RULES.md
/// D86, D85: one index holds FullText values in English and of no
/// language, each read in its own: an English stop word is posted in a
/// document of no language and found there, and FORM OF finds the
/// English forms; the projections of one text differ by its language,
/// and a character string's is the index language's.
#[test]
fn values_in_two_languages_share_an_index() {
    let index = index();
    let english = full_text(Language::English, "the jumping fox").unwrap();
    let none = full_text(Language::None, "the jumping fox").unwrap();
    index.insert(Key(vec![english.clone()]), 1).unwrap();
    index.insert(Key(vec![none.clone()]), 2).unwrap();
    index.insert(doc("the jumping fox"), 3).unwrap();
    assert_eq!(found(&index, r#""fox""#), [1, 2, 3]);
    assert!(found(&index, r#""the jumping""#).contains(&2));
    assert!(holds(&none, r#""the jumping""#));
    assert!(found(&index, r#"FORM OF "jumped""#).contains(&1));
    let projection = index.key_projection().unwrap();
    let of = |value: &Value| projection.project(&Key(vec![value.clone()])).unwrap();
    assert_ne!(of(&english), of(&none));
    assert_eq!(of(&english), of(&Value::Char("the jumping fox".into())));
}
