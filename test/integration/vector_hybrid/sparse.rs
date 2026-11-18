//! The inverted index of a SPARSE column (the vector design document
//! §10.3) as an approximate index (R13.1): FETCH APPROX of a DOT distance
//! reads it, its candidates re-ranked by the exact distance; a filter and
//! another metric.

use turborel::db::Session;

use super::fixture::{plan, read, register};

/// The dimensions of the column.
const DIMS: u32 = 100;
/// The rows of the table.
const ROWS: u32 = 2000;

/// `entries`, (dimension, weight) pairs, as a literal of the column's type.
fn sparse(entries: &[(u32, f64)]) -> String {
    let dims: Vec<String> = entries.iter().map(|(d, _)| d.to_string()).collect();
    let weights: Vec<String> = entries.iter().map(|(_, w)| format!("{w}")).collect();
    format!("TO_VECTOR('[{DIMS},[{}],[{}]]', {DIMS}, 'FLOAT32', 'SPARSE')", dims.join(","), weights.join(","))
}

/// The column's type: VECTOR(100, FLOAT32, SPARSE), its format FLOAT32's
/// code, 1, plus SPARSE's 256, as the type keeps them.
const SPARSE_TYPE: &str = "VECTOR(100, 257)";

/// Schema S's table TERMS (ID INTEGER, SV of `SPARSE_TYPE`, C INTEGER):
/// row k holds dimension k mod 100 at weight 1 plus k / 1000 and
/// dimension 99 at 0.5, and C k mod 3; its index TERMS_SV of `index`.
fn terms(index: &str) -> Session {
    let db = turborel::db::Database::new();
    register(&db);
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION S CREATE TABLE TERMS (ID INTEGER, SV {SPARSE_TYPE}, C INTEGER)")).unwrap();
    let s = db.session("S");
    s.run("SET SQL.FUNCTION_CALLS = 'ON'").unwrap();
    let values: Vec<String> =
        (0..ROWS).map(|k| format!("({k}, {}, {})", sparse(&[(k % 98, 1.0 + f64::from(k) / 1000.0), (99, 0.5)]), k % 3)).collect();
    for chunk in values.chunks(250) {
        s.run(&format!("INSERT INTO TERMS VALUES {}", chunk.join(", "))).unwrap();
    }
    s.run("INSERT INTO TERMS VALUES (-1, NULL, 0)").unwrap();
    s.commit().unwrap();
    s.run(index).unwrap();
    s.commit().unwrap();
    s
}

/// The rows nearest dimension 7 by `metric`, `fetch` of them, with `filter`.
fn nearest(metric: &str, fetch: &str, filter: &str) -> String {
    format!(
        "SELECT ID FROM TERMS {filter} ORDER BY VECTOR_DISTANCE(SV, {}, '{metric}') FETCH {fetch} FIRST 3 ROWS ONLY",
        sparse(&[(7, 1.0)])
    )
}

/// Verifies: [EXTENSION] vector_standard §10.3, R13.1, R13.5: an index of
/// ORGANIZATION INVERTED is read for FETCH APPROX of a DOT distance of its
/// SPARSE column, EXPLAIN naming it and the metric; its rows are the
/// exact search's, of the greatest dot product first; FETCH EXACT and a
/// distance of another metric read the table.
#[test]
fn fetch_approx_of_dot_reads_the_inverted_index() {
    let s = terms("CREATE VECTOR INDEX TERMS_SV ON TERMS (SV) ORGANIZATION INVERTED");
    let approx = nearest("DOT", "APPROX", "");
    let shown = plan(&s, &approx);
    assert!(shown.contains("NearestScan S.TERMS using TERMS_SV [inverted]"), "{shown}");
    assert!(shown.contains("approximate DOT"), "{shown}");
    assert_eq!(read(&s, &approx), ["1967", "1869", "1771"]);
    assert_eq!(read(&s, &approx), read(&s, &nearest("DOT", "EXACT", "")));
    assert!(!plan(&s, &nearest("DOT", "EXACT", "")).contains("NearestScan"));
    assert!(!plan(&s, &nearest("COSINE", "APPROX", "")).contains("NearestScan"));
    assert_eq!(read(&s, &nearest("COSINE", "APPROX", "")), read(&s, &nearest("COSINE", "EXACT", "")));
}

/// Verifies: [EXTENSION] vector_standard R13.2, R13.4: a
/// condition on other columns is asked of the index's candidates as it
/// finds them under VECTOR_INFILTER, or of the candidates it gives under
/// VECTOR_POSTFILTER, each giving the exact search's rows.
#[test]
fn a_filter_narrows_the_inverted_index() {
    let s = terms("CREATE INDEX TERMS_SV ON TERMS (SV) USING SPARSE");
    let exact = read(&s, &nearest("DOT", "EXACT", "WHERE C = 0"));
    assert_eq!(exact, ["1869", "1575", "1281"]);
    for hint in ["VECTOR_INFILTER", "VECTOR_POSTFILTER"] {
        let q = format!("{} HINT ({hint})", nearest("DOT", "APPROX", "WHERE C = 0"));
        let shown = plan(&s, &q);
        assert!(shown.contains("using TERMS_SV [inverted]"), "{shown}");
        assert_eq!(read(&s, &q), exact, "{hint}");
    }
}

/// Verifies: [EXTENSION] RULES.md D94, D99, vector_standard R6.4: a query
/// vector of other dimensions, which VECTOR_DISTANCE refuses, is refused
/// by an approximate search as by an exact one; rows changed after the
/// index was made are found, a deleted one no more.
#[test]
fn the_inverted_index_follows_the_rows_and_refuses_as_the_call_does() {
    let s = terms("CREATE VECTOR INDEX TERMS_SV ON TERMS (SV) ORGANIZATION INVERTED");
    let wide = "SELECT ID FROM TERMS ORDER BY VECTOR_DISTANCE(SV, TO_VECTOR('[101,[7],[1]]', 101, 'FLOAT32', 'SPARSE'), 'DOT') \
                FETCH APPROX FIRST 3 ROWS ONLY";
    let exact = wide.replace("APPROX", "EXACT");
    assert_eq!(crate::common::rows(&s, wide).err(), crate::common::rows(&s, &exact).err());
    assert!(crate::common::rows(&s, wide).is_err());
    s.run(&format!("INSERT INTO TERMS VALUES (5000, {}, 0)", sparse(&[(7, 9.0)]))).unwrap();
    s.run("DELETE FROM TERMS WHERE ID = 1967").unwrap();
    s.commit().unwrap();
    assert_eq!(read(&s, &nearest("DOT", "APPROX", "")), ["5000", "1869", "1771"]);
}
