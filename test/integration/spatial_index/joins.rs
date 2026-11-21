//! SQL joins must retain the rows, NULL semantics, errors and versions
//! of their original products when independent index bounds replace them.

use super::*;

const MATCH: &str = "P.X >= W.LX AND P.X <= W.HX AND P.Y >= W.LY AND P.Y <= W.HY";

fn windows(db: &Database) {
    let session = db.session("S");
    session.run("CREATE TABLE W (ID INTEGER, LX DOUBLE PRECISION NOT NULL, LY DOUBLE PRECISION NOT NULL, HX DOUBLE PRECISION NOT NULL, HY DOUBLE PRECISION NOT NULL)").unwrap();
    for n in 0_i32..32_i32 {
        let (x, y) = (n % 29_i32, n % 31_i32);
        session.run(&format!("INSERT INTO W VALUES ({n}, {x}, {y}, {}, {})", x + 2_i32, y + 3_i32)).unwrap();
    }
    session.commit().unwrap();
}

/// Verifies: [DESIGN] Independent bounds join points to windows through
/// the index in either table order and through a column projection with
/// a filter. Indexed results equal the complete product's, including
/// touching boundaries, duplicate positions and nullable coordinates.
#[test]
fn spatial_joins_equal_products_in_both_orders_and_through_projections() {
    let db = database();
    windows(&db);
    let session = db.session("S");
    // A range join (RULES.md D580) costs less here; the index's join is
    // what these tests read.
    session.run("SET PLANNER.RANGE_JOINS = 'OFF'").unwrap();
    let queries = [
        format!("SELECT P.ID, W.ID FROM P, W WHERE {MATCH} ORDER BY P.ID, W.ID"),
        format!("SELECT P.ID, W.ID FROM W, P WHERE {MATCH} ORDER BY P.ID, W.ID"),
        "SELECT P.ID, W.ID FROM W, (SELECT ID, Y AS V, X AS U FROM P WHERE ID < 1000) AS P WHERE P.U >= W.LX AND P.U <= W.HX AND P.V >= W.LY AND P.V <= W.HY ORDER BY P.ID, W.ID".into(),
    ];
    for query in &queries {
        let plan = session.physical_plan(query, &Params::new()).unwrap();
        assert!(plan.contains("IndexNestedLoop") && plan.contains("P_XY"), "{plan}");
    }
    let indexed: Vec<_> = queries.iter().map(|q| rows(&session, q).unwrap()).collect();
    assert_ne!(indexed[0], Vec::<String>::new());
    assert_eq!(indexed[0], indexed[1]);
    session.commit().unwrap();
    db.drop_index("P_XY").unwrap();
    for (query, expected) in queries.iter().zip(indexed) {
        assert_eq!(rows(&session, query).unwrap(), expected, "{query}");
    }
}

/// Verifies: [DESIGN] A rectangle index retains spanning candidates
/// when its minimum and maximum comparisons cross. Its indexed join
/// answers the same point-in-window predicate as a sequential product.
#[test]
fn spatial_joins_probe_four_coordinate_rectangles() {
    let db = database();
    windows(&db);
    db.drop_index("P_XY").unwrap();
    let session = db.session("S");
    session.run("SET PLANNER.RANGE_JOINS = 'OFF'").unwrap();
    session.run("INSERT INTO W SELECT ID + 100, LX, LY, HX, HY FROM W").unwrap();
    session.commit().unwrap();
    db.create_index("W_BOUNDS", "S.W", &["LX", "LY", "HX", "HY"], "rtree").unwrap();
    let query = format!("SELECT P.ID, W.ID FROM P, W WHERE {MATCH} AND P.ID < 4 ORDER BY P.ID, W.ID");
    let plan = session.physical_plan(&query, &Params::new()).unwrap();
    assert!(plan.contains("IndexNestedLoop") && plan.contains("W_BOUNDS"), "{plan}");
    let expected = rows(&session, &query).unwrap();
    session.commit().unwrap();
    db.drop_index("W_BOUNDS").unwrap();
    assert_eq!(rows(&session, &query).unwrap(), expected);
}

