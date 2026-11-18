//! CREATE HYBRID VECTOR INDEX (the vector design document §11.1),
//! desugared to an index of the method `hybrid` and its options, read back
//! from INFORMATION_SCHEMA; what it refuses; and the index kept as rows
//! change, on files too.

use turborel::db::{Database, Session};
use turborel::error::Code;

use super::fixture::{documents, literal, plan, read};
use crate::common::{Scratch, rows};

/// The method of index `name`, then its options set, as `METHOD;OPTION=VALUE...`.
fn described(s: &Session, name: &str) -> String {
    let method = read(s, &format!("SELECT METHOD_NAME FROM INFORMATION_SCHEMA.INDEXES WHERE INDEX_NAME = '{name}'"));
    let options = read(
        s,
        &format!(
            "SELECT OPTION_NAME || '=' || OPTION_VALUE FROM INFORMATION_SCHEMA.INDEX_OPTIONS \
             WHERE INDEX_NAME = '{name}' AND VALUE_SOURCE <> 'DEFAULT' ORDER BY OPTION_NAME"
        ),
    );
    format!("{};{}", method.join(""), options.join(";"))
}

/// The IDs HYBRID_SEARCH of index `index` gives for `text` and the vector
/// `vector`, `n` of them.
pub fn searched(s: &Session, index: &str, (text, vector): (&str, &[f64]), n: u32) -> Vec<String> {
    let q = format!(
        "SELECT DOC_ID FROM HYBRID_SEARCH(INDEX = {index}, QUERY_TEXT = '{text}', QUERY_VECTOR = {}, TOP_N = {n}) AS H",
        literal(vector)
    );
    read(s, &q)
}

/// Verifies: [EXTENSION] vector_standard §11.1, R10.2: CREATE HYBRID
/// VECTOR INDEX names the method hybrid; VECTOR_IDX's TYPE its vector
/// part's method, HNSW by default, its DISTANCE that method's METRIC and
/// each other parameter the method's own; TEXT_IDX's each an option of the
/// group FULLTEXT; WITH, table options as CREATE INDEX takes them.
#[test]
fn parameters_desugar_to_the_options_of_each_part_11_1() {
    let (_db, s) = documents(None);
    for (sql, expected) in [
        ("CREATE HYBRID VECTOR INDEX H1 ON DOCS (BODY, EMB)", "hybrid;"),
        (
            "CREATE HYBRID VECTOR INDEX H2 ON DOCS (BODY, EMB) PARAMETERS (VECTOR_IDX 'TYPE HNSW, DISTANCE COSINE, NEIGHBORS 8', \
             TEXT_IDX 'LANGUAGE english')",
            "hybrid;FULLTEXT.LANGUAGE=ENGLISH;HNSW.M=8;HNSW.METRIC=COSINE;HYBRID.VECTOR_METHOD=HNSW",
        ),
        (
            "CREATE HYBRID VECTOR INDEX H3 ON DOCS (BODY, EMB) PARAMETERS (VECTOR_IDX 'TYPE IVF, DISTANCE EUCLIDEAN')",
            "hybrid;HYBRID.VECTOR_METHOD=IVF;IVF.METRIC=EUCLIDEAN",
        ),
        (
            "CREATE HYBRID VECTOR INDEX H4 ON DOCS (BODY, EMB) WITH (HYBRID.VECTOR_METHOD = DISKANN, FULLTEXT.K1 = 1.5)",
            "hybrid;FULLTEXT.K1=1.5E0;HYBRID.VECTOR_METHOD=DISKANN",
        ),
    ] {
        s.run(sql).unwrap();
        let name = sql.split_whitespace().nth(4).unwrap();
        assert_eq!(described(&s, name), expected, "{sql}");
        s.commit().unwrap();
    }
    let shown = plan(
        &s,
        "SELECT ID FROM DOCS ORDER BY VECTOR_DISTANCE(EMB, TO_VECTOR('[1,0,0,0]', 4, 'FLOAT32'), 'EUCLIDEAN') FETCH APPROX FIRST 2 ROWS ONLY",
    );
    assert!(shown.contains("using H3 [hybrid]") && shown.contains("approximate EUCLIDEAN"), "{shown}");
}

/// Verifies: [EXTENSION] vector_standard §11.1: a hybrid index is of two
/// columns, a text and a vector; it takes no MODEL or CHUNKER, as it
/// embeds nothing (SQLCODE -900); a VECTOR_IDX TYPE of no vector method
/// is refused, as is a temporary table's.
#[test]
fn what_a_hybrid_index_refuses_11_1() {
    let (db, s) = documents(None);
    let code = |sql: &str| crate::common::exec(&s, sql).expect_err(sql);
    assert_eq!(code("CREATE HYBRID VECTOR INDEX H ON DOCS (BODY, EMB) PARAMETERS (MODEL app.minilm)"), Code::Unsupported);
    assert_eq!(code("CREATE HYBRID VECTOR INDEX H ON DOCS (BODY, EMB) PARAMETERS (CHUNKER BY WORDS)"), Code::Unsupported);
    assert_eq!(code("CREATE HYBRID VECTOR INDEX H ON DOCS (BODY, EMB) PARAMETERS (VECTOR_IDX 'TYPE BTREE')"), Code::SyntaxRule);
    assert_eq!(code("CREATE HYBRID VECTOR INDEX H ON DOCS (BODY)"), Code::Syntax);
    assert_eq!(code("CREATE INDEX H ON DOCS (BODY) USING hybrid"), Code::Unsupported);
    assert_eq!(code("CREATE INDEX H ON DOCS (EMB, BODY) USING hybrid"), Code::Unsupported);
    let temporary = "CREATE SCHEMA AUTHORIZATION P CREATE GLOBAL TEMPORARY TABLE TD (ID INTEGER, BODY VARCHAR(40), EMB VECTOR(4)) \
                     ON COMMIT PRESERVE ROWS";
    db.schema(temporary).unwrap();
    let p = db.session("P");
    assert!(crate::common::exec(&p, "CREATE INDEX TH ON TD (BODY, EMB) USING hybrid").is_err());
}

