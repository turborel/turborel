//! The built-in vector distances answered by the vector index methods
//! (the vector design document §9, §10, R13.1; RULES.md D94, D93):
//! VECTOR_DISTANCE in both orders and of a parameter, VECTOR_SEARCH, a
//! range, the accuracy report, each index's TARGET_ACCURACY, a column
//! declared NORMALIZED, and a registered metric named `schema.name`.

use turborel::db::{Database, Session};
use turborel::error::{Code, Result};
use turborel::params::ParameterType;
use turborel::value::Value;
use turborel::vector::metric::{MetricProperties, VectorMetric};
use turborel::vector::{Element, VectorRef, VectorType};

use super::fixture::{DIMS, K, METHODS, QUERIES, database, literal, nearest_sql, plan, query_of, recall, register, unit, vector_of};
use crate::common::rows;

/// The rows of the tables searched.
const ROWS: u64 = 2000;
/// The least recall@10 asked of an index of default options.
const LEAST_RECALL: f64 = 0.9;

/// CREATE INDEX of `method` on T(V) of `metric`, and `more` options.
fn create(s: &Session, method: &str, (metric, more): (&str, &str)) {
    let group = method.to_ascii_uppercase();
    s.commit().unwrap();
    s.run(&format!("CREATE INDEX T_V ON T (V) USING {method} WITH ({group}.METRIC = {metric}{more})")).unwrap();
    s.commit().unwrap();
}

fn drop_index(s: &Session) {
    s.commit().unwrap();
    s.run("DROP INDEX T_V").unwrap();
    s.commit().unwrap();
}

/// The share of the rows `exact` finds that `approx` finds too, of each
/// query `n` of `QUERIES`.
fn agreement(s: &Session, exact: &dyn Fn(u64) -> String, approx: &dyn Fn(u64) -> String) -> f64 {
    let (mut found, mut asked) = (0_usize, 0_usize);
    for n in 0..QUERIES as u64 {
        let want = rows(s, &exact(n)).unwrap();
        let got = rows(s, &approx(n)).unwrap();
        asked += want.len();
        found += got.iter().filter(|r| want.contains(r)).count();
    }
    f64::from(u32::try_from(found).unwrap()) / f64::from(u32::try_from(asked.max(1)).unwrap())
}

/// Verifies: [EXTENSION] vector_standard §6.1 R6.1, §13 R13.1: the
/// built-in VECTOR_DISTANCE, its metric named after its vectors or before
/// them, is served by an index of each method of its metric, EXPLAIN
/// naming the index, its rows nine in ten of FETCH EXACT's; of another
/// metric, or FETCH EXACT, it is not.
#[test]
fn vector_distance_is_served_by_each_method_r13_1() {
    let db = database(ROWS);
    let s = db.session("S");
    for method in METHODS {
        create(&s, method, ("EUCLIDEAN", ""));
        let after = |fetch: &str, n: u64| {
            format!(
                "SELECT ID FROM T ORDER BY VECTOR_DISTANCE(V, {}, 'EUCLIDEAN') FETCH {fetch} FIRST {K} ROWS ONLY",
                literal(&query_of(n))
            )
        };
        let before = |n: u64| {
            format!("SELECT ID FROM T ORDER BY VECTOR_DISTANCE('euclidean', {}, V) FETCH APPROX FIRST {K} ROWS ONLY", literal(&query_of(n)))
        };
        for sql in [after("APPROX", 0), before(0)] {
            assert!(plan(&s, &sql).contains(&format!("NearestScan S.T using T_V [{method}]")), "{method}: {sql}");
        }
        assert!(!plan(&s, &after("EXACT", 0)).contains("NearestScan"), "{method}");
        let cosine = after("APPROX", 0).replace("'EUCLIDEAN'", "'COSINE'");
        assert!(!plan(&s, &cosine).contains("NearestScan"), "{method}");
        let found = agreement(&s, &|n| after("EXACT", n), &|n| after("APPROX", n));
        assert!(found >= LEAST_RECALL, "{method}: recall@10 {found}");
        assert!(agreement(&s, &|n| after("EXACT", n), &before) >= LEAST_RECALL, "{method}");
        drop_index(&s);
    }
}

