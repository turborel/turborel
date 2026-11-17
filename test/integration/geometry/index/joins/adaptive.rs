//! Broad geometry joins retain exact refinement, residuals and snapshots
//! when the small input permits a single scan of the large input.

use super::{populated, windows};
use crate::common::{rows, serializable};
use turborel::db::Database;
use turborel::error::Code;
use turborel::params::Params;
use turborel::value::Value;

const JOIN: &str = "SELECT P.ID, W.ID FROM WINDOWS W JOIN SHAPES P ON ST_INTERSECTS(P.G, W.G) = 1";

fn broad() -> Database {
    let db = populated();
    windows(&db, "GEOMETRY(4326)");
    let a = db.session("A");
    a.run("UPDATE WINDOWS SET G = ST_MAKEENVELOPE(-1, -1, 3000, 3000, 4326) WHERE ID = 10").unwrap();
    a.run("UPDATE WINDOWS SET G = ST_GEOMFROMTEXT('POLYGON ((-1 -1, 3000 -1, 3000 3000, -1 3000, -1 -1), (2 2, 2 20, 20 20, 20 2, 2 2))', 4326) WHERE ID = 11").unwrap();
    a.commit().unwrap();
    db
}

/// Verifies: [DESIGN] D15, D85: broad adaptive joins preserve holes,
/// boundary points, NULL/EMPTY values, both argument and input orders,
/// projected inner columns, duplicate output columns and ON residuals.
/// Disabling preparation or dropping the index yields the same pairs.
#[test]
fn adaptive_spatial_joins_match_products_and_preparation_modes() {
    let db = broad();
    let a = db.session("A");
    let queries = [
        JOIN,
        "SELECT P.ID, W.ID FROM SHAPES P JOIN WINDOWS W ON 1 = ST_INTERSECTS(W.G, P.G) AND P.ID < W.ID",
        "SELECT P.ID, P.ID, W.ID FROM WINDOWS W JOIN (SELECT G AS H, ID FROM SHAPES WHERE ID < 100) P ON ST_INTERSECTS(P.H, W.G) = 1",
    ];
    let mut results = Vec::new();
    for query in queries {
        let plan = a.physical_plan(query, &Params::new()).unwrap();
        assert!(plan.contains("AdaptiveSpatialJoin"), "{plan}");
        let mut found = rows(&a, query).unwrap();
        found.sort();
        results.push(found);
    }
    for setting in ["EXECUTOR.BATCHES", "OPTIMIZER.CONSTANT_FOLDING"] {
        db.set_setting(setting, &Value::Char("OFF".into())).unwrap();
        for (query, expected) in queries.iter().zip(&results) {
            let mut found = rows(&a, query).unwrap();
            found.sort();
            assert_eq!(&found, expected, "{setting}: {query}");
        }
        db.set_setting(setting, &Value::Char("ON".into())).unwrap();
    }
    let ordered = format!("{JOIN} ORDER BY P.ID, W.ID");
    assert!(!a.physical_plan(&ordered, &Params::new()).unwrap().contains("AdaptiveSpatialJoin"));
    a.commit().unwrap();
    db.drop_index("SHAPES_G").unwrap();
    for (query, expected) in queries.iter().zip(results) {
        let mut found = rows(&a, query).unwrap();
        found.sort();
        assert_eq!(found, expected, "{query}");
    }
}

