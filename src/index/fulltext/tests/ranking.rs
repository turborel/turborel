//! Unit tests of Rank by an index's ranking: BM25 against a hand
//! computation, a ranking of another order changing the top documents,
//! top-k against a full sort, a ranking past its bound refused, and the
//! option FULLTEXT.RANKING kept, changed in place and refused when unknown
//! (RULES.md R-TEST-1, R-TEST-2).

use super::method::described;
use super::*;
use crate::fulltext::pattern::{Pattern, parse};
use crate::fulltext::rank::{Corpus, InDocument, NoIndex, Statistics, TermStats};
use crate::storage::Rebuild;

/// A test's ranking by plain term frequency: `tf · ln(1 + N / df)`, its
/// bound the weight at the greatest frequency. Unlike BM25 it does not
/// saturate, nor normalize by length.
struct TfIdf;

impl Ranking for TfIdf {
    fn name(&self) -> &'static str {
        "TFIDF"
    }
    fn weight(&self, corpus: &Corpus, term: &TermStats, in_doc: &InDocument) -> f64 {
        f64::from(in_doc.frequency) * idf(corpus, term)
    }
    fn bound(&self, corpus: &Corpus, term: &TermStats) -> f64 {
        f64::from(term.most) * idf(corpus, term)
    }
}

/// TfIdf's inverse document frequency.
fn idf(corpus: &Corpus, term: &TermStats) -> f64 {
    let documents = f64::from(u32::try_from(corpus.documents).unwrap());
    (1.0_f64 + documents / f64::from(u32::try_from(term.documents).unwrap())).ln()
}

/// A ranking that breaks its contract: a weight ten times its bound.
struct Liar;

impl Ranking for Liar {
    fn name(&self) -> &'static str {
        "LIAR"
    }
    fn weight(&self, corpus: &Corpus, term: &TermStats, in_doc: &InDocument) -> f64 {
        Bm25.weight(corpus, term, in_doc) * 10.0_f64 + 1.0_f64
    }
    fn bound(&self, corpus: &Corpus, term: &TermStats) -> f64 {
        Bm25.bound(corpus, term)
    }
}

/// The rankings of the tests: BM25, TfIdf and the liar.
fn registry() -> Rankings {
    Arc::new(|| {
        let rankings: [Arc<dyn Ranking>; 3] = [Arc::new(Bm25), Arc::new(TfIdf), Arc::new(Liar)];
        rankings.to_vec()
    })
}

/// An index ranking by `name` of the tests' rankings, which it may name.
fn ranking_by(name: &str) -> Arc<FullTextIndex> {
    let (pool, file) = memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 4_i32), "a test").unwrap();
    let mut index = FullTextIndex::with(BLinkTree::create(pool, file).unwrap(), (english(), Weights::default()), spill(TEST_AREA));
    index.ranking = Mutex::new(named(&registry(), name).unwrap());
    index.rankings = registry();
    Arc::new(index)
}

/// The character string `text`.
fn text(text: &str) -> Value {
    Value::Char(text.into())
}

/// `text` read as an English pattern.
fn pattern(text: &str) -> Pattern {
    parse(text, Language::English).unwrap()
}