/// Verifies: [EXTENSION] vector_standard §9.2, [ISO/IEC 9075:1992 §17.6
/// GR5]: a `?` in a distance's vector place takes the type of the column
/// it is measured against, so a query vector given as text is read in the
/// column's dimensions and format, and the prepared search of an index
/// gives the rows the literal one does.
#[test]
fn a_parameter_takes_the_columns_vector_type() {
    let db = database(ROWS);
    let s = db.session("S");
    create(&s, "hnsw", ("EUCLIDEAN", ""));
    let p = s.prepare(&format!("SELECT ID FROM T ORDER BY L2_DISTANCE(V, ?) FETCH APPROX FIRST {K} ROWS ONLY")).unwrap();
    let ParameterType::Of(t) = p.parameters().first().unwrap() else { panic!("a type") };
    // The column's type, VECTOR(16, FLOAT32), written as its default
    // format leaves it.
    assert_eq!(t.to_string(), format!("VECTOR({DIMS})"));
    let text: Vec<String> = query_of(3).iter().map(|x| format!("{x}")).collect();
    let mut cursor = s.open_prepared(&p, &[Value::Char(format!("[{}]", text.join(",")).into())]).unwrap();
    let mut got = Vec::new();
    while let Some(row) = cursor.fetch().unwrap() {
        got.push(row.first().unwrap().to_string());
    }
    drop(cursor);
    assert_eq!(got, rows(&s, &nearest_sql("L2_DISTANCE", 3, ("APPROX", ""))).unwrap());
    let general = s.prepare("SELECT VECTOR_DISTANCE(V, ?, ?) FROM T").unwrap();
    assert_eq!(general.parameters().len(), 2);
    assert!(matches!(general.parameters().get(1), Some(ParameterType::Of(t)) if t.is_character()));
}

/// Verifies: [EXTENSION] vector_standard §9.3: VECTOR_SEARCH of the
/// built-in distance is an approximate read of an index of each method,
/// its TOP_N rows the exact nearest nine in ten.
#[test]
fn vector_search_reads_each_method_9_3() {
    let db = database(ROWS);
    let s = db.session("S");
    for method in METHODS {
        create(&s, method, ("EUCLIDEAN", ""));
        let search = |n: u64| {
            format!(
                "SELECT R.ID FROM VECTOR_SEARCH(TABLE = T, COLUMN = V, SIMILAR_TO = {}, METRIC = 'EUCLIDEAN', TOP_N = {K}) AS R",
                literal(&query_of(n))
            )
        };
        assert!(plan(&s, &search(0)).contains(&format!("[{method}]")), "{method}: {}", plan(&s, &search(0)));
        let found = agreement(&s, &|n| nearest_sql("L2_DISTANCE", n, ("EXACT", "")), &search);
        assert!(found >= LEAST_RECALL, "{method}: recall@10 {found}");
        drop_index(&s);
    }
}

/// Verifies: [EXTENSION] vector_standard §9.4 R9.1, RULES.md D94: a
/// WHERE condition of the distance itself, a range, does not bar the
/// index, as a built-in distance of a fixed-shape column raises alike for
/// every row (`rechecked_errors`); the rows are those within the range.
#[test]
fn a_range_of_the_distance_is_served_r9_1() {
    let db = database(ROWS);
    let s = db.session("S");
    create(&s, "diskann", ("EUCLIDEAN", ""));
    let q = literal(&query_of(1));
    let ranged = |fetch: &str| {
        format!("SELECT ID FROM T WHERE L2_DISTANCE(V, {q}) < 6.0E0 ORDER BY L2_DISTANCE(V, {q}) FETCH {fetch} FIRST 5 ROWS ONLY")
    };
    assert!(plan(&s, &ranged("APPROX")).contains("NearestScan"), "{}", plan(&s, &ranged("APPROX")));
    assert_eq!(rows(&s, &ranged("APPROX")).unwrap(), rows(&s, &ranged("EXACT")).unwrap());
}

/// Verifies: [EXTENSION] vector_standard §10.6: INDEX_ACCURACY_REPORT of
/// an index of each method searches each sample by the built-in
/// VECTOR_DISTANCE of its metric, both ways; of no index it is refused.
#[test]
fn the_accuracy_report_measures_each_method_10_6() {
    let db = database(ROWS);
    let s = db.session("S");
    let samples: Vec<Value> = (0..4).map(|n| s_vector(&s, &query_of(n))).collect();
    for method in METHODS {
        create(&s, method, ("EUCLIDEAN", ""));
        let report = s.index_accuracy_report("T_V", &samples, K).unwrap();
        assert_eq!((report.queries, report.expected), (4, 4 * K), "{method}");
        assert!(report.recall() >= LEAST_RECALL, "{method}: {report:?}");
        drop_index(&s);
    }
    assert_eq!(s.index_accuracy_report("NONE_SUCH", &samples, K).unwrap_err().code, Code::SyntaxRule);
}

