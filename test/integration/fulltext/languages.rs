//! Documents in many languages through SQL (RULES.md D320 to D329):
//! FULLTEXT values naming their language, searched by CONTAINS with and
//! without a full-text index, ranked by RANK, and tokenized by their
//! methods; words stemmed by Snowball, elisions taken off, Turkish case
//! folded, and Chinese, Japanese and Thai read as character bigrams.

use turborel::db::{Database, Session};
use turborel::params::Params;

use super::documents;
use crate::common::rows;

/// The least ID of the filler rows, which no test looks for.
const FILLER: u32 = 1000;

/// Documents of one sentence each, in the language named after it.
const BODIES: [&str; 9] = [
    "FULLTEXT('Les enfants jouaient dans l''école du village', 'FRENCH')",
    "FULLTEXT('Die Kinder spielten im großen Garten', 'GERMAN')",
    "FULLTEXT('Дети играли в школе', 'RUSSIAN')",
    "FULLTEXT('東京都庁は新宿にあります', 'JAPANESE')",
    "FULLTEXT('我们在北京大学学习', 'CHINESE')",
    "FULLTEXT('สวัสดีครับ ยินดีต้อนรับ', 'THAI')",
    "FULLTEXT('İstanbul ılık bir şehir', 'TURKISH')",
    "FULLTEXT('Los niños jugaban en la escuela', 'SPANISH')",
    "FULLTEXT('The children played in the school', 'ENGLISH')",
];

/// DOCS of `BODIES` and 2000 English filler rows, so a lookup costs less
/// than a scan.
fn filled(db: &Database) -> Session {
    let a = documents(db, &BODIES);
    let filler: Vec<String> = (FILLER..FILLER + 2000).map(|k| format!("({k}, FULLTEXT('filler row number {k}'))")).collect();
    for chunk in filler.chunks(200) {
        a.run(&format!("INSERT INTO DOCS VALUES {}", chunk.join(", "))).unwrap();
    }
    a.run("COMMIT WORK").unwrap();
    a
}

/// The query of the documents CONTAINS finds `pattern` in.
fn search(pattern: &str) -> String {
    format!("SELECT ID FROM DOCS WHERE CONTAINS(BODY, '{}') = 1 ORDER BY ID", pattern.replace('\'', "''"))
}

/// The IDs below `FILLER` of the documents CONTAINS finds `pattern` in.
fn found(a: &Session, pattern: &str) -> Vec<String> {
    let ids = rows(a, &search(pattern)).unwrap_or_else(|e| panic!("{pattern}: {e:?}"));
    ids.into_iter().filter(|id| id.parse::<u32>().is_ok_and(|id| id < FILLER)).collect()
}

