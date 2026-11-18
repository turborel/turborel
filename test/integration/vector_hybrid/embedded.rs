//! A hybrid index that embeds its text itself, by a MODEL and a CHUNKER
//! (the vector design document §11.1, §12): its chunks, their embeddings
//! and the index kept as the text changes, HYBRID_SEARCH giving each
//! document once, at its best chunk; DROP INDEX taking all with it.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::value::Value;

use super::fixture::{read, register};

/// The model's statement: words hashed into 16 dimensions.
const MODEL: &str = "CREATE EMBEDDING MODEL WORDS WITH (TYPE = 'HASH_DENSE', DIMENSIONS = 16, NGRAMS = 1)";

/// Schema S's table DOCS (ID INTEGER PRIMARY KEY, BODY VARCHAR(400)) of
/// three documents, each of two sentences, and the model WORDS.
pub(super) fn docs() -> (Database, Session) {
    let db = Database::new();
    register(&db);
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE DOCS (ID INTEGER PRIMARY KEY, BODY VARCHAR(400))").unwrap();
    let s = db.session("S");
    s.run(MODEL).unwrap();
    s.run(
        "INSERT INTO DOCS VALUES (1, 'refund policy for damaged items. shipping is free'), \
         (2, 'store opening hours. we open at nine'), (3, 'a damaged parcel is returned. refund in a week')",
    )
    .unwrap();
    s.commit().unwrap();
    (db, s)
}

/// The hybrid search of `words` and their embedding in `index`, its
/// DOC_ID, VECTOR_RANK and TEXT_RANK.
fn searched(s: &Session, index: &str, words: &str) -> Vec<String> {
    read(
        s,
        &format!(
            "SELECT DOC_ID, CHUNK_TEXT FROM HYBRID_SEARCH(INDEX = {index}, QUERY_TEXT = '{words}', \
             QUERY_VECTOR = EMBED('WORDS', '{words}'), TOP_N = 3) AS H ORDER BY SCORE DESC, DOC_ID"
        ),
    )
}

/// Verifies: [EXTENSION] vector_standard §11.1, §12.3, §12.5, R11.2: a
/// hybrid index of one text column and a MODEL stores the text's chunks,
/// cut as CHUNKER says, their embeddings by the model, and an index of
/// both; HYBRID_SEARCH gives each document by its key once, with its best
/// chunk; the chunks follow inserts, updates and deletes; DROP INDEX
/// drops the chunks and their pipeline.
#[test]
fn a_model_and_a_chunker_embed_the_text_11_1() {
    let (_db, s) = docs();
    s.run("CREATE HYBRID VECTOR INDEX DOCS_H ON DOCS (BODY) PARAMETERS (MODEL WORDS, CHUNKER BY SENTENCES MAX 1 OVERLAP 0, VECTOR_IDX 'DISTANCE COSINE')")
        .unwrap();
    let options = "SELECT OPTION_NAME || '=' || OPTION_VALUE FROM INFORMATION_SCHEMA.INDEX_OPTIONS \
                   WHERE INDEX_NAME = 'DOCS_H' AND OPTION_NAME LIKE 'HYBRID.%' ORDER BY OPTION_NAME";
    assert_eq!(
        read(&s, options),
        [
            "HYBRID.CHUNKER=BY SENTENCES MAX 1 OVERLAP 0",
            "HYBRID.MODE=SYNC",
            "HYBRID.MODEL=WORDS",
            "HYBRID.STATE=READY",
            "HYBRID.VECTOR_METHOD=HNSW"
        ]
    );
    assert_eq!(read(&s, "SELECT COUNT(*) FROM DOCS_H_CHUNKS"), ["6"]);
    let found = searched(&s, "DOCS_H", "refund");
    for best in ["1,refund policy for damaged items.", "3,refund in a week"] {
        assert!(found.contains(&best.to_owned()), "{found:?}");
    }
    assert_eq!(found.len(), 3, "one row a document: {found:?}");
    let wide = "SELECT COUNT(*) - COUNT(DISTINCT DOC_ID) FROM HYBRID_SEARCH(INDEX = DOCS_H, QUERY_TEXT = 'refund', \
                QUERY_VECTOR = EMBED('WORDS', 'refund damaged'), TOP_N = 9) AS H";
    assert_eq!(read(&s, wide), ["0"], "no document twice");
    s.run("INSERT INTO DOCS VALUES (4, 'zebra crossing. stripes')").unwrap();
    s.run("UPDATE DOCS SET BODY = 'closed on sundays' WHERE ID = 2").unwrap();
    s.run("DELETE FROM DOCS WHERE ID = 3").unwrap();
    s.commit().unwrap();
    assert_eq!(read(&s, "SELECT DOC_ID, CHUNK_TEXT FROM DOCS_H_CHUNKS ORDER BY DOC_ID, CHUNK_ID").len(), 5);
    assert_eq!(searched(&s, "DOCS_H", "zebra").first().map(String::as_str), Some("4,zebra crossing."));
    assert!(searched(&s, "DOCS_H", "parcel").iter().all(|r| !r.starts_with("3,")));
    s.commit().unwrap();
    s.run("DROP INDEX DOCS_H").unwrap();
    let kept = "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'DOCS_H_CHUNKS' \
                UNION SELECT PIPELINE_NAME FROM INFORMATION_SCHEMA.VECTOR_PIPELINES";
    assert_eq!(read(&s, kept), Vec::<String>::new());
    s.run("INSERT INTO DOCS VALUES (5, 'kept without its index')").unwrap();
    s.commit().unwrap();
}

