//! Unit tests of an index's stop words (`FULLTEXT.STOP_WORDS`): which
//! words a set posts, the places kept whatever the set, searches and the
//! projection under each set, and the option's values (RULES.md R-TEST-1,
//! R-TEST-2).

use super::*;
use crate::fulltext::matches::TextMatches;
use crate::functions::ScalarRef;
use crate::index::fulltext::options::stop_words_of;
use crate::index::searched;

/// An index in memory reading English with the stop words `stop`.
fn stopping(stop: StopWords) -> Arc<FullTextIndex> {
    let (pool, file) = memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 4_i32), "a test").unwrap();
    let reading = Reading { language: Language::English, stop };
    Arc::new(FullTextIndex::with(BLinkTree::create(pool, file).unwrap(), (reading, Weights::default()), spill(TEST_AREA)))
}

/// The words of `index`'s postings, each once, in order.
fn posted(index: &FullTextIndex) -> Vec<String> {
    let mut words: Vec<String> = tagged(index, POSTING)
        .into_iter()
        .map(|(key, _)| {
            let term = &key[1..key.len().saturating_sub(keys::BLOCK_OVERHEAD - 1)];
            String::from_utf8(term.to_vec()).unwrap()
        })
        .collect();
    words.dedup();
    words
}

/// The document of the tests.
const TEXT: &str = "the cat and the hat";

/// Verifies: [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §4.4.2]; [EXTENSION]
/// FULLTEXT.STOP_WORDS: the language's list leaves out "the" and "and";
/// a list of its own leaves out what it lists, "cat", and posts what the
/// language's leaves out; NONE posts every word. Each word keeps its
/// place whatever the set, and the length counts the words posted.
#[test]
fn each_set_posts_what_it_does_not_list() {
    let sets = [
        (StopWords::Language, vec!["cat", "hat"]),
        (StopWords::parse("cat, HAT", Language::English), vec!["and", "the"]),
        (StopWords::None, vec!["and", "cat", "hat", "the"]),
    ];
    for (stop, want) in sets {
        let index = stopping(stop.clone());
        let key = doc(TEXT);
        index.insert(key.clone(), 1).unwrap();
        assert_eq!(posted(&index), want, "{stop:?}");
        assert_eq!(index.totals().unwrap().length, u64::try_from(want.len()).unwrap() + u64::from(want.contains(&"the")), "{stop:?}");
        for (word, ordinals) in [("the", vec![1, 4]), ("cat", vec![2]), ("and", vec![3]), ("hat", vec![5])] {
            let found = index.postings(word, &key, 1).unwrap();
            let places = found.map(|(_, p)| p.iter().map(|p| p.ordinal).collect::<Vec<u32>>());
            assert_eq!(places, want.contains(&word).then_some(ordinals), "{word} under {stop:?}");
        }
        assert_eq!(index.stop_words(), &stop);
    }
}

/// Verifies: [EXTENSION] TEXT_MATCHES through an index of its own stop
/// words: a word the index lists is dropped from the search, so it never
/// empties the candidates; English stop words are dropped too, as the
/// function reads its words in English; what is left is intersected.
#[test]
fn searches_drop_the_indexs_stop_words() {
    let index = stopping(StopWords::parse("cat", Language::English));
    index.insert(doc(TEXT), 1).unwrap();
    index.insert(doc("a dog and a hat"), 2).unwrap();
    let matching = |words: &str| {
        let function = ScalarRef(Arc::new(TextMatches));
        let probe = Probe::Predicate {
            predicate: Box::new(Predicate { function, column: 0, argument: Some(0) }),
            value: Value::Char(words.into()),
        };
        searched(&index, &probe).unwrap().into_iter().map(|(_, row)| row).collect::<Vec<RowId>>()
    };
    assert_eq!(matching("cat hat"), [1, 2], "cat is not posted, so it does not narrow");
    assert_eq!(matching("dog the"), [2], "the is an English stop word");
    assert_eq!(matching("cat"), [1, 2]);
}

/// Verifies: [EXTENSION] RULES.md D85, FULLTEXT.STOP_WORDS and the
/// projection: a descriptor's projection equals its made index's under
/// each set; sets that mark a document's words differently project it
/// differently, as its hash covers which are stop words.
#[test]
fn projections_follow_the_stop_words() {
    let method = FullTextMethod::in_memory();
    let column = [DataType::varying(100)];
    let mut seen = Vec::new();
    for value in ["DEFAULT", "none", "cat,hat", " , "] {
        let (table, desc) = method::described(vec![(STOP_WORDS_OPTION.to_owned(), Value::Char(value.into()))]);
        let made = method.create_on(&column, (&table, &desc)).unwrap();
        let early = FullTextMethod::projection(&desc).unwrap();
        let projected = early.project(&doc(TEXT)).unwrap();
        assert_eq!(projected, made.key_projection().unwrap().project(&doc(TEXT)).unwrap(), "{value}");
        seen.push(projected);
    }
    assert_ne!(seen[0], seen[1]);
    assert_ne!(seen[1], seen[2]);
    assert_eq!(seen[1], seen[3], "a list of no word is NONE");
}

/// Verifies: [EXTENSION] FULLTEXT.STOP_WORDS's values: DEFAULT and NONE
/// in any case, a list by commas, normalized, and one of no word NONE;
/// the option is declared as text, DEFAULT by default, changed by a
/// rebuild; an index opened by its locator keeps its set.
#[test]
fn the_option_takes_default_none_or_words() {
    assert_eq!(stop_words_of(" default ", Language::English), StopWords::Language);
    assert_eq!(stop_words_of("None", Language::English), StopWords::None);
    assert_eq!(stop_words_of("Cat, hat ,", Language::English), StopWords::parse("cat,hat", Language::English));
    assert_eq!(stop_words_of(",,", Language::English), StopWords::None);
    let declared = FullTextMethod::in_memory().index_options();
    let option = declared.iter().find(|o| o.name == STOP_WORDS_OPTION).unwrap();
    assert!(matches!(option.kind, crate::settings::Kind::Text { .. }));
    assert!(matches!(&option.default, Value::Char(d) if d == DEFAULT_STOP_WORDS));
    assert_eq!(option.rebuild, crate::storage::Rebuild::Yes);
    let method = FullTextMethod::in_memory();
    let column = [DataType::varying(100)];
    for value in ["DEFAULT", "NONE", "cat,hat"] {
        let (table, desc) = method::described(vec![(STOP_WORDS_OPTION.to_owned(), Value::Char(value.into()))]);
        let made = method.create_on(&column, (&table, &desc)).unwrap();
        made.insert(doc(TEXT), 1).unwrap();
        let opened = method.open(&column, &made.locator()).unwrap().unwrap();
        assert_eq!(opened.locator(), made.locator(), "{value}");
        assert_eq!(
            opened.key_projection().unwrap().project(&doc(TEXT)).unwrap(),
            made.key_projection().unwrap().project(&doc(TEXT)).unwrap()
        );
        assert_eq!(opened.rows(&doc(TEXT)).unwrap(), [1]);
    }
}
