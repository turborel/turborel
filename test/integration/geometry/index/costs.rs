//! Spatial probes priced by the geometry column's statistics (D137): the
//! rows a window meets from the envelope grid, the vertices each row's
//! evaluation tests, and how closely the rows a probe fetches lie.

use super::*;

/// The table `name` of `values`, each `(id, geometry SQL)`, inserted in
/// the order given, indexed by an rtree, its statistics gathered.
fn loaded(db: &Database, name: &str, values: &[(u32, String)]) {
    let a = db.session("A");
    a.run(&format!("CREATE TABLE {name} (ID INTEGER, G GEOMETRY(0))")).unwrap();
    for chunk in values.chunks(200) {
        let rows = chunk.iter().map(|(id, g)| format!("({id}, {g})")).collect::<Vec<_>>();
        a.run(&format!("INSERT INTO {name} VALUES {}", rows.join(","))).unwrap();
    }
    a.commit().unwrap();
    db.create_index(&format!("{name}_G"), &format!("A.{name}"), &["G"], "rtree").unwrap();
    db.analyze(&format!("A.{name}")).unwrap();
}

fn schema() -> Database {
    let db = registered();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db
}

/// The ids 0 to `n - 1`, in order or scattered by a step prime to `n`.
fn order(n: u32, scattered: bool) -> Vec<u32> {
    (0..n).map(|i| if scattered { i.wrapping_mul(7_919) % n } else { i }).collect()
}

/// The number EXPLAIN's spatial note gives after `label`.
fn noted(plan: &str, label: &str) -> f64 {
    let note = plan.split("[spatial: ").nth(1).unwrap_or_else(|| panic!("no note in {plan}"));
    let after = note.split(&format!("{label}=")).nth(1).unwrap_or_else(|| panic!("no {label} in {plan}"));
    after.split([',', ' ', ']']).next().unwrap().parse().unwrap()
}

/// Verifies: [DESIGN] D137: once a geometry column's statistics are
/// gathered, a broad window's rows are estimated from its envelope grid,
/// not the tree, near the rows it meets; EXPLAIN shows the estimate, the
/// vertices, the clustering and both costs, and the rows are a scan's.
#[test]
fn broad_windows_are_estimated_from_the_envelope_grid() {
    let db = schema();
    let points: Vec<_> = (0..20_000_u32).map(|id| (id, format!("ST_POINT({}, {})", id % 200, id.div_euclid(200)))).collect();
    loaded(&db, "GRID", &points);
    let a = db.session("A");
    let sql = "SELECT ID FROM GRID WHERE ST_INTERSECTS(G, ST_MAKEENVELOPE(0, 0, 30, 40)) = 1";
    let plan = a.physical_plan(sql, &Params::new()).unwrap();
    assert!(plan.contains("IndexScan") && plan.contains("by statistics"), "{plan}");
    let est = noted(&plan, "est. rows");
    assert!((est - 1271.0_f64).abs() < 1271.0_f64 * 0.15_f64, "{plan}");
    assert!(noted(&plan, "vertices") > 0.0_f64 && noted(&plan, "scan cost") > noted(&plan, "probe cost"), "{plan}");
    assert!(noted(&plan, "clustering") > 0.9_f64, "{plan}");
    assert_eq!(rows(&a, sql).unwrap().len(), 1271);
    let narrow = a.physical_plan("SELECT ID FROM GRID WHERE ST_INTERSECTS(G, ST_POINT(3, 3)) = 1", &Params::new()).unwrap();
    assert!(narrow.contains("by the index"), "a window of few rows is counted by the index: {narrow}");
}

/// Verifies: [DESIGN] D137: rows laid out in space are fetched near each
/// other, so a probe of three tenths of them costs less than a scan, and
/// the same rows in no spatial order are scanned. MC/DC: clustering alone
/// differs between the two tables.
#[test]
fn clustered_rows_keep_a_broad_probe_and_scattered_rows_scan() {
    let db = schema();
    for (name, scattered) in [("LAID", false), ("STREWN", true)] {
        let points: Vec<_> =
            order(20_000, scattered).into_iter().map(|id| (id, format!("ST_POINT({}, {})", id % 200, id.div_euclid(200)))).collect();
        loaded(&db, name, &points);
    }
    let a = db.session("A");
    let window = "ST_INTERSECTS(G, ST_MAKEENVELOPE(0, 0, 59.5, 100)) = 1";
    let laid = a.physical_plan(&format!("SELECT ID FROM LAID WHERE {window}"), &Params::new()).unwrap();
    let strewn = a.physical_plan(&format!("SELECT ID FROM STREWN WHERE {window}"), &Params::new()).unwrap();
    assert!(laid.contains("IndexScan") && noted(&laid, "clustering") > 0.9_f64, "{laid}");
    assert!(strewn.contains("SeqScan") && noted(&strewn, "clustering") < 0.2_f64, "{strewn}");
    let mut found = [
        rows(&a, &format!("SELECT ID FROM LAID WHERE {window}")).unwrap(),
        rows(&a, &format!("SELECT ID FROM STREWN WHERE {window}")).unwrap(),
    ];
    for ids in &mut found {
        ids.sort();
    }
    assert_eq!(found[0].len(), 6_000);
    assert_eq!(found[0], found[1]);
}

/// A regular polygon of `sides` sides inscribed in the cell `(x, y)`.
fn polygon(x: u32, y: u32, sides: u32) -> String {
    let (cx, cy) = (f64::from(x) + 0.5_f64, f64::from(y) + 0.5_f64);
    let ring = (0..=sides)
        .map(|k| {
            let angle = std::f64::consts::TAU * f64::from(k % sides) / f64::from(sides);
            format!("{} {}", cx + 0.4_f64 * angle.cos(), cy + 0.4_f64 * angle.sin())
        })
        .collect::<Vec<_>>();
    format!("ST_GEOMFROMTEXT('POLYGON (({}))')", ring.join(", "))
}

/// Verifies: [DESIGN] D137: a spatial conjunct's evaluation grows with
/// its rows' vertices, which a scan pays of every row and a probe of the
/// rows it finds, so the same window over squares is scanned and over
/// polygons of 200 vertices probed. MC/DC: vertices alone differ.
#[test]
fn many_vertices_turn_a_scan_into_a_probe() {
    let db = schema();
    for (name, sides) in [("SQUARES", 4_u32), ("ROUNDS", 199)] {
        let shapes: Vec<_> = order(3_000, true).into_iter().map(|id| (id, polygon(id % 60, id.div_euclid(60), sides))).collect();
        loaded(&db, name, &shapes);
    }
    let a = db.session("A");
    let window = "ST_INTERSECTS(G, ST_MAKEENVELOPE(0, -1, 17.95, 51)) = 1";
    let squares = a.physical_plan(&format!("SELECT ID FROM SQUARES WHERE {window}"), &Params::new()).unwrap();
    let rounds = a.physical_plan(&format!("SELECT ID FROM ROUNDS WHERE {window}"), &Params::new()).unwrap();
    assert!(squares.contains("SeqScan") && noted(&squares, "vertices") < 20.0_f64, "{squares}");
    assert!(rounds.contains("IndexScan") && noted(&rounds, "vertices") > 150.0_f64, "{rounds}");
    assert_eq!(rows(&a, &format!("SELECT ID FROM SQUARES WHERE {window}")).unwrap().len(), 900);
    assert_eq!(rows(&a, &format!("SELECT ID FROM ROUNDS WHERE {window}")).unwrap().len(), 900);
}