/// Verifies: [EXTENSION] vector_standard §11.1, R12.6: MODE ASYNC leaves
/// a chunk's embedding to the queue after commit, so a chunk is found by
/// its words at once and by its embedding once the queue has run; an
/// index of a MODEL names its text column alone, of a table keyed by one
/// column, and a model there is, and a chunking CHUNK_TEXT reads, and
/// no table has the name of its table of chunks, which is left as it
/// was; a refused index leaves no table of chunks behind.
#[test]
fn an_async_index_and_what_a_model_index_refuses_r12_6() {
    let (db, s) = docs();
    s.run("CREATE HYBRID VECTOR INDEX DOCS_A ON DOCS (BODY) PARAMETERS (MODEL S.WORDS, MODE ASYNC)").unwrap();
    assert_eq!(read(&s, "SELECT COUNT(*) FROM DOCS_A_CHUNKS WHERE EMBEDDING IS NULL"), ["3"]);
    db.wait_for_embeddings().unwrap();
    assert_eq!(read(&s, "SELECT COUNT(*) FROM DOCS_A_CHUNKS WHERE EMBEDDING IS NULL"), ["0"]);
    assert!(searched(&s, "DOCS_A", "hours").first().is_some_and(|r| r.starts_with("2,")));
    s.commit().unwrap();
    let code = |sql: &str| crate::common::exec(&s, sql).expect_err(sql);
    assert_eq!(code("CREATE HYBRID VECTOR INDEX X ON DOCS (BODY) PARAMETERS (MODEL NOSUCH)"), Code::SyntaxRule);
    assert_eq!(read(&s, "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME LIKE 'X%'"), Vec::<String>::new());
    s.commit().unwrap();
    s.run("CREATE TABLE Z_CHUNKS (N INTEGER)").unwrap();
    s.run("INSERT INTO Z_CHUNKS VALUES (7)").unwrap();
    s.commit().unwrap();
    assert_eq!(code("CREATE HYBRID VECTOR INDEX Z ON DOCS (BODY) PARAMETERS (MODEL WORDS)"), Code::SyntaxRule);
    assert_eq!(read(&s, "SELECT N FROM Z_CHUNKS"), ["7"], "a table of the name kept");
    s.commit().unwrap();
    s.run("CREATE TABLE PAIRS (A INTEGER, B INTEGER, T VARCHAR(40), PRIMARY KEY (A, B))").unwrap();
    s.commit().unwrap();
    assert_eq!(code("CREATE HYBRID VECTOR INDEX Y ON PAIRS (T) PARAMETERS (MODEL WORDS)"), Code::SyntaxRule);
    s.run("CREATE TABLE E (ID INTEGER PRIMARY KEY, T VARCHAR(40), V VECTOR(16))").unwrap();
    s.commit().unwrap();
    assert_eq!(code("CREATE HYBRID VECTOR INDEX Z ON E (T, V) PARAMETERS (MODEL WORDS)"), Code::SyntaxRule);
    assert_eq!(code("CREATE HYBRID VECTOR INDEX Z ON E (T)"), Code::Unsupported);
    assert_eq!(code("CREATE HYBRID VECTOR INDEX Z ON E (T) PARAMETERS (MODEL WORDS, CHUNKER BY LINES MAX 2)"), Code::Data);
}

