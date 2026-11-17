//! Buffered candidate blocks still fetch the version visible to the query.

use crate::common::rows;
use crate::geometry::registered;
use turborel::db::Database;
use turborel::params::Params;

const JOIN: &str = "SELECT P.ID FROM WINDOWS W JOIN POINTS P ON ST_INTERSECTS(P.G, W.G) = 1";

fn populated() -> Database {
    let db = registered();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE POINTS (ID INTEGER PRIMARY KEY, G GEOMETRY(4326)) CREATE TABLE WINDOWS (G GEOMETRY(4326))").unwrap();
    let a = db.session("A");
    for start in (0..5_000_i32).step_by(200) {
        let values = (start..start + 200).map(|n| format!("({n}, ST_POINT({n}, {n}, 4326))")).collect::<Vec<_>>();
        a.run(&format!("INSERT INTO POINTS VALUES {}", values.join(","))).unwrap();
    }
    a.run("INSERT INTO WINDOWS VALUES (ST_MAKEENVELOPE(-1, -1, 10000, 10000, 4326))").unwrap();
    a.commit().unwrap();
    db.create_index("POINTS_G", "A.POINTS", &["G"], "rtree").unwrap();
    db
}

/// Verifies: [DESIGN] D29, D78: a spatial join spanning multiple buffered
/// blocks keeps its snapshot across moved, NULL and equal-envelope shape
/// updates and an insertion. Later reads see the changes, with no duplicate
/// from counted versions; rollback restores the full visible result.
#[test]
fn clustered_geometry_join_preserves_snapshots_across_blocks() {
    let db = populated();
    let (reader, writer) = (db.session("A"), db.session("A"));
    reader.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    let plan = reader.physical_plan(JOIN, &Params::new()).unwrap();
    assert!(plan.contains("IndexNestedLoop") && plan.contains("POINTS_G"), "{plan}");
    let mut cursor = reader.open_cursor(JOIN).unwrap();
    let mut actual = vec![cursor.fetch().unwrap().unwrap()[0].to_string().parse::<i32>().unwrap()];
    writer.run("UPDATE POINTS SET G = ST_POINT(30000, 30000, 4326) WHERE ID = 4000").unwrap();
    writer.run("UPDATE POINTS SET G = NULL WHERE ID = 4001").unwrap();
    writer.run("UPDATE POINTS SET G = ST_GEOMFROMTEXT('LINESTRING (4002 4002, 4002 4002)', 4326) WHERE ID = 4002").unwrap();
    writer.run("INSERT INTO POINTS VALUES (5000, ST_POINT(1, 1, 4326))").unwrap();
    writer.commit().unwrap();
    while let Some(row) = cursor.fetch().unwrap() {
        actual.push(row[0].to_string().parse().unwrap());
    }
    actual.sort_unstable();
    assert_eq!(actual, (0..5_000_i32).collect::<Vec<_>>());
    drop(cursor);
    reader.commit().unwrap();
    let ordered = format!("{JOIN} ORDER BY P.ID");
    let committed = rows(&writer, &ordered).unwrap();
    assert_eq!(committed, (0..=5_000_i32).filter(|n| ![4000, 4001].contains(n)).map(|n| n.to_string()).collect::<Vec<_>>());
    writer.run("UPDATE POINTS SET G = ST_POINT(30000, 30000, 4326)").unwrap();
    assert_eq!(rows(&writer, JOIN).unwrap(), Vec::<String>::new());
    writer.rollback().unwrap();
    assert_eq!(rows(&writer, &ordered).unwrap(), committed);
}
