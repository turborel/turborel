//! Vector indexes of columns of every element format and the metrics that
//! measure them (RULES.md D93, D94, D95): BINARY columns by HAMMING and
//! JACCARD; a registered metric, given each vector in its column's own
//! format; a metric that gives null or fails.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use turborel::db::{Database, Session};
use turborel::error::{Code, Result, SqlError};
use turborel::vector::metric::{MetricProperties, VectorMetric};
use turborel::vector::{Element, VectorRef};

use super::fixture::{K, METHODS, database, insert, literal, plan, query_of, register, unit};
use crate::common::rows;

/// The dimensions of the tables' vectors: eight bytes of a BINARY one.
const DIMS: u64 = 64;
/// The rows of the tables.
const ROWS: u64 = 300;
/// The queries agreement is measured over.
const QUERIES: u64 = 5;
/// The least share of an approximate answer within the exact one's
/// farthest distance.
const LEAST: f64 = 0.9;

/// Element `d` of row `id`'s vector, in [0, 1.3): near one of 16 centres.
fn element(id: u64, d: u64) -> f64 {
    unit(id % 16 * 131 + d) + unit(id * 977 + d) * 0.3
}

/// Row `id`'s vector as a SQL literal of `format`: a bit set where its
/// element passes 0.65, of BINARY; a whole number of fifty times it, of
/// INT8; to two places, of FLOAT16.
fn vector(format: Element, id: u64) -> String {
    let column = name(format);
    let elements: Vec<String> = match format {
        Element::Binary => {
            let bits: Vec<u32> = (0..DIMS).map(|d| u32::from(element(id, d) > 0.65)).collect();
            bits.chunks(8).map(|byte| byte.iter().fold(0, |b, x| b * 2 + x).to_string()).collect()
        }
        Element::Int8 => (0..DIMS).map(|d| format!("{}", (element(id, d) * 50.0).round())).collect(),
        Element::Float16 | Element::Float32 | Element::Float64 => {
            (0..DIMS).map(|d| format!("{}", (element(id, d) * 100.0).round() / 100.0)).collect()
        }
    };
    format!("CAST('[{}]' AS VECTOR({DIMS}, {column}))", elements.join(","))
}

/// The SQL name of `format`.
fn name(format: Element) -> &'static str {
    match format {
        Element::Binary => "BINARY",
        Element::Int8 => "INT8",
        Element::Float16 => "FLOAT16",
        Element::Float32 => "FLOAT32",
        Element::Float64 => "FLOAT64",
    }
}

/// A database of S.B (ID INTEGER, V VECTOR(64, `format`)) of `ROWS`
/// rows, with `metrics` registered.
fn table(format: Element, metrics: Vec<Box<dyn VectorMetric>>) -> Database {
    let db = Database::new();
    register(&db);
    for metric in metrics {
        db.register_metric(metric).unwrap();
    }
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION S CREATE TABLE B (ID INTEGER, V VECTOR({DIMS}, {}))", name(format))).unwrap();
    let s = db.session("S");
    for start in (0..ROWS).step_by(100) {
        let values: Vec<String> = (start..start + 100).map(|i| format!("({i}, {})", vector(format, i))).collect();
        s.run(&format!("INSERT INTO B VALUES {}", values.join(","))).unwrap();
    }
    s.commit().unwrap();
    db
}

/// The distance of row V to query `n` of `format` by `metric`.
fn distance(format: Element, metric: &str, n: u64) -> String {
    format!("VECTOR_DISTANCE(V, {}, {metric})", vector(format, n * 7919 + 100_003))
}

/// The IDs of the `K` rows nearest query `n` of `format` by `metric`, as
/// `fetch` asks (APPROX, EXACT). Only the IDs are selected: a second call
/// of a metric that may raise would keep an index from the search (D94).
fn nearest(format: Element, metric: &str, (fetch, n): (&str, u64)) -> String {
    format!("SELECT ID FROM B ORDER BY {} FETCH {fetch} FIRST {K} ROWS ONLY", distance(format, metric, n))
}