/// Verifies: [EXTENSION] vector_standard §11.1, RULES.md D19: the index is
/// kept as rows change: a row inserted is found by its words and by its
/// vector, an updated one by its new ones, a deleted one no more; a row
/// of no embedding is found by its words, one of no text by its vector;
/// a rolled back change leaves none.
#[test]
fn the_hybrid_index_follows_the_rows() {
    let (_db, s) = documents(Some("CREATE HYBRID VECTOR INDEX DOCS_H ON DOCS (BODY, EMB)"));
    s.run(&format!("INSERT INTO DOCS VALUES (6, 'zebra crossing', {})", literal(&[0.5, 0.0, 0.0, 1.0]))).unwrap();
    s.run("INSERT INTO DOCS VALUES (7, 'zebra stripes', NULL)").unwrap();
    s.run(&format!("INSERT INTO DOCS VALUES (8, NULL, {})", literal(&[0.5, 0.0, 0.1, 1.0]))).unwrap();
    s.commit().unwrap();
    assert_eq!(searched(&s, "DOCS_H", ("zebra", &[0.5, 0.0, 0.0, 1.0]), 3), ["6", "7", "8"]);
    s.run("UPDATE DOCS SET BODY = 'plain crossing' WHERE ID = 6").unwrap();
    s.run("DELETE FROM DOCS WHERE ID = 8").unwrap();
    s.commit().unwrap();
    assert_eq!(searched(&s, "DOCS_H", ("zebra", &[0.5, 0.0, 0.0, 1.0]), 2), ["6", "7"]);
    s.run(&format!("INSERT INTO DOCS VALUES (9, 'zebra zebra', {})", literal(&[0.5, 0.0, 0.0, 1.0]))).unwrap();
    s.rollback().unwrap();
    assert_eq!(searched(&s, "DOCS_H", ("zebra", &[0.5, 0.0, 0.0, 1.0]), 2), ["6", "7"]);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM DOCS WHERE ID BETWEEN 6 AND 999").unwrap(), ["2"]);
}

/// Verifies: [DESIGN] RULES.md D25: a database of files keeps a hybrid
/// index: reopened, it is read by its locator, both its parts answering
/// as before, and it still holds a row of no embedding by its words, its
/// text part still read for RANK.
#[test]
fn a_hybrid_index_on_files_reopens() {
    let scratch = Scratch::new("vector-hybrid-reopen");
    let open = || {
        let db = Database::new();
        super::fixture::register(&db);
        db.open_files(&crate::common::after_crash(), &scratch.path("db"), &turborel::db::OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE DOCS (ID INTEGER PRIMARY KEY, BODY VARCHAR(400), EMB VECTOR(4))").unwrap();
    let s = db.session("S");
    s.run("SET SQL.FUNCTION_CALLS = 'ON'").unwrap();
    for (n, (text, v)) in super::fixture::DOCUMENTS.iter().enumerate() {
        s.run(&format!("INSERT INTO DOCS VALUES ({}, '{text}', {})", n + 1, literal(v))).unwrap();
    }
    s.commit().unwrap();
    s.run("CREATE HYBRID VECTOR INDEX DOCS_H ON DOCS (BODY, EMB)").unwrap();
    let before = searched(&s, "DOCS_H", ("refund damaged", &[1.0, 0.0, 0.0, 0.0]), 5);
    db.checkpoint().unwrap();
    drop((s, db));
    let db = open();
    let s = db.session("S");
    s.run("SET SQL.FUNCTION_CALLS = 'ON'").unwrap();
    assert_eq!(searched(&s, "DOCS_H", ("refund damaged", &[1.0, 0.0, 0.0, 0.0]), 5), before);
    assert_eq!(before.first().map(String::as_str), Some("1"));
    s.run("INSERT INTO DOCS VALUES (6, 'zebra crossing', NULL)").unwrap();
    s.commit().unwrap();
    assert!(searched(&s, "DOCS_H", ("zebra", &[0.0, 0.0, 0.0, 1.0]), 6).contains(&"6".to_owned()));
    let filler: Vec<String> = (100..2100).map(|k| format!("({k}, 'filler row number {k}', NULL)")).collect();
    for chunk in filler.chunks(200) {
        s.run(&format!("INSERT INTO DOCS VALUES {}", chunk.join(", "))).unwrap();
    }
    s.commit().unwrap();
    let ranked = "SELECT ID FROM DOCS ORDER BY RANK(BODY, 'IS ABOUT \"zebra\"') DESC NULLS LAST FETCH FIRST 1 ROWS ONLY";
    let shown = plan(&s, ranked);
    assert!(shown.contains("NearestScan S.DOCS using DOCS_H [hybrid]: RANK("), "{shown}");
    assert_eq!(read(&s, ranked), ["6"]);
}