/// `v` as a VECTOR value of T's column.
fn s_vector(s: &Session, v: &[f64]) -> Value {
    let mut cursor = s.open_cursor(&format!("SELECT {} FROM T WHERE ID = 0", literal(v))).unwrap();
    cursor.fetch().unwrap().unwrap().first().unwrap().clone()
}

/// Verifies: [EXTENSION] vector_standard §10.2, §10.4, R9.4, R10.2: HNSW
/// and IVF declare TARGET_ACCURACY, PQ_BYTES and NORMALIZED as DiskANN
/// does; an index's TARGET_ACCURACY is what a search of none of its own
/// aims at, 100 finding nearly every exact row.
#[test]
fn each_method_honours_its_target_accuracy_10_2() {
    let db = database(ROWS);
    let s = db.session("S");
    for method in METHODS {
        let group = method.to_ascii_uppercase();
        for option in ["TARGET_ACCURACY", "PQ_BYTES", "NORMALIZED", "METRIC", "QUANTIZER", "RERANK"] {
            let sql = format!(
                "SELECT COUNT(*) FROM INFORMATION_SCHEMA.INDEX_OPTIONS WHERE INDEX_NAME = 'T_V' AND OPTION_NAME = '{group}.{option}'"
            );
            create(&s, method, ("EUCLIDEAN", ""));
            assert_eq!(rows(&s, &sql).unwrap(), ["1"], "{method}: {option}");
            drop_index(&s);
        }
        create(&s, method, ("EUCLIDEAN", &format!(", {group}.TARGET_ACCURACY = 100")));
        let best = recall(&s, "L2_DISTANCE", "");
        assert!(best >= 0.99, "{method}: recall@10 at 100% {best}");
        drop_index(&s);
    }
}

/// A vector of `seed`, each element uniform in [0, 10): of no clusters, so
/// IVF's lists split the nearest rows of a query among them.
fn uniform(seed: u64) -> Vec<f64> {
    (0..DIMS as u64).map(|d| (unit(seed * 977 + d) * 1000.0).round() / 100.0).collect()
}

/// Verifies: [EXTENSION] vector_standard §10.4, R9.4: of vectors in no
/// clusters, an IVF index of 40 lists, trained, searches all of them at a
/// TARGET_ACCURACY of 100, every exact row found, and one at 1, most
/// missed; with its own PROBES set, it searches that many, whatever its
/// accuracy.
#[test]
fn ivf_probes_follow_its_target_accuracy_10_4() {
    let db = Database::new();
    register(&db);
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (ID INTEGER, V VECTOR({DIMS}, FLOAT32))")).unwrap();
    let s = db.session("S");
    for start in (0..ROWS).step_by(250) {
        let values: Vec<String> = (start..start + 250).map(|i| format!("({i}, {})", literal(&uniform(i)))).collect();
        s.run(&format!("INSERT INTO T VALUES {}", values.join(","))).unwrap();
    }
    let query = |fetch: &str, n: u64| {
        format!("SELECT ID FROM T ORDER BY L2_DISTANCE(V, {}) FETCH {fetch} FIRST {K} ROWS ONLY", literal(&uniform(n + 1_000_000)))
    };
    let found = |s: &Session| agreement(s, &|n| query("EXACT", n), &|n| query("APPROX", n));
    create(&s, "ivf", ("EUCLIDEAN", ", IVF.LISTS = 40, IVF.TARGET_ACCURACY = 100"));
    // Filled row by row, it trains its lists in maintenance.
    s.commit().unwrap();
    db.maintain().unwrap();
    assert!(found(&s) >= 0.999, "every list: recall@10 {}", found(&s));
    for (set, what) in [("IVF.TARGET_ACCURACY = 1", "its accuracy"), ("IVF.TARGET_ACCURACY = 100, IVF.PROBES = 1", "its own probes")] {
        s.commit().unwrap();
        s.run(&format!("ALTER INDEX T_V SET ({set})")).unwrap();
        s.commit().unwrap();
        let one = found(&s);
        assert!(one < 0.8, "one list of forty, of {what}: recall@10 {one}");
    }
}

