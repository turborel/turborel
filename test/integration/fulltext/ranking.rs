//! RANK by BM25 and by registered rankings (RULES.md D87), and the top-k
//! a full-text index answers, through SQL.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::fulltext::rank::{Corpus, InDocument, Ranking, TermStats};
use turborel::params::Params;

use crate::common::rows;

/// Schema R's table DOCS (ID, BODY VARCHAR(200)) of `texts`, ID from 1,
/// with 2000 rows of filler, and its full-text index DOCS_BODY of `options`.
fn indexed(db: &Database, texts: &[&str], options: &str) -> Session {
    db.schema("CREATE SCHEMA AUTHORIZATION R CREATE TABLE DOCS (ID INTEGER NOT NULL UNIQUE, BODY VARCHAR(200))").unwrap();
    let r = db.session("R");
    for (id, text) in texts.iter().enumerate() {
        r.run(&format!("INSERT INTO DOCS VALUES ({}, '{text}')", id + 1)).unwrap();
    }
    let filler: Vec<String> = (100..2100).map(|k| format!("({k}, 'filler row number {k}')")).collect();
    for chunk in filler.chunks(200) {
        r.run(&format!("INSERT INTO DOCS VALUES {}", chunk.join(", "))).unwrap();
    }
    r.run("COMMIT WORK").unwrap();
    r.run(&format!("CREATE INDEX DOCS_BODY ON DOCS (BODY) USING fulltext {options}")).unwrap();
    r
}

/// Texts in which "fox" is ever rarer by weight.
const TEXTS: [&str; 4] = ["fox fox fox", "a fox and a dog", "dog dog dog dog dog dog dog dog dog fox", "a cat"];

/// The top-k query of `n` rows by RANK of `pattern`.
fn top(n: u32, pattern: &str) -> String {
    format!("SELECT ID FROM DOCS WHERE CONTAINS(BODY, '{pattern}') = 1 ORDER BY RANK(BODY, '{pattern}') DESC FETCH FIRST {n} ROWS ONLY")
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.3] RANK of an indexed column
/// scores by BM25 over the index's statistics: more occurrences in a
/// shorter document rank higher, a non-match is 0; and a top-k by RANK
/// DESC is read from the index, giving the rows a full sort gives, the
/// thesaurus tables there or not.
#[test]
fn rank_orders_by_bm25_and_the_index_gives_the_top() {
    let db = Database::new();
    let r = indexed(&db, &TEXTS, "");
    let full = rows(&r, "SELECT ID FROM DOCS WHERE CONTAINS(BODY, '\"fox\"') = 1 ORDER BY RANK(BODY, '\"fox\"') DESC").unwrap();
    assert_eq!(full, ["1", "2", "3"]);
    assert_eq!(rows(&r, &top(2, "\"fox\"")).unwrap(), ["1", "2"]);
    let plan = r.physical_plan(&top(2, "\"fox\""), &Params::new()).unwrap();
    assert!(plan.contains("NearestScan R.DOCS using DOCS_BODY"), "{plan}");
    assert_eq!(rows(&r, "SELECT RANK(BODY, '\"fox\"') FROM DOCS WHERE ID = 4").unwrap(), ["0E0"]);
    // A constant pattern naming no thesaurus reads none, so thesauri there
    // keep the index read (D87).
    db.schema(super::thesaurus::TABLES).unwrap();
    let plan = r.physical_plan(&top(2, "\"fox\""), &Params::new()).unwrap();
    assert!(plan.contains("NearestScan R.DOCS using DOCS_BODY") && !plan.contains("TERM_"), "{plan}");
    let scored = rows(&r, "SELECT RANK(BODY, '\"fox\"') FROM DOCS WHERE ID = 1").unwrap();
    assert!(scored[0].parse::<f64>().is_ok_and(|s| s > 0.0), "{scored:?}");
}

/// A ranking weighing each occurrence 1, bounded truly.
struct Count;

impl Ranking for Count {
    fn name(&self) -> &'static str {
        "COUNTED"
    }

    fn weight(&self, _: &Corpus, _: &TermStats, in_doc: &InDocument) -> f64 {
        f64::from(in_doc.frequency)
    }

    fn bound(&self, _: &Corpus, term: &TermStats) -> f64 {
        f64::from(term.most)
    }
}

