//! Geometry joins retain exact shapes, references and visible versions
//! when an R-tree replaces their Cartesian product (D15, D82).

use super::populated;
use crate::common::{rows, serializable};
use turborel::db::Database;
use turborel::error::Code;
use turborel::params::Params;

fn windows(db: &Database, reference: &str) {
    let a = db.session("A");
    a.run(&format!("CREATE TABLE WINDOWS (ID INTEGER, G {reference})")).unwrap();
    a.run("INSERT INTO WINDOWS VALUES (10, ST_GEOMFROMTEXT('POLYGON ((0 0, 3 0, 3 3, 0 3, 0 0))', 4326)), (11, ST_POINT(5, 5, 4326)), (12, ST_GEOMFROMTEXT('POINT EMPTY', 4326)), (13, NULL)").unwrap();
    a.commit().unwrap();
}

/// Verifies: [DESIGN] D82: both join orders and function/comparison
/// orientations use geometry candidates, then reject holes and empties.
/// Projected columns, inner filters and other ON comparisons are retained.
/// Dropping the index gives exactly the same sorted rows for every query.
#[test]
fn geometry_joins_refine_candidates_and_equal_products() {
    let db = populated();
    windows(&db, "GEOMETRY(4326)");
    let a = db.session("A");
    let queries = [
        "SELECT P.ID, W.ID FROM SHAPES P JOIN WINDOWS W ON ST_INTERSECTS(P.G, W.G) = 1 ORDER BY P.ID, W.ID",
        "SELECT P.ID, W.ID FROM WINDOWS W JOIN SHAPES P ON 1 = ST_INTERSECTS(W.G, P.G) ORDER BY P.ID, W.ID",
        "SELECT P.ID, W.ID FROM SHAPES P, WINDOWS W WHERE ST_INTERSECTS(W.G, P.G) = 1 ORDER BY P.ID, W.ID",
        "SELECT P.ID, W.ID FROM WINDOWS W, (SELECT G AS H, ID FROM SHAPES WHERE ID < 100) AS P WHERE ST_INTERSECTS(P.H, W.G) = 1 AND P.ID < W.ID ORDER BY P.ID, W.ID",
    ];
    let mut results = Vec::new();
    for query in queries {
        let plan = a.physical_plan(query, &Params::new()).unwrap();
        assert!(plan.contains("IndexNestedLoop") && plan.contains("SHAPES_G"), "{query}\n{plan}");
        results.push(rows(&a, query).unwrap());
    }
    assert_eq!(results[0], ["0,10", "1,10", "2,10", "3,10", "5,11", "2002,10"]);
    assert_eq!(results[0], results[1]);
    assert_eq!(results[0], results[2]);
    assert_eq!(results[3], ["0,10", "1,10", "2,10", "3,10", "5,11"]);
    a.commit().unwrap();
    db.drop_index("SHAPES_G").unwrap();
    for (query, expected) in queries.into_iter().zip(results) {
        assert_eq!(rows(&a, query).unwrap(), expected, "{query}");
    }
}

/// Verifies: [DESIGN] D15, D82: fixed but differing SRIDs, including an
/// empty geometry, raise even when envelopes do not meet. An unconstrained
/// reference and another raising conjunct prevent an indexed join; empty
/// inputs still do not evaluate a relationship on a nonexistent pair.
#[test]
fn geometry_joins_preserve_reference_and_other_runtime_errors() {
    for reference in ["GEOMETRY", "GEOMETRY(0)"] {
        let db = populated();
        let a = db.session("A");
        a.run(&format!("CREATE TABLE WINDOWS (ID INTEGER, G {reference})")).unwrap();
        let query = "SELECT P.ID FROM WINDOWS W JOIN SHAPES P ON ST_INTERSECTS(P.G, W.G) = 1";
        let plan = a.physical_plan(query, &Params::new()).unwrap();
        assert!(!plan.contains("IndexNestedLoop"), "{plan}");
        assert_eq!(rows(&a, query).unwrap(), Vec::<String>::new());
        for text in ["POINT (10000 10000)", "POINT EMPTY"] {
            a.run(&format!("INSERT INTO WINDOWS VALUES (1, ST_GEOMFROMTEXT('{text}'))")).unwrap();
            assert_eq!(rows(&a, query).unwrap_err(), Code::Data);
            a.run("DELETE FROM WINDOWS").unwrap();
        }
    }
    let db = populated();
    windows(&db, "GEOMETRY(4326)");
    let a = db.session("A");
    for query in [
        "SELECT P.ID FROM WINDOWS W JOIN SHAPES P ON ST_INTERSECTS(P.G, W.G) = 1 AND 1 / (P.ID - 100) > 0",
        "SELECT P.ID FROM WINDOWS W JOIN (SELECT ID, G FROM SHAPES WHERE 1 / (ID - 100) > 0) AS P ON ST_INTERSECTS(P.G, W.G) = 1",
    ] {
        let plan = a.physical_plan(query, &Params::new()).unwrap();
        assert!(!plan.contains("IndexNestedLoop"), "{plan}");
        assert_eq!(rows(&a, query).unwrap_err(), Code::Data);
    }
}