/// Makes S.N (ID INTEGER, V VECTOR(16, FLOAT32)) of `n` rows, each of the
/// fixture's vectors normalized.
fn normalized_table(db: &Database, n: u64) {
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION S CREATE TABLE N (ID INTEGER, V VECTOR({DIMS}, FLOAT32))")).unwrap();
    let s = db.session("S");
    for start in (0..n).step_by(250) {
        let values: Vec<String> =
            (start..n.min(start + 250)).map(|i| format!("({i}, VECTOR_NORMALIZE({}))", literal(&vector_of(i)))).collect();
        s.run(&format!("INSERT INTO N VALUES {}", values.join(","))).unwrap();
    }
    s.commit().unwrap();
}

/// Verifies: [EXTENSION] vector_standard R13.1 ("COSINE may use a DOT
/// index when the column is flagged normalized"), R8.2: of each method,
/// an index of DOT whose NORMALIZED is ON serves COSINE and EUCLIDEAN
/// searches, nine in ten of the exact rows, a graph measuring by
/// EUCLIDEAN; a COSINE range is the exact one's; a vector whose norm is
/// not 1 is refused (SQLCODE -301), and the table is unchanged. Without
/// NORMALIZED, IVF's DOT index does not serve COSINE.
#[test]
fn cosine_uses_a_dot_index_of_a_normalized_column_r13_1() {
    let db = Database::new();
    register(&db);
    normalized_table(&db, ROWS);
    let s = db.session("S");
    let query = |metric: &str, fetch: &str, n: u64| {
        format!("SELECT ID FROM N ORDER BY VECTOR_DISTANCE(V, {}, '{metric}') FETCH {fetch} FIRST {K} ROWS ONLY", literal(&query_of(n)))
    };
    for method in METHODS {
        let group = method.to_ascii_uppercase();
        s.commit().unwrap();
        s.run(&format!("CREATE INDEX N_V ON N (V) USING {method} WITH ({group}.METRIC = DOT, {group}.NORMALIZED = ON)")).unwrap();
        s.commit().unwrap();
        for metric in ["COSINE", "EUCLIDEAN", "DOT"] {
            assert!(plan(&s, &query(metric, "APPROX", 0)).contains(&format!("[{method}]")), "{method}: {metric}");
            let found = agreement(&s, &|n| query(metric, "EXACT", n), &|n| query(metric, "APPROX", n));
            assert!(found >= LEAST_RECALL, "{method} {metric}: recall@10 {found}");
        }
        let q = literal(&query_of(2));
        let ranged = |fetch: &str| {
            format!(
                "SELECT ID FROM N WHERE COSINE_DISTANCE(V, {q}) < 0.002E0 ORDER BY COSINE_DISTANCE(V, {q}) FETCH {fetch} FIRST 5 ROWS ONLY"
            )
        };
        assert_eq!(rows(&s, &ranged("APPROX")).unwrap(), rows(&s, &ranged("EXACT")).unwrap(), "{method}");
        let raw = s.run(&format!("INSERT INTO N VALUES (-5, {})", literal(&vector_of(5)))).unwrap_err();
        assert_eq!(raw.code, Code::Data, "{method}: {raw}");
        s.rollback().unwrap();
        assert_eq!(rows(&s, "SELECT COUNT(*) FROM N WHERE ID = -5").unwrap(), ["0"]);
        s.commit().unwrap();
        s.run("DROP INDEX N_V").unwrap();
        s.commit().unwrap();
    }
    s.commit().unwrap();
    s.run("CREATE INDEX N_V ON N (V) USING ivf WITH (IVF.METRIC = DOT)").unwrap();
    s.commit().unwrap();
    assert!(!plan(&s, &query("COSINE", "APPROX", 0)).contains("NearestScan"));
    assert!(plan(&s, &query("DOT", "APPROX", 0)).contains("NearestScan"));
}

