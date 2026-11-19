//! Vector indexes keeping codes of a database quantizer (the vector
//! design document §7.2, R13.1), and the metrics and quantizers an index
//! refuses.

use turborel::db::Session;

use super::fixture::{K, METHODS, database, recall};

/// The rows of the table searched.
const ROWS: u64 = 600;
/// The least recall@10 asked of an index of PQ codes, reranked.
const LEAST_RECALL: f64 = 0.6;

/// The SQLCODE of the refusal of `sql` in `s`, rolled back.
fn refused(s: &Session, sql: &str) -> i32 {
    let code = s.run(sql).expect_err(sql).sqlcode();
    s.rollback().unwrap();
    code
}

/// Verifies: [EXTENSION] vector_standard §7.2, §13 R13.1: an index of
/// each method keeping PQ codes, of the bytes PQ_BYTES asks, serves
/// FETCH APPROX of its metric, most of its rows those FETCH EXACT finds.
#[test]
fn each_method_serves_searches_by_pq_codes_7_2() {
    let db = database(ROWS);
    let s = db.session("S");
    s.commit().unwrap();
    for method in METHODS {
        let group = method.to_ascii_uppercase();
        s.run(&format!(
            "CREATE INDEX T_V ON T (V) USING {method} WITH ({group}.METRIC = EUCLIDEAN, {group}.QUANTIZER = PQ, {group}.PQ_BYTES = 4)"
        ))
        .unwrap();
        s.commit().unwrap();
        let found = recall(&s, "L2_DISTANCE", "");
        assert!(found >= LEAST_RECALL, "{method}: recall@{K} {found}");
        s.commit().unwrap();
        s.run("DROP INDEX T_V").unwrap();
        s.commit().unwrap();
    }
}

/// Verifies: [EXTENSION] vector_standard §8 R8.2, §7.2: an index of a
/// metric or quantizer the database does not know is refused, SQLCODE
/// -103; of a metric that does not measure its column's format, SQLCODE
/// -900.
#[test]
fn unknown_or_unfit_metrics_and_quantizers_are_refused_8_2() {
    let db = database(20);
    let s = db.session("S");
    s.commit().unwrap();
    for method in METHODS {
        let group = method.to_ascii_uppercase();
        let create = |option: &str| format!("CREATE INDEX T_V ON T (V) USING {method} WITH ({group}.{option})");
        assert_eq!(refused(&s, &create("METRIC = NO_SUCH_METRIC")), -103, "{method}");
        assert_eq!(refused(&s, &create("QUANTIZER = NO_SUCH_QUANTIZER")), -103, "{method}");
        assert_eq!(refused(&s, &create("METRIC = JACCARD")), -900, "{method}");
    }
}
