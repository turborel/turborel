//! SQL nearest geometry ordering agrees with exhaustive distance sorting.

use super::populated;
use crate::common::rows;
use turborel::error::Code;
use turborel::params::Params;

mod concurrent;
mod storage;

/// Verifies: [DESIGN] D15, D85: nearest candidates refine polygon holes,
/// retain distance ties for secondary ordering, and apply filters before
/// ranking. Both operand orientations and OFFSET agree with full sorting.
#[test]
fn nearest_geometry_candidates_equal_exhaustive_sql() {
    let db = populated();
    let a = db.session("A");
    a.run("INSERT INTO SHAPES VALUES (2003, ST_POINT(5, 6, 4326)), (2004, ST_POINT(6, 5, 4326))").unwrap();
    a.commit().unwrap();
    let queries = [
        "SELECT ID, ST_DISTANCE(G, ST_POINT(5, 5, 4326)) AS D FROM SHAPES ORDER BY D, ID FETCH FIRST 8 ROWS ONLY",
        "SELECT ID, ST_DISTANCE(ST_POINT(5, 5, 4326), G) AS D FROM SHAPES ORDER BY D, ID DESC OFFSET 2 ROWS FETCH FIRST 4 ROWS ONLY",
        "SELECT ID, ST_DISTANCE(G, ST_POINT(5, 5, 4326)) AS D FROM SHAPES WHERE ID > 4 ORDER BY D, ID FETCH FIRST 4 ROWS ONLY",
        "SELECT ID, ST_DISTANCE(G, ST_POINT(5, 5, 4326)) AS D FROM SHAPES WHERE ST_ISEMPTY(G) = 0 ORDER BY D, ID FETCH FIRST 4 ROWS ONLY",
    ];
    let mut results = Vec::new();
    for query in queries {
        let plan = a.physical_plan(query, &Params::new()).unwrap();
        assert!(plan.contains("NearestScan") && plan.contains("SHAPES_G"), "{query}\n{plan}");
        results.push(rows(&a, query).unwrap());
    }
    assert_eq!(&results[0][..3], ["5,0E0", "2003,1E0", "2004,1E0"]);
    assert!(results[0].iter().any(|r| r == "2002,3E0"));
    a.commit().unwrap();
    db.drop_index("SHAPES_G").unwrap();
    for (query, expected) in queries.into_iter().zip(results) {
        assert_eq!(rows(&a, query).unwrap(), expected, "{query}");
    }
}

/// Verifies: [DESIGN] D85: exhausting candidates with fewer than FETCH
/// non-NULL distances adds each qualifying EMPTY and NULL row once.
/// A NULL or EMPTY query falls back to SQL's complete nullable ordering.
#[test]
fn nearest_geometry_null_tails_and_query_fallbacks_equal_scans() {
    let db = populated();
    let a = db.session("A");
    let queries = [
        "SELECT ID, ST_DISTANCE(G, ST_POINT(5, 5, 4326)) AS D FROM SHAPES WHERE CASE WHEN ID >= 2000 THEN 1 ELSE 0 END > 0 ORDER BY D, ID FETCH FIRST 8 ROWS ONLY",
        "SELECT ID, ST_DISTANCE(G, ST_POINT(5, 5, 4326)) AS D FROM SHAPES WHERE CASE WHEN ID >= 2000 THEN 1 ELSE 0 END > 0 AND ST_ISEMPTY(G) = 0 ORDER BY D, ID FETCH FIRST 8 ROWS ONLY",
        "SELECT ID, ST_DISTANCE(G, ST_POINT(5, 5, 4326)) AS D FROM SHAPES WHERE CASE WHEN ID < 0 THEN 1 ELSE 0 END > 0 ORDER BY D, ID FETCH FIRST 8 ROWS ONLY",
        "SELECT ID, ST_DISTANCE(G, CAST(NULL AS GEOMETRY)) AS D FROM SHAPES ORDER BY D, ID FETCH FIRST 3 ROWS ONLY",
        "SELECT ID, ST_DISTANCE(G, ST_GEOMFROMTEXT('POINT EMPTY', 4326)) AS D FROM SHAPES ORDER BY D, ID FETCH FIRST 3 ROWS ONLY",
    ];
    let mut results = Vec::new();
    for query in queries {
        let plan = a.physical_plan(query, &Params::new()).unwrap();
        assert!(plan.contains("NearestScan"), "{query}\n{plan}");
        results.push(rows(&a, query).unwrap());
    }
    assert_eq!(results[0], ["2002,3E0", "2000,NULL", "2001,NULL"]);
    assert_eq!(results[1], ["2002,3E0"]);
    assert_eq!(results[2], Vec::<String>::new());
    assert_eq!(results[3], ["0,NULL", "1,NULL", "2,NULL"]);
    assert_eq!(results[3], results[4]);
    a.commit().unwrap();
    db.drop_index("SHAPES_G").unwrap();
    for (query, expected) in queries.into_iter().zip(results) {
        assert_eq!(rows(&a, query).unwrap(), expected, "{query}");
    }
}

