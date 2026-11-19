//! Geometry outer joins through an R-tree keep every unmatched row of a
//! preserved operand, with nulls, and raise what the product would
//! [ISO/IEC 9075:1992 §7.5] (D15, D85, D135).

use super::populated;
use crate::common::rows;
use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;

fn windows(db: &Database, reference: &str) {
    let a = db.session("A");
    a.run(&format!("CREATE TABLE WINDOWS (ID INTEGER, G {reference})")).unwrap();
    a.run("INSERT INTO WINDOWS VALUES (10, ST_GEOMFROMTEXT('POLYGON ((0 0, 3 0, 3 3, 0 3, 0 0))', 4326)), (11, ST_POINT(5, 5, 4326)), (12, ST_GEOMFROMTEXT('POINT EMPTY', 4326)), (13, NULL)").unwrap();
    a.commit().unwrap();
}

/// Each query's rows, its plan containing `shown`, then the same rows
/// once the index is dropped.
fn indexed_then_scanned(db: &Database, a: &Session, queries: &[(&str, &str)]) -> Vec<Vec<String>> {
    let mut results = Vec::new();
    for (query, shown) in queries {
        let plan = a.physical_plan(query, &Params::new()).unwrap();
        assert!(plan.contains(shown) && plan.contains("SHAPES_G"), "{query}\n{plan}");
        results.push(rows(a, query).unwrap());
    }
    a.commit().unwrap();
    db.drop_index("SHAPES_G").unwrap();
    for ((query, _), expected) in queries.iter().zip(&results) {
        let plan = a.physical_plan(query, &Params::new()).unwrap();
        assert!(!plan.contains("SHAPES_G"), "{plan}");
        assert_eq!(&rows(a, query).unwrap(), expected, "{query}");
    }
    results
}