/// Verifies: [DESIGN] A comparison on X alone cannot read an index that
/// omits NULL Y. A raising conjunct still raises even if spatial bounds
/// exclude every pair; reading candidates cannot suppress SQL errors.
#[test]
fn spatial_joins_preserve_unconstrained_nulls_and_raising_conditions() {
    let db = database();
    windows(&db);
    let session = db.session("S");
    let query = "SELECT P.ID, W.ID FROM P, W WHERE P.X >= W.LX AND P.X <= W.HX ORDER BY P.ID, W.ID";
    let plan = session.physical_plan(query, &Params::new()).unwrap();
    assert!(!plan.contains("IndexNestedLoop"), "{plan}");
    let indexed = rows(&session, query).unwrap();
    let failing = "SELECT P.ID FROM P, W WHERE P.X > W.HX AND P.Y < W.LY AND 1 / (P.ID - P.ID) > 0";
    let code = rows(&session, failing).unwrap_err();
    assert_eq!(code, Code::Data);
    session.rollback().unwrap();
    db.drop_index("P_XY").unwrap();
    assert_eq!(rows(&session, query).unwrap(), indexed);
    assert_eq!(rows(&session, failing).unwrap_err(), code);
}

/// Verifies: [DESIGN] A paused indexed join uses the snapshot's point
/// coordinates after another transaction moves those points. Old index
/// entries remain reachable and newer keys fail the ordinary MVCC recheck.
#[test]
fn paused_spatial_joins_keep_their_snapshot_during_point_updates() {
    let db = database();
    windows(&db);
    let (reader, writer) = (db.session("S"), db.session("S"));
    reader.run("SET PLANNER.RANGE_JOINS = 'OFF'").unwrap();
    reader.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    let query = format!("SELECT P.ID, W.ID FROM W, P WHERE {MATCH}");
    let mut expected = rows(&reader, &query).unwrap();
    expected.sort();
    let plan = reader.physical_plan(&query, &Params::new()).unwrap();
    assert!(plan.contains("IndexNestedLoop"), "{plan}");
    let mut cursor = reader.open_cursor(&query).unwrap();
    let mut actual = vec![cursor.fetch().unwrap().unwrap().iter().map(ToString::to_string).collect::<Vec<_>>().join(",")];
    writer.run("UPDATE P SET X = 999, Y = 999 WHERE X IS NOT NULL AND Y IS NOT NULL").unwrap();
    writer.commit().unwrap();
    while let Some(row) = cursor.fetch().unwrap() {
        actual.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    actual.sort();
    assert_eq!(actual, expected);
}

/// Verifies: [DESIGN] An empty spatial join probe records its predicate
/// for serializable validation. Two transactions cannot both insert new
/// matching points after each observed the window empty.
#[test]
fn spatial_joins_track_empty_probe_phantoms() {
    let db = database();
    windows(&db);
    let setup = db.session("S");
    setup.run("UPDATE W SET LX = 100, LY = 100, HX = 101, HY = 101").unwrap();
    setup.commit().unwrap();
    serializable(&db);
    let (a, b) = (db.session("S"), db.session("S"));
    for s in [&a, &b] {
        s.run("SET PLANNER.RANGE_JOINS = 'OFF'").unwrap();
    }
    let query = format!("SELECT P.ID FROM W, P WHERE {MATCH}");
    let plan = a.physical_plan(&query, &Params::new()).unwrap();
    assert!(plan.contains("IndexNestedLoop"), "{plan}");
    assert_eq!(rows(&a, &query).unwrap(), Vec::<String>::new());
    assert_eq!(rows(&b, &query).unwrap(), Vec::<String>::new());
    a.run("INSERT INTO P VALUES (4000, 100, 100)").unwrap();
    b.run("INSERT INTO P VALUES (4001, 101, 101)").unwrap();
    let first = a.commit();
    let second = b.commit();
    assert!(first.is_err() || second.is_err());
    for error in [first.err(), second.err()].into_iter().flatten() {
        assert_eq!(error.code, Code::Conflict);
    }
}
