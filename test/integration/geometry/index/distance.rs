//! Distance comparisons through R-tree windows grown by their limits,
//! as filters and joins, raising what a scan would [ISO/IEC 13249-3:2016
//! (DIS, SC32 N2593) §5.1.41] (D15, D85, D135).

use super::populated;
use crate::common::{rows, state};
use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::geometry::Geometry;
use turborel::params::Params;
use turborel::value::Value;

/// Each query's rows with the index, its plan naming it, and the same
/// rows without it.
fn indexed_then_scanned(db: &Database, a: &Session, (index, queries): (&str, &[String])) -> Vec<Vec<String>> {
    let mut results = Vec::new();
    for query in queries {
        let plan = a.physical_plan(query, &Params::new()).unwrap();
        assert!(plan.contains(index), "{query}\n{plan}");
        results.push(rows(a, query).unwrap());
    }
    a.commit().unwrap();
    db.drop_index(index).unwrap();
    for (query, expected) in queries.iter().zip(&results) {
        assert_eq!(&rows(a, query).unwrap(), expected, "{query}");
    }
    results
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.41 Description
/// 2] D135: ST_Distance compared `<=` or `<` with a limit, either way
/// round, finds its rows through the R-tree, a polygon at exactly the
/// limit kept by `<=` alone. A zero limit keeps an intersecting geometry;
/// a negative or null limit and an empty geometry keep none. Dropping the
/// index gives the same rows.
#[test]
fn distance_filters_use_windows_and_equal_scans() {
    let db = populated();
    let a = db.session("A");
    let near = "ST_DISTANCE(G, ST_POINT(5, 5, 4326))";
    let conditions = [
        format!("{near} <= 2"),
        format!("{near} <= 3"),
        format!("{near} < 3"),
        "3 >= ST_DISTANCE(ST_POINT(5, 5, 4326), G)".to_owned(),
        "3.0E0 > ST_DISTANCE(ST_POINT(5, 5, 4326), G)".to_owned(),
        format!("{near} <= 0"),
        format!("{near} <= -1"),
        format!("{near} <= CAST(NULL AS DOUBLE PRECISION)"),
        "ST_DISTANCE(G, ST_GEOMFROMTEXT('POINT EMPTY', 4326)) <= 5".to_owned(),
        "ST_DISTANCE(G, ST_POINT(1000, 1000, 4326)) < 1.5 AND ID <> 999".to_owned(),
    ];
    let queries: Vec<String> = conditions.iter().map(|c| format!("SELECT ID FROM SHAPES WHERE {c} ORDER BY ID")).collect();
    let results = indexed_then_scanned(&db, &a, ("SHAPES_G", &queries));
    assert_eq!(results[0], ["4", "5", "6"]);
    assert_eq!(results[1], ["3", "4", "5", "6", "7", "2002"]);
    assert_eq!(results[2], ["3", "4", "5", "6", "7"]);
    assert_eq!(results[3], results[1]);
    assert_eq!(results[4], results[2]);
    assert_eq!(results[5], ["5"]);
    for none in &results[6..9] {
        assert_eq!(none.as_slice(), Vec::<String>::new().as_slice());
    }
    assert_eq!(results[9], ["1000", "1001"]);
}

/// Verifies: [DESIGN] D45, D135: a prepared distance comparison fills
/// both its geometry and its limit each execution, a null limit, a
/// negative one and an empty geometry finding none.
#[test]
fn distance_windows_bind_parameters() {
    let db = populated();
    let a = db.session("A");
    let query = a.prepare("SELECT ID FROM SHAPES WHERE ST_DISTANCE(G, ?) <= ? ORDER BY ID").unwrap();
    let point = |x: f64| Geometry::point(x, x, 4326).unwrap().to_value();
    let cases = [
        (point(100.0), Value::Approx(1.5), vec!["99", "100", "101"]),
        (point(300.5), Value::Approx(0.8), vec!["300", "301"]),
        (point(300.5), Value::Null, Vec::new()),
        (point(300.5), Value::Approx(-1.0), Vec::new()),
        (Geometry::from_wkt("POINT EMPTY", 4326).unwrap().to_value(), Value::Approx(9.0), Vec::new()),
    ];
    for (geometry, limit, expected) in cases {
        let mut cursor = a.open_prepared(&query, &[geometry, limit]).unwrap();
        let mut found = Vec::new();
        while let Some(row) = cursor.fetch().unwrap() {
            found.push(row[0].to_string());
        }
        assert_eq!(found, expected);
    }
    assert!(
        a.physical_plan("SELECT ID FROM SHAPES WHERE ST_DISTANCE(G, ST_POINT(1, 1, 4326)) <= 1", &Params::new())
            .unwrap()
            .contains("SHAPES_G")
    );
}

/// Verifies: [DESIGN] D15, D135: a distance window still raises what a
/// scan would. A geometry of another SRID reads every entry, so the error
/// is raised though no envelope is near, even of a null limit; one beyond
/// the finite-distance domain leaves the tree uncertified, so a distance
/// that overflows (22003) is reached; a unit a system cannot convert is
/// not probed, and its error is the scan's. Another
/// conjunct that can raise keeps the filter off the index.
#[test]
fn distance_windows_preserve_runtime_errors() {
    let db = populated();
    let a = db.session("A");
    for limit in ["1", "CAST(NULL AS DOUBLE PRECISION)"] {
        let query = format!("SELECT ID FROM SHAPES WHERE ST_DISTANCE(G, ST_POINT(-500, -500, 0)) <= {limit}");
        assert_eq!(rows(&a, &query).unwrap_err(), Code::Data, "{query}");
    }
    let prepared = a.prepare("SELECT ID FROM SHAPES WHERE ST_DISTANCE(G, ?) <= ? ORDER BY ID").unwrap();
    for id in 100..124_i32 {
        let point = Geometry::point(f64::from(id), f64::from(id), 4326).unwrap().to_value();
        let mut cursor = a.open_prepared(&prepared, &[point, Value::Approx(0.5)]).unwrap();
        assert_eq!(cursor.fetch().unwrap().unwrap()[0].to_string(), id.to_string());
        assert!(cursor.fetch().unwrap().is_none());
    }
    for limit in [Value::Approx(1.0), Value::Null] {
        let foreign = Geometry::point(-500.0, -500.0, 0).unwrap().to_value();
        assert_eq!(a.open_prepared(&prepared, &[foreign, limit]).and_then(|mut c| c.fetch()).unwrap_err().code, Code::Data);
    }
    a.run("CREATE TABLE FAR (ID INTEGER, G GEOMETRY(0))").unwrap();
    let near: Vec<String> = (0..300_i32).map(|id| format!("({id}, ST_POINT({id}, 0, 0))")).collect();
    a.run(&format!("INSERT INTO FAR VALUES {}, (1000, ST_POINT(1.7E308, 0, 0))", near.join(","))).unwrap();
    a.commit().unwrap();
    db.create_index("FAR_G", "A.FAR", &["G"], "rtree").unwrap();
    let overflow = "SELECT ID FROM FAR WHERE ST_DISTANCE(G, ST_POINT(-1.7E308, 0, 0)) <= 1";
    assert!(a.physical_plan(overflow, &Params::new()).unwrap().contains("FAR_G"));
    assert_eq!(state(&a, overflow), "22003");
    let geographic = "SELECT ID FROM SHAPES WHERE ST_DISTANCE(G, ST_POINT(5, 5, 4326), 'METRE') <= 1";
    let unknown = "SELECT ID FROM FAR WHERE ST_DISTANCE(G, ST_POINT(0, 0, 0), 'FURLONG') <= 1";
    let raising = "SELECT ID FROM SHAPES WHERE ST_DISTANCE(G, ST_POINT(5, 5, 4326)) <= 1 AND 1 / (ID - 100) > 0";
    let mut states = Vec::new();
    for query in [geographic, unknown, raising] {
        assert!(!a.physical_plan(query, &Params::new()).unwrap().contains("_G "), "{query}");
        states.push(state(&a, query));
    }
    a.commit().unwrap();
    db.drop_index("SHAPES_G").unwrap();
    db.drop_index("FAR_G").unwrap();
    assert_eq!(state(&a, overflow), "22003");
    for (query, expected) in [geographic, unknown, raising].into_iter().zip(states) {
        assert_eq!(state(&a, query), expected, "{query}");
    }
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.41 Description
/// 4] D135: a unit form's limit is in the unit named, its window in the
/// column's own units, so kilometres of an SRID 0 column find the points
/// within a thousand times as many coordinate units; EXPLAIN shows the
/// distance scaled. Dropping the index gives the same rows.
#[test]
fn distance_unit_forms_scale_their_windows() {
    let db = super::super::registered();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE METRIC (ID INTEGER, G GEOMETRY(0))").unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0..200_i32).map(|id| format!("({id}, ST_POINT({}, 0, 0))", id * 100)).collect();
    a.run(&format!("INSERT INTO METRIC VALUES {}", values.join(","))).unwrap();
    a.commit().unwrap();
    db.create_index("METRIC_G", "A.METRIC", &["G"], "rtree").unwrap();
    let plan =
        a.physical_plan("SELECT ID FROM METRIC WHERE ST_DISTANCE(G, ST_POINT(0, 0, 0), 'KILOMETRE') <= 1.5", &Params::new()).unwrap();
    assert!(plan.contains("[rtree]: (ST_DISTANCE(METRIC.G, ") && plan.contains("* 1E-3) <= 1.5"), "{plan}");
    let queries: Vec<String> = [
        "ST_DISTANCE(G, ST_POINT(0, 0, 0), 'KILOMETRE') <= 1.5",
        "ST_DISTANCE(G, ST_POINT(0, 0, 0), 'KILOMETRE') < 1.5",
        "ST_DISTANCE(ST_POINT(10000, 50, 0), G, 'METRE') <= 150",
    ]
    .iter()
    .map(|c| format!("SELECT ID FROM METRIC WHERE {c} ORDER BY ID"))
    .collect();
    let results = indexed_then_scanned(&db, &a, ("METRIC_G", &queries));
    assert_eq!(results[0], (0..=15).map(|i: i32| i.to_string()).collect::<Vec<_>>());
    assert_eq!(results[1], (0..15).map(|i: i32| i.to_string()).collect::<Vec<_>>());
    assert_eq!(results[2], ["99", "100", "101"]);
}

/// Verifies: [ISO/IEC 9075:1992 §7.5] D135: inner, LEFT and FULL joins on
/// a distance limit, a constant or a column of the probing operand, use
/// the R-tree. A null limit, a null or empty geometry and a negative limit
/// match nothing and keep their rows with nulls. Dropping the index gives
/// the same rows.
#[test]
fn distance_joins_use_windows_and_keep_unmatched_rows() {
    let db = populated();
    let a = db.session("A");
    a.run("CREATE TABLE PROBES (ID INTEGER, G GEOMETRY(4326), R DOUBLE PRECISION)").unwrap();
    a.run("INSERT INTO PROBES VALUES (1, ST_POINT(10, 10, 4326), 1.5), (2, ST_POINT(50, 50, 4326), NULL), (3, NULL, 2), (4, ST_GEOMFROMTEXT('POINT EMPTY', 4326), 3), (5, ST_POINT(100.5, 100.5, 4326), -1), (6, ST_POINT(700.5, 700.5, 4326), 0.8)").unwrap();
    let queries = [
        "SELECT Q.ID, P.ID FROM PROBES Q JOIN SHAPES P ON ST_DISTANCE(P.G, Q.G) <= 1 ORDER BY Q.ID, P.ID",
        "SELECT Q.ID, P.ID FROM PROBES Q LEFT JOIN SHAPES P ON ST_DISTANCE(P.G, Q.G) <= Q.R ORDER BY Q.ID, P.ID",
        "SELECT Q.ID, P.ID FROM PROBES Q FULL JOIN (SELECT * FROM SHAPES WHERE ID BETWEEN 698 AND 702) AS P ON Q.R > ST_DISTANCE(Q.G, P.G) ORDER BY Q.ID, P.ID",
    ]
    .map(str::to_owned);
    let plans: Vec<String> = queries.iter().map(|q| a.physical_plan(q, &Params::new()).unwrap()).collect();
    assert!(plans[1].contains("LeftOuterIndexNestedLoop") && plans[2].contains("FullOuterIndexNestedLoop"), "{plans:?}");
    let results = indexed_then_scanned(&db, &a, ("SHAPES_G", &queries));
    assert_eq!(results[0], ["1,10", "1,2002", "2,50", "5,100", "5,101", "6,700", "6,701"]);
    assert_eq!(results[1], ["1,9", "1,10", "1,11", "1,2002", "2,NULL", "3,NULL", "4,NULL", "5,NULL", "6,700", "6,701"]);
    assert_eq!(results[2], ["1,NULL", "2,NULL", "3,NULL", "4,NULL", "5,NULL", "6,700", "6,701", "NULL,698", "NULL,699", "NULL,702"]);
}
