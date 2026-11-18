//! The vector index methods "diskann", "hnsw" and "ivf" through SQL
//! (the vector design document §10, RULES.md D94): their recall
//! against FETCH EXACT, DML and rollback, recovery, options, memory and
//! what each refuses. The fixture is `vector_indexes/fixture.rs`;
//! recovery and refusals are `vector_indexes/durable.rs`; the built-in
//! distances they serve, `vector_indexes/served.rs`; their quantizers'
//! codes, `vector_indexes/quantized.rs`.

mod durable;
mod fixture;
mod quantized;
mod served;

use fixture::{METHODS, database, literal, nearest_sql, plan, query_of, recall, vector_of};
use turborel::error::Code;
use turborel::value::{Decimal, Value};

use crate::common::rows;

/// The least recall@10 of an index of each method over the fixture's
/// clustered vectors, its options the defaults.
const LEAST_RECALL: f64 = 0.9;
/// The rows of a table the recall is measured over.
const ROWS: u64 = 3000;

/// CREATE INDEX of `method` on T(V) ordered by `metric`.
fn create(method: &str, metric: &str) -> String {
    format!("CREATE INDEX T_V ON T (V) USING {method} WITH ({}.METRIC = {metric})", method.to_ascii_uppercase())
}

/// Verifies: [EXTENSION] vector_standard §10.1, §13 R13.1: CREATE INDEX
/// ... USING each method makes an index FETCH APPROX of a distance of its
/// metric reads, EXPLAIN naming it, whose ten nearest rows are, over the
/// fixture's queries, at least nine in ten of the exact ones; FETCH EXACT
/// and a distance of another metric never read it.
#[test]
fn each_method_finds_the_exact_nearest_rows_r13_1() {
    let db = database(ROWS);
    let s = db.session("S");
    for method in METHODS {
        s.run(&create(method, "EUCLIDEAN")).unwrap();
        s.commit().unwrap();
        let shown = plan(&s, &nearest_sql("L2_DISTANCE", 0, ("APPROX", "")));
        assert!(shown.contains(&format!("NearestScan S.T using T_V [{method}]")), "{method}: {shown}");
        assert!(!plan(&s, &nearest_sql("L2_DISTANCE", 0, ("EXACT", ""))).contains("NearestScan"), "{method}");
        assert!(!plan(&s, &nearest_sql("COSINE_DISTANCE", 0, ("APPROX", ""))).contains("NearestScan"), "{method}");
        let found = recall(&s, "L2_DISTANCE", "");
        assert!(found >= LEAST_RECALL, "{method}: recall@10 {found}");
        let filtered = recall(&s, "L2_DISTANCE", "WHERE C = 3");
        assert!(filtered >= LEAST_RECALL, "{method}: filtered recall@10 {filtered}");
        s.commit().unwrap();
        s.run("DROP INDEX T_V").unwrap();
        s.commit().unwrap();
    }
}

/// Verifies: [EXTENSION] vector_standard §10.2: CREATE VECTOR INDEX of each
/// organization makes an index of its method that FETCH APPROX reads, of
/// COSINE by default, as well as one made by CREATE INDEX.
#[test]
fn create_vector_index_makes_each_method_10_2() {
    let db = database(ROWS);
    let s = db.session("S");
    for (organization, method) in [("DISK NEIGHBOR GRAPH", "diskann"), ("INMEMORY NEIGHBOR GRAPH", "hnsw"), ("NEIGHBOR PARTITIONS", "ivf")]
    {
        s.run(&format!("CREATE VECTOR INDEX T_V ON T (V) ORGANIZATION {organization}")).unwrap();
        s.commit().unwrap();
        let shown = plan(&s, &nearest_sql("COSINE_DISTANCE", 1, ("APPROX", "")));
        assert!(shown.contains(&format!("[{method}]")), "{method}: {shown}");
        let found = recall(&s, "COSINE_DISTANCE", "");
        assert!(found >= LEAST_RECALL, "{method}: recall@10 {found}");
        s.commit().unwrap();
        s.run("DROP INDEX T_V").unwrap();
        s.commit().unwrap();
    }
}

/// Verifies: [EXTENSION] vector_standard §10.5, RULES.md D29, D94:
/// an index follows DML; a deleted row is never answered, an updated one
/// is found at its new vector, a null vector is not indexed, and a rolled
/// back insert is not answered, its entry taken back.
#[test]
fn dml_and_rollback_are_followed_10_5() {
    for method in METHODS {
        let db = database(1000);
        let s = db.session("S");
        s.run(&create(method, "EUCLIDEAN")).unwrap();
        s.commit().unwrap();
        let first = |s: &turborel::db::Session| rows(s, &nearest_sql("L2_DISTANCE", 0, ("APPROX", ""))).unwrap();
        let before = first(&s);
        let nearest = before.first().cloned().unwrap();
        s.run(&format!("DELETE FROM T WHERE ID = {nearest}")).unwrap();
        s.commit().unwrap();
        assert!(!first(&s).contains(&nearest), "{method}: a deleted row");
        s.run(&format!("UPDATE T SET V = {} WHERE ID = 500", literal(&query_of(0)))).unwrap();
        s.commit().unwrap();
        assert_eq!(first(&s).first().map(String::as_str), Some("500"), "{method}: an updated row");
        s.run(&format!("INSERT INTO T VALUES (9999, {}, 0)", literal(&query_of(0)))).unwrap();
        assert!(first(&s).contains(&"9999".to_owned()), "{method}: its own insert");
        s.rollback().unwrap();
        assert!(!first(&s).contains(&"9999".to_owned()), "{method}: a rolled back insert");
        s.run("UPDATE T SET V = NULL WHERE ID = 500").unwrap();
        s.commit().unwrap();
        assert!(!first(&s).contains(&"500".to_owned()), "{method}: a null vector");
        assert_eq!(rows(&s, &nearest_sql("L2_DISTANCE", 0, ("APPROX", ""))).unwrap().len(), 10);
    }
}

