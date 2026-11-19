//! Geometry joins of two R-tree indexed tables descend both trees
//! together, and return what the unindexed product returns (D138).

use super::populated;
use crate::common::{rows, serializable};
use turborel::db::Database;
use turborel::error::Code;
use turborel::params::Params;

const JOIN: &str = "SELECT P.ID, Z.ID FROM ZONES Z JOIN SHAPES P ON ST_INTERSECTS(P.G, Z.G) = 1";

/// SHAPES with zones beside it: small squares, a square with a hole, an
/// empty geometry and a null, both tables indexed.
fn zoned() -> Database {
    let db = populated();
    let a = db.session("A");
    a.run("CREATE TABLE ZONES (ID INTEGER, G GEOMETRY(4326))").unwrap();
    for start in (0..150_i32).step_by(50) {
        let values: Vec<_> =
            (start..start + 50).map(|id| format!("({id}, ST_MAKEENVELOPE({0}, {0}, {1}, {1}, 4326))", id * 3, id * 3 + 1)).collect();
        a.run(&format!("INSERT INTO ZONES VALUES {}", values.join(","))).unwrap();
    }
    a.run("INSERT INTO ZONES VALUES (900, ST_GEOMFROMTEXT('POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 2 8, 8 8, 8 2, 2 2))', 4326)), (901, ST_GEOMFROMTEXT('POINT EMPTY', 4326)), (902, NULL)").unwrap();
    a.commit().unwrap();
    db.create_index("ZONES_G", "A.ZONES", &["G"], "rtree").unwrap();
    db
}

fn sorted(db: &Database, query: &str) -> Vec<String> {
    let a = db.session("A");
    let mut found = rows(&a, query).unwrap();
    a.commit().unwrap();
    found.sort();
    found
}

/// Verifies: [DESIGN] D138, [ISO/IEC 9075:1992 §7.5 GR1 to GR5]: inner,
/// LEFT, RIGHT and FULL joins of two indexed tables traverse both trees
/// and equal the unindexed product, with holes, boundaries, NULL and
/// empty geometries, outer filters, inner projections and residuals.
#[test]
fn synchronized_spatial_joins_equal_unindexed_products() {
    let db = zoned();
    let queries = [
        JOIN.to_owned(),
        "SELECT P.ID, Z.ID FROM SHAPES P JOIN ZONES Z ON 1 = ST_INTERSECTS(Z.G, P.G) AND P.ID <> Z.ID".into(),
        "SELECT P.ID, Z.ID FROM (SELECT * FROM ZONES WHERE ID < 100) Z JOIN SHAPES P ON ST_INTERSECTS(P.G, Z.G) = 1".into(),
        "SELECT P.ID, Z.ID FROM ZONES Z JOIN SHAPES P ON ST_TOUCHES(P.G, Z.G) = 1".into(),
        "SELECT P.ID, Z.ID FROM ZONES Z LEFT JOIN SHAPES P ON ST_INTERSECTS(P.G, Z.G) = 1".into(),
        "SELECT P.ID, Z.ID FROM SHAPES P RIGHT JOIN ZONES Z ON ST_INTERSECTS(P.G, Z.G) = 1".into(),
        "SELECT P.ID, Z.ID FROM ZONES Z FULL JOIN SHAPES P ON ST_INTERSECTS(P.G, Z.G) = 1".into(),
    ];
    let a = db.session("A");
    for query in &queries {
        let plan = a.physical_plan(query, &Params::new()).unwrap();
        assert!(plan.contains("SynchronizedSpatialJoin"), "{query}\n{plan}");
    }
    a.commit().unwrap();
    let traversed: Vec<_> = queries.iter().map(|q| sorted(&db, q)).collect();
    assert!(traversed[0].contains(&"3,1".to_owned()) && traversed[0].contains(&"1,900".to_owned()));
    assert!(!traversed[0].contains(&"5,900".to_owned()), "a point in the hole");
    assert!(traversed[4].contains(&"NULL,902".to_owned()) && traversed[4].contains(&"NULL,901".to_owned()));
    assert_eq!(traversed[4], traversed[5]);
    assert!(traversed[6].contains(&"2001,NULL".to_owned()));
    db.drop_index("SHAPES_G").unwrap();
    db.drop_index("ZONES_G").unwrap();
    for (query, expected) in queries.iter().zip(traversed) {
        assert_eq!(sorted(&db, query), expected, "{query}");
    }
}

