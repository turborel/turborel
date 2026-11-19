//! A hybrid index of a MODEL on a partitioned table.

use turborel::db::Database;
use turborel::threads::JobPolicy;

use crate::common::{exec, sorted};

/// Verifies: [EXTENSION] RULES.md D158: a hybrid index of a MODEL on a
/// partitioned table is a template, its full-text part made on each
/// partition, and keeps one table of chunks of the tree's rows, keyed by
/// the table's primary key, kept by its pipeline as rows are written to
/// any partition, moved between them, or deleted; HYBRID_SEARCH reads the
/// rows of every partition; DROP INDEX takes all of it.
#[test]
fn a_hybrid_index_of_a_model_serves_a_partitioned_table() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    for sql in [
        "CREATE EMBEDDING MODEL WORDS WITH (TYPE = 'HASH_DENSE', DIMENSIONS = 16, NGRAMS = 1)",
        "CREATE TABLE DOCS (ID INTEGER PRIMARY KEY, BODY VARCHAR(400)) PARTITION BY RANGE (ID)",
        "CREATE TABLE D0 PARTITION OF DOCS FOR VALUES FROM (0) TO (10)",
        "CREATE TABLE D1 PARTITION OF DOCS FOR VALUES FROM (10) TO (20)",
        "INSERT INTO DOCS VALUES (1, 'refund policy for damaged items. shipping is free'), \
         (12, 'store opening hours. we open at nine'), (13, 'a damaged parcel is returned. refund in a week')",
        "COMMIT",
        "CREATE HYBRID VECTOR INDEX DOCS_H ON DOCS (BODY) PARAMETERS (MODEL WORDS, CHUNKER BY SENTENCES MAX 1 OVERLAP 0)",
    ] {
        exec(&s, sql).unwrap();
    }
    let made = "SELECT PARTITION_NAME FROM INFORMATION_SCHEMA.PARTITION_INDEXES WHERE TEMPLATE_NAME = 'DOCS_H'";
    assert_eq!(sorted(&s, made), ["D0", "D1"]);
    assert_eq!(sorted(&s, "SELECT COUNT(*) FROM DOCS_H_CHUNKS"), ["6"]);
    let search = "SELECT DOC_ID FROM HYBRID_SEARCH(INDEX = DOCS_H, QUERY_TEXT = 'refund', \
                  QUERY_VECTOR = EMBED('WORDS', 'refund'), TOP_N = 2) AS H";
    assert_eq!(sorted(&s, search), ["1", "13"]);
    exec(&s, "INSERT INTO DOCS VALUES (2, 'refund desk. near the door')").unwrap();
    exec(&s, "UPDATE DOCS SET ID = 3 WHERE ID = 13").unwrap();
    exec(&s, "DELETE FROM DOCS WHERE ID = 1").unwrap();
    exec(&s, "COMMIT").unwrap();
    assert_eq!(sorted(&s, "SELECT DISTINCT DOC_ID FROM DOCS_H_CHUNKS"), ["12", "2", "3"]);
    assert_eq!(sorted(&s, search), ["2", "3"]);
    exec(&s, "COMMIT").unwrap();
    exec(&s, "DROP INDEX DOCS_H").unwrap();
    assert!(exec(&s, "SELECT COUNT(*) FROM DOCS_H_CHUNKS").is_err());
}

/// Verifies: [EXTENSION] RULES.md D158: HYBRID_SEARCH of a hybrid index of
/// a text and a vector column on a partitioned table reads the rows of
/// every partition through each one's index.
#[test]
fn a_hybrid_index_of_two_columns_serves_a_partitioned_table() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION S").unwrap();
    let s = db.session("S");
    for sql in [
        "CREATE TABLE DOCS (ID INTEGER PRIMARY KEY, BODY VARCHAR(40), EMB VECTOR(4, FLOAT32)) PARTITION BY RANGE (ID)",
        "CREATE TABLE D0 PARTITION OF DOCS FOR VALUES FROM (0) TO (10)",
        "CREATE TABLE D1 PARTITION OF DOCS FOR VALUES FROM (10) TO (20)",
        "INSERT INTO DOCS VALUES (1, 'refund policy', TO_VECTOR('[1,0,0,0]', 4, 'FLOAT32')), \
         (12, 'opening hours', TO_VECTOR('[0,1,0,0]', 4, 'FLOAT32')), (13, 'refund in a week', TO_VECTOR('[0.9,0.1,0,0]', 4, 'FLOAT32'))",
        "COMMIT",
        "CREATE HYBRID VECTOR INDEX DOCS_H ON DOCS (BODY, EMB)",
    ] {
        exec(&s, sql).unwrap();
    }
    let search = "SELECT DOC_ID FROM HYBRID_SEARCH(INDEX = DOCS_H, QUERY_TEXT = 'refund', \
                  QUERY_VECTOR = TO_VECTOR('[1,0,0,0]', 4, 'FLOAT32'), TOP_N = 2) AS H";
    assert_eq!(sorted(&s, search), ["1", "13"]);
}