/// Verifies: [ISO/IEC 9075:1992 §7.5 GR1c, GR2 to GR5] D135: LEFT, RIGHT
/// and FULL joins on ST_Intersects probe the R-tree of the operand they
/// read. A null or empty geometry matches nothing and keeps its row with
/// nulls; a FULL join then keeps every table row no probe paired, of a
/// derived table only those its filter keeps. Other ON comparisons are
/// judged of each candidate. Dropping the index gives the same rows.
#[test]
fn geometry_outer_joins_keep_unmatched_rows_and_equal_products() {
    let db = populated();
    windows(&db, "GEOMETRY(4326)");
    let a = db.session("A");
    let on = "ON ST_INTERSECTS(P.G, W.G) = 1";
    let left = format!("SELECT W.ID, P.ID FROM WINDOWS W LEFT JOIN SHAPES P {on} ORDER BY W.ID, P.ID");
    let right = format!("SELECT W.ID, P.ID FROM SHAPES P RIGHT JOIN WINDOWS W {on} ORDER BY W.ID, P.ID");
    let full = format!("SELECT W.ID, P.ID FROM WINDOWS W FULL JOIN SHAPES P {on} ORDER BY W.ID, P.ID");
    let derived =
        format!("SELECT W.ID, P.ID FROM WINDOWS W FULL JOIN (SELECT ID, G FROM SHAPES WHERE ID < 8) AS P {on} ORDER BY W.ID, P.ID");
    let other = format!("SELECT W.ID, P.ID FROM WINDOWS W LEFT JOIN SHAPES P {on} AND P.ID > 1 ORDER BY W.ID, P.ID");
    let queries = [
        (left.as_str(), "LeftOuterIndexNestedLoop"),
        (right.as_str(), "LeftOuterIndexNestedLoop"),
        (full.as_str(), "FullOuterIndexNestedLoop"),
        (derived.as_str(), "FullOuterIndexNestedLoop"),
        (other.as_str(), "LeftOuterIndexNestedLoop"),
    ];
    let results = indexed_then_scanned(&db, &a, &queries);
    let kept = ["10,0", "10,1", "10,2", "10,3", "10,2002", "11,5", "12,NULL", "13,NULL"];
    assert_eq!(results[0], kept);
    assert_eq!(results[1], kept);
    assert_eq!(results[2].len(), kept.len() + 2003 - 6);
    for row in kept.iter().chain(&["NULL,4", "NULL,2000", "NULL,2001"]) {
        assert!(results[2].iter().any(|r| r == row), "{row}");
    }
    assert_eq!(results[3], ["10,0", "10,1", "10,2", "10,3", "11,5", "12,NULL", "13,NULL", "NULL,4", "NULL,6", "NULL,7"]);
    assert_eq!(results[4], ["10,2", "10,3", "10,2002", "11,5", "12,NULL", "13,NULL"]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.43 to §5.1.45,
/// §5.1.50 to §5.1.54] D135: outer joins on each relationship true only of
/// meeting operands, ST_Equals, and ST_Relate of a pattern asking for an
/// interior or boundary to meet use the R-tree and equal the product.
/// ST_Equals of two empty sets is true, so its probe of an empty value
/// reads every entry. A relate pattern disjoint geometries may match is
/// not probed.
#[test]
fn geometry_outer_joins_of_every_windowed_relationship() {
    let db = populated();
    windows(&db, "GEOMETRY(4326)");
    let a = db.session("A");
    let conditions = [
        "ST_WITHIN(P.G, W.G) = 1",
        "ST_CONTAINS(W.G, P.G) = 1",
        "ST_TOUCHES(P.G, W.G) = 1",
        "ST_CROSSES(P.G, W.G) = 1",
        "ST_OVERLAPS(W.G, P.G) = 1",
        "ST_EQUALS(P.G, W.G) = 1",
        "ST_RELATE(P.G, W.G, 'T*F**F***') = 1",
        "1 = ST_RELATE(W.G, P.G, '****T****')",
    ];
    let queries: Vec<String> =
        conditions.iter().map(|c| format!("SELECT W.ID, P.ID FROM WINDOWS W LEFT JOIN SHAPES P ON {c} ORDER BY W.ID, P.ID")).collect();
    let shown: Vec<(&str, &str)> = queries.iter().map(|q| (q.as_str(), "LeftOuterIndexNestedLoop")).collect();
    let disjoint = "SELECT COUNT(*) FROM WINDOWS W LEFT JOIN SHAPES P ON ST_RELATE(P.G, W.G, 'FF*FF****') = 1";
    assert!(!a.physical_plan(disjoint, &Params::new()).unwrap().contains("SHAPES_G"));
    let results = indexed_then_scanned(&db, &a, &shown);
    assert_eq!(results[0], ["10,1", "10,2", "11,5", "12,NULL", "13,NULL"]);
    assert_eq!(results[0], results[1]);
    assert_eq!(results[5], ["10,NULL", "11,5", "12,2000", "13,NULL"]);
    assert_eq!(results[6], results[0]);
}

/// Verifies: [DESIGN] D15, D135: an outer join whose outer geometry has
/// another SRID, or none fixed, still probes the R-tree, and the probe of
/// a value of another SRID reads every entry, so the SRID error is raised
/// even for a point or an empty geometry no envelope is near. An empty
/// outer operand raises nothing and a FULL join keeps every table row.
/// Another conjunct that can raise keeps the join off the index.
#[test]
fn geometry_outer_joins_preserve_runtime_errors() {
    for reference in ["GEOMETRY", "GEOMETRY(0)"] {
        let db = populated();
        let a = db.session("A");
        a.run(&format!("CREATE TABLE WINDOWS (ID INTEGER, G {reference})")).unwrap();
        for kind in ["LEFT", "FULL"] {
            let query = format!("SELECT P.ID FROM WINDOWS W {kind} JOIN SHAPES P ON ST_INTERSECTS(P.G, W.G) = 1");
            assert_eq!(rows(&a, &query).unwrap().len(), if kind == "LEFT" { 0 } else { 2003 });
            for text in ["POINT (10000 10000)", "POINT EMPTY"] {
                a.run(&format!("INSERT INTO WINDOWS VALUES (1, ST_GEOMFROMTEXT('{text}'))")).unwrap();
                let plan = a.physical_plan(&query, &Params::new()).unwrap();
                assert!(plan.contains("OuterIndexNestedLoop"), "{plan}");
                assert_eq!(rows(&a, &query).unwrap_err(), Code::Data, "{query} {text}");
                a.run("DELETE FROM WINDOWS").unwrap();
            }
        }
    }
    let db = populated();
    windows(&db, "GEOMETRY(4326)");
    let a = db.session("A");
    let query = "SELECT P.ID FROM WINDOWS W LEFT JOIN SHAPES P ON ST_INTERSECTS(P.G, W.G) = 1 AND 1 / (P.ID - 100) > 0";
    assert!(!a.physical_plan(query, &Params::new()).unwrap().contains("IndexNestedLoop"));
    assert_eq!(rows(&a, query).unwrap_err(), Code::Data);
}

/// Verifies: [DESIGN] D135: EXPLAIN names the outer index joins: a LEFT
/// join probing the right operand's table, a RIGHT one as the LEFT join of
/// its operands swapped under a projection, and a FULL one, with the whole
/// condition each candidate pair is judged by.
#[test]
fn geometry_outer_joins_explain_their_probes() {
    let db = populated();
    windows(&db, "GEOMETRY(4326)");
    let a = db.session("A");
    let plan = |sql: &str| a.physical_plan(sql, &Params::new()).unwrap();
    let left = plan("SELECT W.ID, P.ID FROM WINDOWS W LEFT JOIN SHAPES P ON ST_INTERSECTS(P.G, W.G) = 1");
    assert!(left.contains("LeftOuterIndexNestedLoop A.SHAPES using SHAPES_G [rtree]: ST_INTERSECTS(P.G, W.G) = 1"), "{left}");
    let right = plan("SELECT W.ID, P.ID FROM SHAPES P RIGHT JOIN WINDOWS W ON ST_INTERSECTS(P.G, W.G) = 1");
    assert!(right.contains("Project") && right.contains("LeftOuterIndexNestedLoop A.SHAPES using SHAPES_G"), "{right}");
    let full = plan("SELECT W.ID, P.ID FROM WINDOWS W FULL JOIN SHAPES P ON ST_INTERSECTS(P.G, W.G) = 1");
    assert!(full.contains("FullOuterIndexNestedLoop A.SHAPES using SHAPES_G [rtree]"), "{full}");
}

/// Verifies: [DESIGN] D135: regression: a call of null values alone of a
/// name a geometry function shares with routines of §15, ST_EQUALS, is the
/// geometry function's and null, as an index finding its rows assumes.
/// Before, it raised a §15 routine's null-argument error (2FF03), which an
/// outer join probing the R-tree hid, as it pairs no null geometry.
#[test]
fn null_calls_of_shared_names_are_null() {
    let db = populated();
    let a = db.session("A");
    assert_eq!(rows(&a, "VALUES ST_EQUALS(CAST(NULL AS GEOMETRY), CAST(NULL AS GEOMETRY))").unwrap(), ["NULL"]);
    assert_eq!(rows(&a, "SELECT ST_EQUALS(G, G) FROM SHAPES WHERE ID = 2001").unwrap(), ["NULL"]);
}