/// A metric of a name, MANHATTAN's distance of FLOAT32 vectors.
struct Manhattan(&'static str);

impl VectorMetric for Manhattan {
    fn name(&self) -> &str {
        self.0
    }
    fn formats(&self) -> &[Element] {
        &[Element::Float32]
    }
    fn properties(&self) -> MetricProperties {
        MetricProperties { symmetric: true, non_negative: true, triangle: true, normalized_as: None }
    }
    fn distance(&self, a: VectorRef<'_>, b: VectorRef<'_>) -> Result<Option<f64>> {
        Ok(Some((0..a.dims).map(|d| (a.at(d).unwrap_or_default() - b.at(d).unwrap_or_default()).abs()).sum()))
    }
}

/// Verifies: [EXTENSION] vector_standard §10.2 `DISTANCE schema.udf_name`,
/// §6.1 R6.2, R13.1: a metric registered as S.MY_L1 is a CREATE VECTOR
/// INDEX's DISTANCE of each method, and a search of VECTOR_DISTANCE of it
/// reads the index, nine in ten of the exact rows; a DISTANCE no metric's
/// is SQLCODE -103; a metric of another name form, or registered twice, is
/// refused (-910).
#[test]
fn a_registered_distance_function_is_an_index_metric_10_2() {
    let db = database(ROWS);
    db.register_metric(Box::new(Manhattan("S.MY_L1"))).unwrap();
    assert_eq!(db.register_metric(Box::new(Manhattan("S.MY_L1"))).unwrap_err().code, Code::Api);
    assert_eq!(db.register_metric(Box::new(Manhattan("s.lower"))).unwrap_err().code, Code::Api);
    assert_eq!(db.register_metric(Box::new(Manhattan("A.B.C"))).unwrap_err().code, Code::Api);
    let s = db.session("S");
    let query = |fetch: &str, n: u64| {
        format!("SELECT ID FROM T ORDER BY VECTOR_DISTANCE(V, {}, 'S.MY_L1') FETCH {fetch} FIRST {K} ROWS ONLY", literal(&query_of(n)))
    };
    for (organization, method) in [("DISK NEIGHBOR GRAPH", "diskann"), ("INMEMORY NEIGHBOR GRAPH", "hnsw"), ("NEIGHBOR PARTITIONS", "ivf")]
    {
        s.commit().unwrap();
        s.run(&format!("CREATE VECTOR INDEX T_V ON T (V) ORGANIZATION {organization} DISTANCE S.MY_L1")).unwrap();
        s.commit().unwrap();
        assert!(plan(&s, &query("APPROX", 0)).contains(&format!("[{method}]")), "{method}");
        let found = agreement(&s, &|n| query("EXACT", n), &|n| query("APPROX", n));
        assert!(found >= LEAST_RECALL, "{method}: recall@10 {found}");
        assert_eq!(rows(&s, &query("APPROX", 0)).unwrap().len(), K);
        drop_index(&s);
    }
    s.commit().unwrap();
    let unknown = s.run("CREATE VECTOR INDEX T_V ON T (V) DISTANCE S.NO_SUCH").unwrap_err();
    assert_eq!(unknown.code, Code::SyntaxRule);
}

/// Verifies: [EXTENSION] RULES.md D93, vector_standard §3.1 R3.2, R3.3:
/// VECTOR and its functions are built in, so a new database takes a
/// VECTOR(n, FLOAT32) column and VECTOR_DISTANCE with nothing registered,
/// and a second VECTOR type is refused (SQLCODE -910).
#[test]
fn vector_is_built_in_d88() {
    let db = Database::new();
    assert_eq!(db.register_type(Box::new(VectorType)).unwrap_err().code, Code::Api);
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE W (V VECTOR(3, float32), B VECTOR(8, BINARY, DENSE), F VECTOR(*, *))").unwrap();
    let s = db.session("S");
    s.run("INSERT INTO W VALUES ('[3,4,0]', NULL, '[1,2]')").unwrap();
    assert_eq!(rows(&s, "SELECT VECTOR_DISTANCE(V, '[0,0,0]', 'EUCLIDEAN') FROM W").unwrap(), ["5E0"]);
    assert_eq!(rows(&s, "SELECT VECTOR_DIMENSION_COUNT(F) FROM W").unwrap(), ["2"]);
}

/// Verifies: [EXTENSION] vector_standard R3.11, R10.6: there is no CREATE
/// UNIQUE INDEX statement (SQLCODE -102); a UNIQUE index is a UNIQUE
/// constraint's, of TABLES.UNIQUE_INDEX_METHOD, and a UNIQUE constraint
/// of a vector column is refused (-103) whichever method that names,
/// vectors being compared by no `=`.
#[test]
fn each_method_refuses_a_unique_index_r3_11() {
    let db = database(10);
    let s = db.session("S");
    for method in METHODS {
        assert_eq!(s.run(&format!("CREATE UNIQUE INDEX X ON T (V) USING {method}")).unwrap_err().code, Code::Syntax, "{method}");
        s.rollback().unwrap();
        db.set_setting("TABLES.UNIQUE_INDEX_METHOD", &Value::Char(method.into())).unwrap();
        let unique = s.run(&format!("CREATE TABLE U_{method} (V VECTOR({DIMS}, FLOAT32) UNIQUE)")).unwrap_err();
        assert_eq!(unique.code, Code::SyntaxRule, "{method}: {unique}");
        s.rollback().unwrap();
    }
}