/// Verifies: [DESIGN] D15, D85: pruning cannot hide an SRID mismatch,
/// distance overflow, or errors in other projections or WHERE terms.
/// Descending, NULLS FIRST, WITH TIES and PERCENT retain ordinary plans.
#[test]
fn nearest_geometry_preserves_runtime_errors_and_unsupported_orders() {
    let db = populated();
    let a = db.session("A");
    for query in [
        "SELECT ID, ST_DISTANCE(G, ST_POINT(5, 5)) AS D FROM SHAPES ORDER BY D FETCH FIRST 1 ROW ONLY",
        "SELECT ID, ST_DISTANCE(G, ST_GEOMFROMTEXT('POINT EMPTY')) AS D FROM SHAPES ORDER BY D FETCH FIRST 1 ROW ONLY",
    ] {
        assert!(a.physical_plan(query, &Params::new()).unwrap().contains("NearestScan"));
        assert_eq!(rows(&a, query).unwrap_err(), Code::Data);
    }
    for query in [
        "SELECT 1 / (ID - 100), ST_DISTANCE(G, ST_POINT(5, 5, 4326)) AS D FROM SHAPES ORDER BY D FETCH FIRST 1 ROW ONLY",
        "SELECT ID, ST_DISTANCE(G, ST_POINT(5, 5, 4326)) AS D FROM SHAPES WHERE 1 / (ID - 100) > 0 ORDER BY D FETCH FIRST 1 ROW ONLY",
    ] {
        assert!(!a.physical_plan(query, &Params::new()).unwrap().contains("NearestScan"));
        assert_eq!(rows(&a, query).unwrap_err(), Code::Data);
    }
    for order in [
        "D DESC FETCH FIRST 1 ROW ONLY",
        "D NULLS FIRST FETCH FIRST 1 ROW ONLY",
        "D FETCH FIRST 1 ROW WITH TIES",
        "D FETCH FIRST 1 PERCENT ROWS ONLY",
    ] {
        let query = format!("SELECT ID, ST_DISTANCE(G, ST_POINT(5, 5, 4326)) AS D FROM SHAPES ORDER BY {order}");
        assert!(!a.physical_plan(&query, &Params::new()).unwrap().contains("NearestScan"));
        rows(&a, &query).unwrap();
    }
    a.run("INSERT INTO SHAPES VALUES (2003, ST_POINT(1.7E308, 1.7E308, 4326))").unwrap();
    let query = "SELECT ID, ST_DISTANCE(G, ST_POINT(0, 0, 4326)) AS D FROM SHAPES ORDER BY D FETCH FIRST 1 ROW ONLY";
    assert!(a.physical_plan(query, &Params::new()).unwrap().contains("NearestScan"));
    assert_eq!(rows(&a, query).unwrap_err(), Code::Data);
}