/// Verifies: [EXTENSION] RULES.md D41: a vector index's options are in
/// INFORMATION_SCHEMA.INDEX_OPTIONS; ALTER INDEX ... SET of an option that
/// changes in place (efSearch, a search list, NPROBE) keeps the index, and
/// of one that rebuilds it (M, R, the list count) rebuilds it; each still
/// answers.
#[test]
fn options_are_shown_and_altered_d41() {
    for (method, in_place, rebuilt) in [
        ("diskann", "DISKANN.SEARCH_LIST = 120", "DISKANN.MAX_DEGREE = 24"),
        ("hnsw", "HNSW.EF_SEARCH = 90", "HNSW.M = 12"),
        ("ivf", "IVF.PROBES = 6", "IVF.LISTS = 24"),
    ] {
        let db = database(1000);
        let s = db.session("S");
        s.run(&create(method, "EUCLIDEAN")).unwrap();
        s.commit().unwrap();
        let options = |s: &turborel::db::Session| {
            rows(
                s,
                "SELECT OPTION_NAME || '=' || OPTION_VALUE FROM INFORMATION_SCHEMA.INDEX_OPTIONS \
                 WHERE INDEX_NAME = 'T_V' AND VALUE_SOURCE <> 'DEFAULT' ORDER BY OPTION_NAME",
            )
            .unwrap()
        };
        assert_eq!(options(&s), [format!("{}.METRIC=EUCLIDEAN", method.to_ascii_uppercase())], "{method}");
        for set in [in_place, rebuilt] {
            s.commit().unwrap();
            s.run(&format!("ALTER INDEX T_V SET ({set})")).unwrap();
            s.commit().unwrap();
            assert!(options(&s).contains(&set.replace(" = ", "=")), "{method}: {set}");
            let found = recall(&s, "L2_DISTANCE", "");
            assert!(found >= LEAST_RECALL, "{method} after {set}: recall@10 {found}");
        }
    }
}

/// Verifies: [EXTENSION] vector_standard R10.3, R8.2, R3.11: each method
/// refuses (SQLCODE -900) an index of a column that is not a VECTOR, of
/// two columns, or of a metric it may not order by: the graphs a
/// metric that is not SYMMETRIC and NON_NEGATIVE (DOT), which IVF takes;
/// an unknown metric is SQLCODE -103.
#[test]
fn each_method_refuses_what_it_cannot_index_r10_3() {
    let db = database(100);
    let s = db.session("S");
    for method in METHODS {
        let group = method.to_ascii_uppercase();
        for (sql, code) in [
            (format!("CREATE INDEX X ON T (C) USING {method}"), Code::Unsupported),
            (format!("CREATE INDEX X ON T (V, C) USING {method}"), Code::Unsupported),
            (format!("CREATE INDEX X ON T (V) USING {method} WITH ({group}.METRIC = NO_SUCH_METRIC)"), Code::SyntaxRule),
        ] {
            assert_eq!(s.run(&sql).err().map(|e| e.code), Some(code), "{method}: {sql}");
            s.rollback().unwrap();
        }
        let dot = s.run(&create(method, "DOT"));
        assert_eq!(dot.err().map(|e| e.code), (method != "ivf").then_some(Code::Unsupported), "{method}: DOT");
        s.rollback().unwrap();
    }
}

/// Verifies: [EXTENSION] RULES.md D38: an index whose memory the budget
/// cannot hold is refused it (SQLCODE -918) rather than exceed it, and the
/// table is still answered exactly.
#[test]
fn a_budget_too_small_is_refused_or_read_around_d38() {
    for method in METHODS {
        let db = database(0);
        let s = db.session("S");
        s.run(&create(method, "EUCLIDEAN")).unwrap();
        s.commit().unwrap();
        db.set_setting("MEMORY.BUDGET_BYTES", &Value::Exact(Decimal::new(1 << 20, 0))).unwrap();
        let mut refused = false;
        for start in (0..20_000_u64).step_by(2000) {
            let values: Vec<String> = (start..start + 2000).map(|i| format!("({i}, {}, 0)", literal(&vector_of(i)))).collect();
            match s.run(&format!("INSERT INTO T VALUES {}", values.join(","))) {
                Ok(_) => s.commit().unwrap(),
                Err(e) => {
                    assert_eq!(e.code, Code::Resources, "{method}: {e}");
                    s.rollback().unwrap();
                    refused = true;
                    break;
                }
            }
        }
        let approx = rows(&s, &nearest_sql("L2_DISTANCE", 0, ("APPROX", ""))).unwrap();
        let exact = rows(&s, &nearest_sql("L2_DISTANCE", 0, ("EXACT", ""))).unwrap();
        assert!(refused || approx.iter().filter(|r| exact.contains(r)).count() >= 8, "{method}");
    }
}