/// The distances to query `n` of the rows `ids`.
fn distances(s: &Session, format: Element, metric: &str, (n, ids): (u64, &[String])) -> Vec<f64> {
    let sql = format!("SELECT {} FROM B WHERE ID IN ({})", distance(format, metric, n), ids.join(","));
    rows(s, &sql).unwrap().iter().map(|d| d.parse().unwrap()).collect()
}

/// The share of FETCH APPROX's rows no farther than FETCH EXACT's
/// farthest, over `QUERIES` queries, each answering the `K` rows asked:
/// measured by distance, as rows of one distance tie.
fn agreement(s: &Session, format: Element, metric: &str) -> f64 {
    let (mut within, mut asked) = (0_u32, 0_u32);
    for n in 0..QUERIES {
        let (exact, approx) =
            (rows(s, &nearest(format, metric, ("EXACT", n))).unwrap(), rows(s, &nearest(format, metric, ("APPROX", n))).unwrap());
        assert_eq!(approx.len(), K, "{metric}: FETCH APPROX answers the rows asked");
        let farthest = distances(s, format, metric, (n, &exact)).into_iter().fold(f64::MIN, f64::max);
        let found = distances(s, format, metric, (n, &approx));
        asked += u32::try_from(found.len()).unwrap();
        within += u32::try_from(found.iter().filter(|d| **d <= farthest).count()).unwrap();
    }
    f64::from(within) / f64::from(asked.max(1))
}

/// The CREATE VECTOR INDEX of `method` on B(V) by the registered metric
/// `metric`.
fn by_registered(method: &str, metric: &str) -> String {
    let organization = match method {
        "diskann" => "DISK NEIGHBOR GRAPH",
        "hnsw" => "INMEMORY NEIGHBOR GRAPH",
        _ => "NEIGHBOR PARTITIONS",
    };
    format!("CREATE VECTOR INDEX B_V ON B (V) ORGANIZATION {organization} DISTANCE {metric}")
}