/// A ranking whose bound is less than its weights.
struct Lying;

impl Ranking for Lying {
    fn name(&self) -> &'static str {
        "LYING"
    }

    fn weight(&self, _: &Corpus, _: &TermStats, in_doc: &InDocument) -> f64 {
        f64::from(in_doc.frequency)
    }

    fn bound(&self, _: &Corpus, _: &TermStats) -> f64 {
        0.5
    }
}

/// Verifies: [EXTENSION] RULES.md D87, a ranking registered by
/// `Database::register_ranking` is named by FULLTEXT.RANKING at CREATE
/// and ALTER INDEX, and RANK and the top-k score by it; an unknown name is
/// refused (-103), as are a duplicate, BM25 included, and a name not an
/// upper-case identifier (-910); a ranking breaking its bound is -910.
#[test]
fn registered_rankings_score_and_are_checked() {
    let db = Database::new();
    db.register_ranking(Box::new(Count)).unwrap();
    db.register_ranking(Box::new(Lying)).unwrap();
    assert_eq!(db.register_ranking(Box::new(Count)).unwrap_err().code, Code::Api);
    let r = indexed(&db, &TEXTS, "WITH (FULLTEXT.RANKING = COUNTED)");
    assert_eq!(rows(&r, "SELECT RANK(BODY, '\"dog\"') FROM DOCS WHERE ID = 3").unwrap(), ["9E0"]);
    assert_eq!(rows(&r, &top(1, "\"dog\"")).unwrap(), ["3"]);
    r.run("COMMIT WORK").unwrap();
    let unknown = r.run("ALTER INDEX DOCS_BODY SET (FULLTEXT.RANKING = NOWHERE)").unwrap_err();
    assert_eq!(unknown.code, Code::SyntaxRule);
    r.run("ALTER INDEX DOCS_BODY SET (FULLTEXT.RANKING = LYING)").unwrap();
    for broken in ["SELECT RANK(BODY, '\"dog\"') FROM DOCS WHERE ID = 3".to_owned(), top(1, "\"dog\"")] {
        let refused = r.open_cursor(&broken).and_then(|mut c| c.fetch().map(|_| ())).unwrap_err();
        assert_eq!(refused.code, Code::Api, "{broken}");
    }
    r.run("ROLLBACK WORK").unwrap();
    r.run("ALTER INDEX DOCS_BODY SET (FULLTEXT.RANKING = DEFAULT)").unwrap();
    assert_eq!(rows(&r, &top(1, "\"dog\"")).unwrap(), ["3"]);
}

/// A ranking named in lower case.
struct Lower;

impl Ranking for Lower {
    fn name(&self) -> &'static str {
        "lower"
    }

    fn weight(&self, _: &Corpus, _: &TermStats, _: &InDocument) -> f64 {
        0.0
    }

    fn bound(&self, _: &Corpus, _: &TermStats) -> f64 {
        0.0
    }
}

/// A ranking named as the built-in.
struct Shadow;

impl Ranking for Shadow {
    fn name(&self) -> &'static str {
        "BM25"
    }

    fn weight(&self, _: &Corpus, _: &TermStats, _: &InDocument) -> f64 {
        0.0
    }

    fn bound(&self, _: &Corpus, _: &TermStats) -> f64 {
        0.0
    }
}

/// Verifies: [EXTENSION] RULES.md D87, `register_ranking` refuses a name not an
/// upper-case identifier and the built-in's name (-910); RANK of a
/// column of no index is BM25 with every inverse document frequency 1.
#[test]
fn rankings_are_named_once_and_rank_needs_no_index() {
    let db = Database::new();
    assert_eq!(db.register_ranking(Box::new(Lower)).unwrap_err().code, Code::Api);
    assert_eq!(db.register_ranking(Box::new(Shadow)).unwrap_err().code, Code::Api);
    db.schema("CREATE SCHEMA AUTHORIZATION U CREATE TABLE T (ID INTEGER, BODY VARCHAR(40))").unwrap();
    let u = db.session("U");
    u.run("INSERT INTO T VALUES (1, 'fox fox'), (2, 'fox and a dog')").unwrap();
    let ranked = rows(&u, "SELECT ID FROM T ORDER BY RANK(BODY, '\"fox\"') DESC").unwrap();
    assert_eq!(ranked, ["1", "2"]);
}