/// The rows of the statement `sql` prepared, of the dynamic parameters
/// `values`, each its columns' values joined by commas.
fn prepared(s: &Session, sql: &str, values: &[Value]) -> Vec<String> {
    let p = s.prepare(sql).unwrap();
    let mut c = s.open_prepared(&p, values).unwrap();
    let mut found = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        found.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    found
}

/// Verifies: [EXTENSION] vector_standard §11.2, §12.2, [ISO/IEC 9075:1992
/// §17.6]: of an index of a MODEL, HYBRID_SEARCH of QUERY_TEXT alone
/// embeds it by that model, as a query, and searches both lists, as
/// QUERY_VECTOR of that embedding does; a QUERY_TEXT of a dynamic
/// parameter, read twice so, is one parameter, numbered among the
/// statement's others in order, and gives the rows its value written
/// gives.
#[test]
fn query_text_alone_is_embedded_by_the_model_11_2() {
    let (_db, s) = docs();
    s.run("CREATE HYBRID VECTOR INDEX DOCS_H ON DOCS (BODY) PARAMETERS (MODEL WORDS, CHUNKER BY SENTENCES MAX 1 OVERLAP 0)").unwrap();
    let columns = "SELECT DOC_ID, VECTOR_RANK, CHUNK_TEXT FROM HYBRID_SEARCH(INDEX = DOCS_H";
    let alone = read(&s, &format!("{columns}, QUERY_TEXT = 'damaged parcel', TOP_N = 3) AS H ORDER BY SCORE DESC, DOC_ID"));
    let given = read(
        &s,
        &format!(
            "{columns}, QUERY_VECTOR = EMBED('WORDS', 'damaged parcel', 'query'), QUERY_TEXT = 'damaged parcel', TOP_N = 3) AS H \
             ORDER BY SCORE DESC, DOC_ID"
        ),
    );
    assert_eq!(alone, given);
    assert_eq!(alone.first().map(String::as_str), Some("3,1,a damaged parcel is returned."), "{alone:?}");
    let but_two: Vec<String> = alone.iter().filter(|r| !r.starts_with("2,")).map(|r| format!("x,{r}")).collect();
    for text in ["?", "CAST(? AS VARCHAR(40))"] {
        let dynamic = format!(
            "SELECT CAST(? AS VARCHAR(4)), DOC_ID, VECTOR_RANK, CHUNK_TEXT FROM HYBRID_SEARCH(INDEX = DOCS_H, QUERY_TEXT = {text}, \
             TOP_N = 3) AS H WHERE DOC_ID <> CAST(? AS INTEGER) ORDER BY SCORE DESC, DOC_ID"
        );
        let values = [Value::Char("x".into()), Value::Char("damaged parcel".into()), Value::Char("2".into())];
        assert_eq!(prepared(&s, &dynamic, &values), but_two, "{text}");
    }
}

/// Verifies: [EXTENSION] vector_standard §11.1 ("a full-text index on
/// body"), §11.2, R11.2: of an index of a MODEL, CONTAINS and RANK of the
/// table's text read the index, a document a row, not a chunk; a document
/// found by the text list alone is given its chunk ranked highest.
#[test]
fn contains_and_rank_are_of_documents_11_2() {
    let (_db, s) = docs();
    s.run("CREATE HYBRID VECTOR INDEX DOCS_H ON DOCS (BODY) PARAMETERS (MODEL WORDS, CHUNKER BY SENTENCES MAX 1 OVERLAP 0)").unwrap();
    let contains = "SELECT ID FROM DOCS WHERE CONTAINS(BODY, '\"refund\"') = 1 ORDER BY ID";
    let shown = super::fixture::plan(&s, contains);
    assert!(shown.contains("'DOCS_H'"), "ranked by the index's statistics: {shown}");
    assert_eq!(read(&s, contains), ["1", "3"]);
    let ranked = "SELECT ID FROM DOCS WHERE RANK(BODY, 'IS ABOUT \"damaged\"') > 0 ORDER BY RANK(BODY, 'IS ABOUT \"damaged\"') DESC, ID";
    assert_eq!(read(&s, ranked), ["3", "1"], "one row a document");
    let text_alone = "SELECT DOC_ID, CHUNK_TEXT FROM HYBRID_SEARCH(INDEX = DOCS_H, QUERY_TEXT = 'shipping', \
                      QUERY_VECTOR = EMBED('WORDS', 'store opening hours'), TOP_N = 2, CANDIDATES = 1) AS H \
                      WHERE VECTOR_RANK IS NULL";
    assert_eq!(read(&s, text_alone), ["1,shipping is free"]);
}