/// The patterns of the search tests, each with the IDs it finds.
const CASES: [(&str, &[&str]); 19] = [
    ("\"école\"", &["1"]),
    ("FORM OF \"écoles\"", &["1"]),
    ("FORM OF \"enfant\"", &["1"]),
    ("FORM OF \"Kind\"", &["2"]),
    ("\"GROSSEN\"", &[]),
    ("FORM OF \"школа\"", &["3"]),
    ("\"東京\"", &["4"]),
    ("\"京都\"", &["4"]),
    ("\"東京都庁\"", &["4"]),
    ("\"京\"", &["4", "5"]),
    ("\"北京大学\"", &["5"]),
    ("\"สวัสดี\"", &["6"]),
    ("\"İSTANBUL\"", &["7"]),
    ("\"ISTANBUL\"", &[]),
    ("\"ILIK\"", &["7"]),
    ("FORM OF \"niño\"", &["8"]),
    ("FORM OF \"plays\"", &["9"]),
    ("FORM OF \"children\" | FORM OF \"enfants\"", &["1", "9"]),
    ("SPANISH \"escuela\" & FORM OF \"jugar\"", &["8"]),
];

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4, §4.4, §4.5, §5.1.2,
/// §5.3.1 Description 3c] [EXTENSION] (RULES.md D320 to D324, D329)
/// CONTAINS finds each document by its words read in its own language:
/// stems by Snowball, an elided article taken off, accents kept, Turkish
/// dotted and dotless I apart, character bigrams of Japanese, Chinese and Thai,
/// a single ideograph found in the bigrams holding it; alike with a
/// full-text index of documents in many languages, which it reads.
#[test]
fn contains_finds_words_in_each_language() {
    let db = Database::new();
    let a = filled(&db);
    for (pattern, want) in CASES {
        assert_eq!(found(&a, pattern), want, "{pattern} unindexed");
    }
    a.run("COMMIT WORK").unwrap();
    a.run("CREATE INDEX DOCS_BODY ON DOCS (BODY) USING fulltext").unwrap();
    for (pattern, want) in CASES {
        assert_eq!(found(&a, pattern), want, "{pattern} indexed");
    }
    let plan = a.physical_plan(&search("\"東京\""), &Params::new()).unwrap();
    assert!(plan.contains("using DOCS_BODY"), "{plan}");
    a.run("INSERT INTO DOCS VALUES (10, FULLTEXT('Gli studenti dell''università', 'ITALIAN'))").unwrap();
    assert_eq!(found(&a, "FORM OF \"università\""), ["10"]);
    assert_eq!(found(&a, "\"studenti\" & \"università\""), ["10"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.4, §5.1.5, §5.1.7] [EXTENSION]
/// (RULES.md D321 to D323) Tokenize, TokenizeAndStem and
/// TokenizePosition read a value in its language: French elisions and
/// stop words, German stems, and Japanese bigrams at their characters.
#[test]
fn tokenizing_methods_read_each_language() {
    let db = Database::new();
    let a = documents(&db, &[BODIES[0], BODIES[1], BODIES[3]]);
    let words = |id: u32, method: &str| {
        rows(&a, &format!("SELECT W FROM DOCS, UNNEST(DOCS.BODY.{method}()) WITH ORDINALITY AS U (W, N) WHERE ID = {id} ORDER BY N"))
            .unwrap()
    };
    assert_eq!(words(1, "Tokenize"), ["enfants", "jouaient", "école", "village"]);
    assert_eq!(words(1, "TokenizeAndStem"), ["enfant", "jou", "écol", "villag"]);
    assert_eq!(words(2, "TokenizeAndStem"), ["kind", "spielt", "gross", "gart"]);
    assert_eq!(words(3, "Tokenize"), ["東京", "京都", "都庁", "庁は", "は新", "新宿", "宿に", "にあ", "あり", "りま", "ます"]);
    let places = "SELECT T, P, C FROM DOCS, UNNEST(DOCS.BODY.TokenizePosition('CHARACTERS')) WITH ORDINALITY AS U (T, P, C, N) WHERE ID = 1 AND N = 3";
    assert_eq!(rows(&a, places).unwrap(), ["école,29,5"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.3] [EXTENSION] (RULES.md D324)
/// RANK ranks documents in each language by the words read in theirs,
/// with and without a full-text index alike.
#[test]
fn rank_ranks_each_language() {
    let db = Database::new();
    let a = filled(&db);
    let ranked = "SELECT ID FROM DOCS WHERE RANK(BODY, 'FORM OF \"école\" | \"東京\"') > 0 AND ID < 1000 ORDER BY ID";
    assert_eq!(rows(&a, ranked).unwrap(), ["1", "4"]);
    a.run("COMMIT WORK").unwrap();
    a.run("CREATE INDEX DOCS_BODY ON DOCS (BODY) USING fulltext").unwrap();
    assert_eq!(rows(&a, ranked).unwrap(), ["1", "4"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.13.5, §8] [EXTENSION] (RULES.md
/// D329) a thesaurus's synonyms expand a word of each language, a
/// Japanese term read as bigrams and joined back, a French one with its
/// accents kept.
#[test]
fn thesauri_expand_each_language() {
    let db = Database::new();
    let a = documents(
        &db,
        &[
            "FULLTEXT('東京都に住む', 'JAPANESE')",
            "FULLTEXT('首都で働く', 'JAPANESE')",
            "FULLTEXT('Paris est la capitale', 'FRENCH')",
            "FULLTEXT('Lyon est une métropole', 'FRENCH')",
        ],
    );
    a.run("GRANT SELECT ON DOCS TO PUBLIC").unwrap();
    a.run("COMMIT WORK").unwrap();
    db.schema(super::thesaurus::TABLES).unwrap();
    let t = db.session("FT_THESAURUS");
    t.run(
        "INSERT INTO TERM_DICTIONARY VALUES (1, '東京都', 'CITY'), (2, '首都', 'CITY'), (3, 'capitale', 'CITY'), (4, 'métropole', 'CITY')",
    )
    .unwrap();
    t.run("INSERT INTO TERM_SYNONYM VALUES (1, 2, NULL, 'CITY'), (3, 4, NULL, 'CITY')").unwrap();
    t.run("COMMIT WORK").unwrap();
    let expanded = |term: &str| {
        let pattern = format!("THESAURUS \"CITY\" EXPAND SYNONYM TERM OF \"{term}\"");
        rows(&a, &format!("SELECT ID FROM DOCS WHERE CONTAINS(BODY, '{pattern}') = 1 ORDER BY ID")).unwrap()
    };
    assert_eq!(expanded("東京都"), ["1", "2"]);
    assert_eq!(expanded("capitale"), ["3", "4"]);
}

/// Verifies: [EXTENSION] RULES.md D327: TEXT_MATCHES reads both its
/// strings in the FULLTEXT.LANGUAGE of the index of the document's
/// column, alike by the index and by a scan, so a French elision is
/// taken off; with no index, in ENGLISH, where it is not.
#[test]
fn text_matches_reads_the_index_language() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION F CREATE TABLE FR (ID INTEGER NOT NULL UNIQUE, BODY VARCHAR(100))").unwrap();
    let f = db.session("F");
    f.run("INSERT INTO FR VALUES (1, 'Les enfants de l''école'), (2, 'Le village')").unwrap();
    f.run("COMMIT WORK").unwrap();
    let matched = "SELECT ID FROM FR WHERE TEXT_MATCHES(BODY, 'ÉCOLE enfants') ORDER BY ID";
    assert!(rows(&f, matched).unwrap().is_empty(), "l'école is one English word");
    f.run("COMMIT WORK").unwrap();
    f.run("CREATE INDEX FR_BODY ON FR (BODY) USING fulltext WITH (FULLTEXT.LANGUAGE = FRENCH)").unwrap();
    f.run("COMMIT WORK").unwrap();
    assert_eq!(rows(&f, matched).unwrap(), ["1"]);
    let scanned = "SELECT ID FROM FR WHERE TEXT_MATCHES(BODY, 'école') OR ID = 0 ORDER BY ID";
    assert_eq!(rows(&f, scanned).unwrap(), ["1"], "a scan reads it in the index's language too");
    assert!(rows(&f, "SELECT ID FROM FR WHERE TEXT_MATCHES(BODY, 'ecole')").unwrap().is_empty(), "French keeps accents");
}