/// Verifies: [DESIGN] D82: a paused geometry join retains snapshot shapes
/// across equal-envelope, moved-envelope and NULL updates. Rollback restores
/// the original pairs without duplicate entries from projected versions.
#[test]
fn geometry_joins_keep_visible_shapes_across_updates_and_rollback() {
    let db = populated();
    windows(&db, "GEOMETRY(4326)");
    let (reader, writer) = (db.session("A"), db.session("A"));
    reader.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    let query = "SELECT P.ID, W.ID FROM WINDOWS W JOIN SHAPES P ON ST_INTERSECTS(P.G, W.G) = 1";
    let mut expected = rows(&reader, query).unwrap();
    expected.sort();
    assert!(reader.physical_plan(query, &Params::new()).unwrap().contains("SHAPES_G"));
    let mut cursor = reader.open_cursor(query).unwrap();
    let mut actual = vec![cursor.fetch().unwrap().unwrap().iter().map(ToString::to_string).collect::<Vec<_>>().join(",")];
    writer.run("UPDATE SHAPES SET G = ST_GEOMFROMTEXT('POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0))', 4326) WHERE ID = 2002").unwrap();
    writer.run("UPDATE SHAPES SET G = ST_POINT(10000, 10000, 4326) WHERE ID = 5").unwrap();
    writer.run("UPDATE SHAPES SET G = NULL WHERE ID = 0").unwrap();
    writer.commit().unwrap();
    while let Some(row) = cursor.fetch().unwrap() {
        actual.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    actual.sort();
    assert_eq!(actual, expected);
    drop(cursor);
    reader.commit().unwrap();
    let ordered = format!("{query} ORDER BY P.ID, W.ID");
    let committed = rows(&writer, &ordered).unwrap();
    assert_eq!(committed, ["1,10", "2,10", "3,10", "2002,10", "2002,11"]);
    writer.run("UPDATE SHAPES SET G = ST_POINT(20000, 20000, 4326)").unwrap();
    assert_eq!(rows(&writer, query).unwrap(), Vec::<String>::new());
    writer.rollback().unwrap();
    assert_eq!(rows(&writer, &ordered).unwrap(), committed);
}

/// Verifies: [DESIGN] D29, D82: two serializable readers of an empty
/// geometry join cannot both commit inserts that would match it. The
/// predicate read is recorded even when the R-tree finds no candidate.
#[test]
fn geometry_joins_track_empty_probe_phantoms() {
    let db = populated();
    windows(&db, "GEOMETRY(4326)");
    let setup = db.session("A");
    setup.run("UPDATE WINDOWS SET G = ST_POINT(10000, 10000, 4326)").unwrap();
    setup.commit().unwrap();
    serializable(&db);
    let (a, b) = (db.session("A"), db.session("A"));
    let query = "SELECT P.ID FROM WINDOWS W JOIN SHAPES P ON ST_INTERSECTS(P.G, W.G) = 1";
    assert!(a.physical_plan(query, &Params::new()).unwrap().contains("SHAPES_G"));
    assert_eq!(rows(&a, query).unwrap(), Vec::<String>::new());
    assert_eq!(rows(&b, query).unwrap(), Vec::<String>::new());
    a.run("INSERT INTO SHAPES VALUES (4000, ST_POINT(10000, 10000, 4326))").unwrap();
    b.run("INSERT INTO SHAPES VALUES (4001, ST_POINT(10000, 10000, 4326))").unwrap();
    let commits = [a.commit(), b.commit()];
    assert!(commits.iter().any(Result::is_err));
    for error in commits.into_iter().filter_map(Result::err) {
        assert_eq!(error.code, Code::Conflict);
    }
}
