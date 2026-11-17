//! Bounded statistics keep useful spatial probes beyond an exact-count cap.

use super::*;

/// Verifies: [DESIGN] D35, D78: a window matching over 1000 candidates
/// can still use its spatial index. Its complete result agrees with both
/// integer-coordinate inclusion and a scan after dropping the index.
/// The old exact-count cutoff fell back to a whole-table estimate here.
#[test]
fn broad_spatial_statistics_keep_selective_windows_indexed() {
    let db = registered();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE GRID (ID INTEGER, G GEOMETRY(0))").unwrap();
    let a = db.session("A");
    for start in (0..20_000_u32).step_by(200) {
        let values =
            (start..start + 200).map(|id| format!("({id}, ST_POINT({}, {}))", id % 200, id.checked_div(200).unwrap())).collect::<Vec<_>>();
        a.run(&format!("INSERT INTO GRID VALUES {}", values.join(","))).unwrap();
    }
    a.commit().unwrap();
    db.create_index("GRID_G", "A.GRID", &["G"], "rtree").unwrap();
    let sql = "SELECT ID FROM GRID WHERE ST_INTERSECTS(G, ST_MAKEENVELOPE(0, 0, 30, 40)) = 1";
    let plan = a.physical_plan(sql, &Params::new()).unwrap();
    assert!(plan.contains("IndexScan"), "{plan}");
    let mut indexed = rows(&a, sql).unwrap();
    indexed.sort();
    let mut expected =
        (0..20_000_u32).filter(|id| id % 200 <= 30 && id.checked_div(200).unwrap() <= 40).map(|id| id.to_string()).collect::<Vec<_>>();
    expected.sort();
    assert_eq!(indexed.len(), 1271);
    assert_eq!(indexed, expected);
    a.commit().unwrap();
    db.drop_index("GRID_G").unwrap();
    let mut scanned = rows(&a, sql).unwrap();
    scanned.sort();
    assert_eq!(indexed, scanned);
}