/// Verifies: [DESIGN] D29, D85: the scan path reads statement-visible
/// shapes across moved, equal-envelope, deleted, inserted and NULL rows.
/// A paused repeatable-read cursor retains its original complete pairs;
/// a later statement sees committed changes and rollback restores them.
#[test]
fn adaptive_spatial_joins_keep_their_snapshot_across_writes() {
    let db = broad();
    let (reader, writer) = (db.session("A"), db.session("A"));
    reader.run("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ").unwrap();
    let mut expected = rows(&reader, JOIN).unwrap();
    expected.sort();
    let mut cursor = reader.open_cursor(JOIN).unwrap();
    let mut actual = vec![cursor.fetch().unwrap().unwrap().iter().map(ToString::to_string).collect::<Vec<_>>().join(",")];
    writer.run("UPDATE SHAPES SET G = ST_POINT(10000, 10000, 4326) WHERE ID = 1").unwrap();
    writer.run("UPDATE SHAPES SET G = NULL WHERE ID = 2").unwrap();
    writer.run("UPDATE SHAPES SET G = ST_MAKEENVELOPE(0, 0, 10, 10, 4326) WHERE ID = 2002").unwrap();
    writer.run("DELETE FROM SHAPES WHERE ID = 3").unwrap();
    writer.run("INSERT INTO SHAPES VALUES (3000, ST_POINT(8, 8, 4326))").unwrap();
    writer.commit().unwrap();
    while let Some(row) = cursor.fetch().unwrap() {
        actual.push(row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    }
    drop(cursor);
    actual.sort();
    assert_eq!(actual, expected);
    let mut repeated = rows(&reader, JOIN).unwrap();
    repeated.sort();
    assert_eq!(repeated, expected);
    reader.commit().unwrap();
    let mut committed = rows(&writer, JOIN).unwrap();
    committed.sort();
    assert_ne!(committed, expected);
    writer.run("UPDATE SHAPES SET G = NULL").unwrap();
    assert_eq!(rows(&writer, JOIN).unwrap(), Vec::<String>::new());
    writer.rollback().unwrap();
    let mut restored = rows(&writer, JOIN).unwrap();
    restored.sort();
    assert_eq!(restored, committed);
}

/// Verifies: [DESIGN] D29, D85: broad adaptive scans record predicate
/// reads. Two serializable readers cannot both commit new matching rows.
#[test]
fn adaptive_spatial_scans_record_serializable_reads() {
    let db = broad();
    serializable(&db);
    let (a, b) = (db.session("A"), db.session("A"));
    assert_ne!(rows(&a, JOIN).unwrap(), Vec::<String>::new());
    assert_ne!(rows(&b, JOIN).unwrap(), Vec::<String>::new());
    a.run("INSERT INTO SHAPES VALUES (4000, ST_POINT(100, 100, 4326))").unwrap();
    b.run("INSERT INTO SHAPES VALUES (4001, ST_POINT(100, 100, 4326))").unwrap();
    let commits = [a.commit(), b.commit()];
    assert!(commits.iter().any(Result::is_err));
    for error in commits.into_iter().filter_map(Result::err) {
        assert_eq!(error.code, Code::Conflict);
    }
}

/// Verifies: [DESIGN] D85: SQL planning permits adaptive spatial builds
/// beyond one mask word. Grouping still counts every matching pair;
/// removing the R-tree produces the same groups through a product.
#[test]
fn larger_spatial_builds_keep_grouped_sql_results() {
    let db = broad();
    let a = db.session("A");
    for id in 20..90_i32 {
        a.run(&format!("INSERT INTO WINDOWS VALUES ({id}, ST_MAKEENVELOPE(-1, -1, 3000, 3000, 4326))")).unwrap();
    }
    a.commit().unwrap();
    let query = "SELECT W.ID, COUNT(*) FROM WINDOWS W JOIN SHAPES P ON ST_INTERSECTS(P.G, W.G) = 1 GROUP BY W.ID";
    let plan = a.physical_plan(query, &Params::new()).unwrap();
    assert!(plan.contains("AdaptiveSpatialJoin"), "{plan}");
    let mut expected = rows(&a, query).unwrap();
    expected.sort();
    assert_eq!(expected.len(), 72);
    a.commit().unwrap();
    db.drop_index("SHAPES_G").unwrap();
    let mut actual = rows(&a, query).unwrap();
    actual.sort();
    assert_eq!(actual, expected);
}