/// Drops B_V, committed.
fn drop_index(s: &Session) {
    s.commit().unwrap();
    s.run("DROP INDEX B_V").unwrap();
    s.commit().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D93, D95: an index of each method on a
/// BINARY column by HAMMING or by JACCARD is read by FETCH APPROX of that
/// distance, and nine in ten of its answers are as near as the exact
/// nearest rows.
#[test]
fn binary_columns_are_indexed_by_hamming_and_jaccard() {
    let db = table(Element::Binary, Vec::new());
    let s = db.session("S");
    for method in METHODS {
        for metric in ["HAMMING", "JACCARD"] {
            let group = method.to_ascii_uppercase();
            s.run(&format!("CREATE INDEX B_V ON B (V) USING {method} WITH ({group}.METRIC = {metric})")).unwrap();
            s.commit().unwrap();
            db.maintain().unwrap();
            assert!(plan(&s, &nearest(Element::Binary, metric, ("APPROX", 0))).contains("NearestScan"), "{method}, {metric}");
            let found = agreement(&s, Element::Binary, metric);
            assert!(found >= LEAST, "{method}, {metric}: {found}");
            drop_index(&s);
        }
    }
}

/// MANHATTAN's distance of vectors of one format, the format each vector
/// it was given was in counted when it is not that one.
struct Manhattan {
    name: &'static str,
    format: [Element; 1],
    strays: Arc<AtomicUsize>,
}

impl VectorMetric for Manhattan {
    fn name(&self) -> &str {
        self.name
    }
    fn formats(&self) -> &[Element] {
        &self.format
    }
    fn properties(&self) -> MetricProperties {
        MetricProperties { symmetric: true, non_negative: true, triangle: true, normalized_as: None }
    }
    fn distance(&self, a: VectorRef<'_>, b: VectorRef<'_>) -> Result<Option<f64>> {
        let [format] = self.format;
        if a.element != format || b.element != format {
            self.strays.fetch_add(1, Ordering::SeqCst);
        }
        Ok(Some((0..a.dims).map(|d| (a.at(d).unwrap_or_default() - b.at(d).unwrap_or_default()).abs()).sum()))
    }
}

/// Verifies: [EXTENSION] RULES.md D93, D95: a registered metric of a
/// BINARY, INT8 or FLOAT16 column is an index's DISTANCE, and the index
/// gives it each vector in the column's own format, as a SQL call does,
/// never another; its approximate answers are as near as the exact ones,
/// nine in ten.
#[test]
fn a_registered_metric_is_given_its_columns_own_format() {
    for format in [Element::Binary, Element::Int8, Element::Float16] {
        let strays = Arc::new(AtomicUsize::new(0));
        let metric = Manhattan { name: "S.L1", format: [format], strays: Arc::clone(&strays) };
        let db = table(format, vec![Box::new(metric)]);
        let s = db.session("S");
        s.run(&by_registered("hnsw", "S.L1")).unwrap();
        s.commit().unwrap();
        let called = "'S.L1'";
        let shown = plan(&s, &nearest(format, called, ("APPROX", 0)));
        assert!(shown.contains("NearestScan"), "{format:?}: {shown}");
        let found = agreement(&s, format, called);
        assert!(found >= LEAST, "{format:?}: {found}");
        assert_eq!(strays.load(Ordering::SeqCst), 0, "{format:?}: every vector in the column's format");
    }
}

/// A metric of FLOAT32 vectors that gives null, or fails.
struct Unmeasuring {
    name: &'static str,
    fails: bool,
}

impl VectorMetric for Unmeasuring {
    fn name(&self) -> &str {
        self.name
    }
    fn formats(&self) -> &[Element] {
        &[Element::Float32]
    }
    fn properties(&self) -> MetricProperties {
        MetricProperties { symmetric: true, non_negative: true, triangle: true, normalized_as: None }
    }
    fn distance(&self, _: VectorRef<'_>, _: VectorRef<'_>) -> Result<Option<f64>> {
        if self.fails {
            return Err(SqlError::new(Code::Data, "[EXTENSION] the metric cannot measure"));
        }
        Ok(None)
    }
}

/// Verifies: [EXTENSION] RULES.md D94, D95: an index of each method by a
/// registered metric that gives null, or fails, is made and kept by
/// writes, which never fail for it, its entries taken as farthest; a
/// search by the metric that gives null answers the rows asked, and one by
/// the metric that fails raises what the exact search raises.
#[test]
fn a_metric_that_fails_never_fails_a_write() {
    for method in METHODS {
        let db = database(0);
        db.register_metric(Box::new(Unmeasuring { name: "S.NULLS", fails: false })).unwrap();
        db.register_metric(Box::new(Unmeasuring { name: "S.FAILS", fails: true })).unwrap();
        let s = db.session("S");
        insert(&s, (0, 100));
        for metric in ["S.NULLS", "S.FAILS"] {
            s.run(&by_registered(method, metric).replace("B_V ON B", "T_V ON T")).unwrap();
            s.commit().unwrap();
            insert(&s, (100, 200));
            db.maintain().unwrap();
            let search = |fetch: &str| {
                format!(
                    "SELECT ID FROM T ORDER BY VECTOR_DISTANCE(V, {}, '{metric}') FETCH {fetch} FIRST {K} ROWS ONLY",
                    literal(&query_of(0))
                )
            };
            let (exact, approx) = (rows(&s, &search("EXACT")), rows(&s, &search("APPROX")));
            match metric {
                "S.NULLS" => assert_eq!(approx.map(|r| r.len()), Ok(K), "{method}: the rows asked"),
                _ => assert_eq!((approx, exact.is_err()), (Err(Code::Data), true), "{method}: as the exact search"),
            }
            s.rollback().unwrap();
            s.run("DROP INDEX T_V").unwrap();
            s.commit().unwrap();
            s.run("DELETE FROM T WHERE ID >= 100").unwrap();
            s.commit().unwrap();
        }
    }
}