/// The ranks and rows the index's top-k gives for `text` by `statistics`,
/// best first.
fn top(index: &Arc<FullTextIndex>, text: &str, statistics: Option<Arc<dyn Statistics>>) -> Vec<(f64, RowId)> {
    let budget = MemoryBudget::new(usize::MAX);
    let found = Arc::clone(index).ranked(&Value::Char(text.into()), statistics, &budget).unwrap().unwrap();
    found.map(|entry| entry.map(|(rank, _, row)| (rank, row))).collect::<Result<_>>().unwrap()
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.3, §4.3]; [EXTENSION] BM25: over
/// three documents of lengths 2, 3 and 1, the rank of "fox" in the
/// first, by k1 1.2 and b 0.75, is the hand computation; stop words are
/// not counted in its length; a word under NOT adds nothing; with no
/// index, every inverse document frequency is 1.
#[test]
fn bm25_is_the_hand_computation() {
    let index = ranking_by("BM25");
    index.insert(doc("fox fox"), 1).unwrap();
    index.insert(doc("the fox dog cat"), 2).unwrap();
    index.insert(doc("dog"), 3).unwrap();
    // N = 3, df(fox) = 2, average length (2 + 3 + 1) / 3 = 2.
    let idf = (1.0_f64 + (3.0_f64 - 2.0_f64 + 0.5_f64) / (2.0_f64 + 0.5_f64)).ln();
    let first = idf * 2.0_f64 * 2.2_f64 / (2.0_f64 + 1.2_f64 * (0.25_f64 + 0.75_f64 * 2.0_f64 / 2.0_f64));
    let second = idf * 1.0_f64 * 2.2_f64 / (1.0_f64 + 1.2_f64 * (0.25_f64 + 0.75_f64 * 3.0_f64 / 2.0_f64));
    let snapshot = index.snapshot().unwrap();
    let rank = |asked: &str, document: &str| index.rank(&pattern(asked), &text(document), &*snapshot).unwrap().unwrap();
    assert!((rank(r#""fox""#, "fox fox") - first).abs() < 1e-12_f64, "{} {first}", rank(r#""fox""#, "fox fox"));
    assert!((rank(r#""fox""#, "the fox dog cat") - second).abs() < 1e-12_f64);
    assert!((rank(r#""fox" & NOT "dog""#, "the fox dog cat") - second).abs() < 1e-12_f64);
    assert_eq!(rank(r#""cow""#, "fox fox"), 0.0_f64);
    let none = NoIndex { k1: DEFAULT_K1, b: DEFAULT_B };
    let unindexed = index.rank(&pattern(r#""fox""#), &text("fox fox"), &none).unwrap().unwrap();
    assert!((unindexed - 2.0_f64 * 2.2_f64 / (2.0_f64 + 1.2_f64)).abs() < 1e-12_f64);
    let bound = Bm25.bound(&snapshot.corpus(), &TermStats { documents: 2, most: 2 });
    assert!(first <= bound, "the bound, of length 0, bounds every length");
}

/// Verifies: [EXTENSION] RULES.md R-SAFE-3, BM25's bound: of every
/// frequency up to its greatest and every length, the weight is at most
/// the bound, for weights across their ranges.
#[test]
fn bm25_weights_stay_within_their_bound() {
    let weights = [(0.0_f64, 0.0_f64), (1.2_f64, 0.75_f64), (3.0_f64, 1.0_f64), (1_000.0_f64, 0.5_f64)];
    let averages = [0.0_f64, 0.5_f64, 7.0_f64, 1e6_f64];
    let corpora =
        weights.iter().flat_map(|(k1, b)| averages.map(|average_length| Corpus { documents: 1_000, average_length, k1: *k1, b: *b }));
    for corpus in corpora {
        for term in [TermStats { documents: 1, most: 1 }, TermStats { documents: 500, most: 7 }, TermStats { documents: 1_000, most: 300 }]
        {
            let bound = Bm25.bound(&corpus, &term);
            let places = (1..=term.most)
                .flat_map(|frequency| [frequency, frequency.saturating_mul(3), u32::MAX].map(|length| InDocument { frequency, length }));
            for in_doc in places {
                let weight = Bm25.weight(&corpus, &term, &in_doc);
                assert!(weight.is_finite() && weight >= 0.0_f64 && weight <= bound, "{corpus:?} {term:?} {in_doc:?}");
            }
        }
    }
}

/// Verifies: [EXTENSION] FULLTEXT.RANKING, top-k by any ranking: of a
/// short document saying "fox" once and a long one saying it often,
/// BM25, which saturates and normalizes by length, ranks the short first,
/// TfIdf the long; each in descending rank.
#[test]
fn another_ranking_changes_the_order() {
    let long = "fox fox fox fox dog dog dog dog dog dog dog dog dog dog dog dog dog dog dog dog";
    let mut orders = Vec::new();
    for name in ["BM25", "TFIDF"] {
        let index = ranking_by(name);
        index.insert(doc("fox"), 1).unwrap();
        index.insert(doc(long), 2).unwrap();
        index.insert(doc("cat"), 3).unwrap();
        let found = top(&index, r#""fox""#, None);
        assert!(found.windows(2).all(|w| w.first().unwrap().0 >= w.last().unwrap().0), "{name}: {found:?}");
        orders.push(found.first().unwrap().1);
    }
    assert_eq!(orders, [1, 2]);
}

/// Verifies: [EXTENSION] `Index::nearest` of RANK; [DESIGN] RULES.md D86:
/// on a random corpus, the index's top-k gives every document, those the
/// pattern's words match by their rank descending as a full sort of
/// each's rank gives, then the others of rank 0; each rank, of one
/// snapshot, is Rank's of its document bit for bit.
#[test]
fn top_k_is_a_full_sort() {
    for name in ["BM25", "TFIDF"] {
        let index = ranking_by(name);
        let words = ["fox", "dog", "cat", "the", "jumped", "jumping", "quick", "quiet"];
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let mut texts = Vec::new();
        for row in 1..=300_u64 {
            let mut text = String::new();
            for _ in 0..(seed % 13) {
                seed ^= seed << 13_u32;
                seed ^= seed >> 7_u32;
                seed ^= seed << 17_u32;
                text.push_str(words.get(usize::try_from(seed % 8).unwrap()).unwrap());
                text.push(' ');
            }
            seed = seed.wrapping_add(row);
            index.insert(doc(&text), row).unwrap();
            texts.push(text);
        }
        for asked in [r#""fox""#, r#""fox" | "dog""#, r#"FORM OF "jump" & "qui%""#, r#"IS ABOUT "the quick cat""#] {
            let snapshot = index.snapshot().unwrap();
            let scored = |row: RowId| {
                let document = text(&texts[usize::try_from(row).unwrap().checked_sub(1).unwrap()]);
                index.rank(&pattern(asked), &document, &*snapshot).unwrap().unwrap()
            };
            let mut want: Vec<f64> = (1..=300_u64).map(scored).collect();
            want.sort_by(|a, b| b.total_cmp(a));
            let got = top(&index, asked, Some(Arc::clone(&snapshot)));
            let ranks: Vec<f64> = got.iter().map(|(rank, _)| *rank).collect();
            assert_eq!(ranks, want, "{name} {asked}: a full sort, bit for bit");
            let mut rows: Vec<RowId> = got.iter().map(|(_, row)| *row).collect();
            assert!(got.iter().all(|(rank, row)| rank.to_bits() == scored(*row).to_bits()), "{name} {asked}: each Rank's");
            rows.sort_unstable();
            rows.dedup();
            assert_eq!(rows.len(), 300, "{name} {asked}: each document once");
        }
    }
}

/// Verifies: [EXTENSION] RULES.md R-SAFE-3, a ranking's contract: one
/// whose weight passes its bound is refused, SQLCODE -910 naming it, both
/// in the top-k, whose order the bounds decide, and in Rank.
#[test]
fn a_ranking_past_its_bound_is_refused() {
    let index = ranking_by("LIAR");
    index.insert(doc("fox fox"), 1).unwrap();
    index.insert(doc("dog"), 2).unwrap();
    let budget = MemoryBudget::new(usize::MAX);
    let found = Arc::clone(&index).ranked(&Value::Char(r#""fox""#.into()), None, &budget).unwrap().unwrap();
    let e = found.collect::<Result<Vec<_>>>().unwrap_err();
    assert_eq!((e.code, e.sqlcode()), (Code::Api, -910_i32));
    assert!(e.msg.contains("LIAR"), "{}", e.msg);
    let refused = index.rank(&pattern(r#""fox""#), &text("fox fox"), &*index.snapshot().unwrap()).unwrap_err();
    assert_eq!(refused.code, Code::Api);
}

/// Verifies: [EXTENSION] RULES.md D41, FULLTEXT.RANKING: BM25 by default;
/// a registered ranking named in any case is taken; a change made in
/// place (`reconfigure`); an unknown one refused as a bad option's value
/// is, SQLCODE -103, naming those known.
#[test]
fn the_ranking_option_round_trips() {
    let method = FullTextMethod::in_memory().with_rankings(registry());
    let declared = method.index_options();
    let option = declared.iter().find(|o| o.name == RANKING_OPTION).unwrap();
    let Value::Char(default) = &option.default else { panic!("{:?}", option.default) };
    assert_eq!(default, "BM25");
    assert_eq!(option.rebuild, Rebuild::No);
    let column = [DataType::varying(100)];
    let set = |name: &str| described(vec![(RANKING_OPTION.to_owned(), Value::Char(name.into()))]);
    let (table, tfidf) = set("tfidf");
    method.create_on(&column, (&table, &tfidf)).unwrap();
    let refused = method.create_on(&column, (&table, &set("NOPE").1)).err().unwrap();
    assert_eq!(refused.code, Code::SyntaxRule);
    assert!(refused.msg.contains("FULLTEXT.RANKING is one of BM25, TFIDF, LIAR, not NOPE"), "{}", refused.msg);
    let builtin = FullTextMethod::in_memory();
    assert_eq!(builtin.create_on(&column, (&table, &tfidf)).err().unwrap().code, Code::SyntaxRule, "not registered");
    let options = options_of(&tfidf).unwrap();
    let tree = BLinkTree::create(method.files().unwrap().0, method.files().unwrap().1).unwrap();
    let index = method.index_of(tree, options).unwrap();
    assert_eq!(index.ranking().name(), "TFIDF");
    index.reconfigure((&table, &set("BM25").1));
    assert_eq!(index.ranking().name(), "BM25");
    index.reconfigure((&table, &set("NOPE").1));
    assert_eq!(index.ranking().name(), "BM25", "an unknown name, refused before, leaves it");
}