/// Verifies: [DESIGN] D138: an ordered join, one of few outer rows, and
/// a distance window keep their probes; EXPLAIN names both trees.
#[test]
fn synchronized_spatial_joins_are_chosen_by_cost_and_order() {
    let db = zoned();
    let a = db.session("A");
    a.run("CREATE TABLE FEW (ID INTEGER, G GEOMETRY(4326))").unwrap();
    a.run("INSERT INTO FEW VALUES (1, ST_MAKEENVELOPE(0, 0, 9, 9, 4326))").unwrap();
    a.commit().unwrap();
    db.create_index("FEW_G", "A.FEW", &["G"], "rtree").unwrap();
    let plan = a.physical_plan(JOIN, &Params::new()).unwrap();
    assert!(plan.contains("SynchronizedSpatialJoin A.SHAPES using SHAPES_G with A.ZONES using ZONES_G [rtree]"), "{plan}");
    for query in [
        format!("{JOIN} ORDER BY Z.ID"),
        "SELECT P.ID, Z.ID FROM FEW Z JOIN SHAPES P ON ST_INTERSECTS(P.G, Z.G) = 1".into(),
        "SELECT P.ID, Z.ID FROM ZONES Z JOIN SHAPES P ON ST_DISTANCE(P.G, Z.G) < 1".into(),
    ] {
        let plan = a.physical_plan(&query, &Params::new()).unwrap();
        assert!(!plan.contains("SynchronizedSpatialJoin"), "{query}\n{plan}");
    }
}

/// Verifies: [DESIGN] D15, D138: an error of the select list is raised
/// over the traversal's pairs as over the product's, and a join of an
/// empty table, or of tables whose envelopes are apart, gives no rows.
#[test]
fn synchronized_spatial_joins_raise_and_end_as_products_do() {
    let db = zoned();
    let a = db.session("A");
    let failing = "SELECT 1 / (P.ID - P.ID) FROM ZONES Z JOIN SHAPES P ON ST_INTERSECTS(P.G, Z.G) = 1";
    assert!(a.physical_plan(failing, &Params::new()).unwrap().contains("SynchronizedSpatialJoin"));
    assert_eq!(rows(&a, failing).unwrap_err(), Code::Data);
    a.rollback().unwrap();
    a.run("UPDATE ZONES SET G = ST_MAKEENVELOPE(100000, 0, 100001, 1, 4326) WHERE G IS NOT NULL").unwrap();
    assert_eq!(rows(&a, JOIN).unwrap(), Vec::<String>::new());
    a.run("DELETE FROM ZONES").unwrap();
    assert_eq!(rows(&a, JOIN).unwrap(), Vec::<String>::new());
    a.rollback().unwrap();
    db.drop_index("SHAPES_G").unwrap();
    assert_eq!(rows(&a, failing).unwrap_err(), Code::Data);
}

/// Verifies: [DESIGN] D29, D134, D138: a traversal records the whole
/// outer table and the probed rows meeting the outer tree's hull, so two
/// serializable readers that each write what the other read cannot both
/// commit, whether the first writes a zone anywhere or a shape inside it.
#[test]
fn synchronized_spatial_joins_record_serializable_reads() {
    for insert in [
        "INSERT INTO ZONES VALUES (950, ST_MAKEENVELOPE(5000, 5000, 5001, 5001, 4326))",
        "INSERT INTO SHAPES VALUES (5000, ST_POINT(1, 1, 4326))",
    ] {
        let db = zoned();
        serializable(&db);
        let (a, b) = (db.session("A"), db.session("A"));
        assert!(a.physical_plan(JOIN, &Params::new()).unwrap().contains("SynchronizedSpatialJoin"));
        let seen = rows(&a, JOIN).unwrap().len();
        assert_eq!(rows(&b, JOIN).unwrap().len(), seen);
        a.run(insert).unwrap();
        b.run("INSERT INTO SHAPES VALUES (5001, ST_POINT(4, 4, 4326))").unwrap();
        let commits = [a.commit(), b.commit()];
        assert!(commits.iter().any(Result::is_err), "{insert}");
        for error in commits.into_iter().filter_map(Result::err) {
            assert_eq!(error.code, Code::Conflict);
        }
    }
}
