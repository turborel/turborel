//! ALTER INDEX of a hybrid index that embeds its text: a MODEL, CHUNKER
//! or MODE set makes its chunks again, as REBUILD PARAMETERS would;
//! what could not be made is refused before anything changes.

use turborel::error::Code;

use turborel::db::Session;

use super::fixture::read;

/// `sql` run after the open transaction is committed, as an index
/// statement runs outside one.
fn alter(s: &Session, sql: &str) -> Result<(), Code> {
    s.commit().unwrap();
    crate::common::exec(s, sql).map(drop)
}

/// The index's chunks, each its document and text.
const CHUNKS: &str = "SELECT CAST(DOC_ID AS VARCHAR(9)) || ':' || CHUNK_TEXT FROM DOCS_H_CHUNKS ORDER BY DOC_ID, CHUNK_ID";
/// The search of 'refund' alone, by the index's model.
const SEARCH: &str = "SELECT DOC_ID FROM HYBRID_SEARCH(INDEX = DOCS_H, QUERY_TEXT = 'refund', TOP_N = 1) AS H";

/// Verifies: [EXTENSION] vector_standard §10.6, §11.1, §12.3: ALTER INDEX
/// SET of HYBRID.CHUNKER cuts the text again, of HYBRID.MODEL embeds it
/// again by the model named, of HYBRID.MODE keeps it after commit, each
/// leaving the index READY and searched by its new model; REBUILD builds
/// it again; a model not there or a chunking not one is refused and the
/// index left as it was; a MODEL taken back, HYBRID.STATE set, or a
/// MODEL given to an index of two columns is refused.
#[test]
fn a_model_chunker_or_mode_set_makes_the_chunks_again_10_6() {
    let (db, s) = super::embedded::docs();
    s.run("CREATE EMBEDDING MODEL WIDE WITH (TYPE = 'HASH_DENSE', DIMENSIONS = 32, NGRAMS = 1)").unwrap();
    s.commit().unwrap();
    s.run("CREATE HYBRID VECTOR INDEX DOCS_H ON DOCS (BODY) PARAMETERS (MODEL WORDS, CHUNKER BY SENTENCES MAX 1 OVERLAP 0)").unwrap();
    assert_eq!(read(&s, CHUNKS).len(), 6);
    alter(&s, "ALTER INDEX DOCS_H SET (HYBRID.CHUNKER = 'BY WORDS MAX 3 OVERLAP 0')").unwrap();
    assert_eq!(read(&s, CHUNKS).first().map(String::as_str), Some("1:refund policy for"));
    alter(&s, "ALTER INDEX DOCS_H SET (HYBRID.MODEL = 'WIDE')").unwrap();
    let dims = "SELECT VECTOR_DIMENSION_COUNT(EMBEDDING) FROM DOCS_H_CHUNKS FETCH FIRST 1 ROW ONLY";
    assert_eq!(read(&s, dims), ["32"]);
    assert_eq!(read(&s, SEARCH).len(), 1);
    let state = "SELECT OPTION_VALUE FROM INFORMATION_SCHEMA.INDEX_OPTIONS WHERE INDEX_NAME = 'DOCS_H' \
                 AND OPTION_NAME IN ('HYBRID.STATE', 'HYBRID.MODEL', 'HYBRID.MODE') ORDER BY OPTION_NAME";
    assert_eq!(read(&s, state), ["SYNC", "WIDE", "READY"]);
    alter(&s, "ALTER INDEX DOCS_H SET (HYBRID.MODE = ASYNC)").unwrap();
    s.run("INSERT INTO DOCS VALUES (4, 'zebra crossing')").unwrap();
    s.commit().unwrap();
    db.wait_for_embeddings().unwrap();
    assert!(read(&s, CHUNKS).contains(&"4:zebra crossing".to_owned()));
    alter(&s, "ALTER INDEX DOCS_H REBUILD").unwrap();
    assert_eq!(read(&s, SEARCH).len(), 1);
    let before = read(&s, CHUNKS);
    let code = |sql: &str| alter(&s, sql).expect_err(sql);
    assert_eq!(code("ALTER INDEX DOCS_H SET (HYBRID.MODEL = 'NOSUCH')"), Code::SyntaxRule);
    assert_eq!(code("ALTER INDEX DOCS_H SET (HYBRID.CHUNKER = 'BY LINES')"), Code::Data);
    assert_eq!(code("ALTER INDEX DOCS_H SET (HYBRID.MODEL = DEFAULT)"), Code::SyntaxRule);
    assert_eq!(code("ALTER INDEX DOCS_H SET (HYBRID.STATE = 'BUILDING')"), Code::SyntaxRule);
    assert_eq!(read(&s, CHUNKS), before, "left as it was");
    assert_eq!(read(&s, state), ["ASYNC", "WIDE", "READY"]);
    s.run("CREATE TABLE PAIRS (ID INTEGER PRIMARY KEY, BODY VARCHAR(40), EMB VECTOR(4))").unwrap();
    s.commit().unwrap();
    s.run("CREATE HYBRID VECTOR INDEX PAIRS_H ON PAIRS (BODY, EMB)").unwrap();
    assert_eq!(code("ALTER INDEX PAIRS_H SET (HYBRID.MODEL = 'WORDS')"), Code::SyntaxRule);
}
