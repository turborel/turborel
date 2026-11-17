//! Reversing a spatial join does not retain an unnecessary geometry projection.

use super::{Params, populated, rows, windows};

/// Verifies: [DESIGN] D31, D85: either written join order uses one final
/// column selection above the spatial join, including repeated output
/// columns. Reversing the join does not retain a full geometry pair only
/// to discard its geometries in a second selection.
#[test]
fn reversed_spatial_joins_select_the_final_columns_directly() {
    let db = populated();
    windows(&db, "GEOMETRY(4326)");
    let session = db.session("A");
    let selections = [
        ("P.ID, W.ID", ["0,10", "1,10", "2,10", "2002,10", "3,10", "5,11"]),
        ("P.ID, W.ID, P.ID", ["0,10,0", "1,10,1", "2,10,2", "2002,10,2002", "3,10,3", "5,11,5"]),
    ];
    for (columns, expected) in selections {
        for from in ["SHAPES P JOIN WINDOWS W", "WINDOWS W JOIN SHAPES P"] {
            let query = format!("SELECT {columns} FROM {from} ON ST_INTERSECTS(P.G, W.G) = 1");
            let plan = session.physical_plan(&query, &Params::new()).unwrap();
            assert!(plan.contains("AdaptiveSpatialJoin"), "{plan}");
            assert_eq!(plan.lines().filter(|line| line.trim_start().starts_with("Project ")).count(), 1, "{plan}");
            let mut actual = rows(&session, &query).unwrap();
            actual.sort();
            assert_eq!(actual, expected);
        }
    }
}
