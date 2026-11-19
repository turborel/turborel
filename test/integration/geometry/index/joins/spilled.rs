//! Broad geometry joins whose outer input outgrows its work area are
//! partitioned by envelope and spilled, and return the product's rows
//! (D38, D138).

use super::{populated, windows};
use crate::common::rows;
use turborel::db::Database;
use turborel::error::Code;
use turborel::params::Params;

const JOIN: &str = "SELECT P.ID, W.ID FROM WINDOWS W JOIN SHAPES P ON ST_INTERSECTS(P.G, W.G) = 1";

/// SHAPES with squares beside its points, and many broad windows beside
/// a polygon, a point, an empty geometry and a null.
fn crowded() -> Database {
    let db = populated();
    windows(&db, "GEOMETRY(4326)");
    let a = db.session("A");
    let squares: Vec<_> = (0..200_i32)
        .map(|n| format!("({}, ST_MAKEENVELOPE({}, {}, {}, {}, 4326))", n + 3000, n * 9, n * 5, n * 9 + 40, n * 5 + 40))
        .collect();
    a.run(&format!("INSERT INTO SHAPES VALUES {}", squares.join(","))).unwrap();
    for start in (0..600_i32).step_by(200) {
        let values: Vec<_> = (start..start + 200)
            .map(|n| {
                format!(
                    "({}, ST_MAKEENVELOPE({}, {}, {}, {}, 4326))",
                    n + 100,
                    (n * 13) % 1900,
                    (n * 7) % 1900,
                    (n * 13) % 1900 + 60,
                    (n * 7) % 1900 + 60
                )
            })
            .collect();
        a.run(&format!("INSERT INTO WINDOWS VALUES {}", values.join(","))).unwrap();
    }
    a.commit().unwrap();
    db
}

fn sorted(db: &Database, work: Option<&str>, query: &str) -> Result<Vec<String>, Code> {
    let a = db.session("A");
    if let Some(work) = work {
        a.run(&format!("SET MEMORY.WORK_BYTES = {work}")).unwrap();
    }
    let found = rows(&a, query);
    a.rollback().unwrap();
    found.map(|mut found| {
        found.sort();
        found
    })
}

/// Verifies: [DESIGN] D38, D85, D138: an adaptive join past its work area
/// gives the pairs it gives in memory and the unindexed product gives,
/// shapes spanning cells produced once, with NULL and empty geometries,
/// residuals and projections, at small and smaller work areas.
#[test]
fn spilled_spatial_joins_equal_in_memory_joins_and_products() {
    let db = crowded();
    let queries = [
        JOIN,
        "SELECT P.ID, W.ID FROM SHAPES P JOIN WINDOWS W ON 1 = ST_INTERSECTS(W.G, P.G) AND P.ID <> W.ID",
        "SELECT W.ID, P.ID, P.ID FROM WINDOWS W JOIN (SELECT G AS H, ID FROM SHAPES WHERE ID > 100) P ON ST_INTERSECTS(P.H, W.G) = 1",
    ];
    let a = db.session("A");
    for query in queries {
        assert!(a.physical_plan(query, &Params::new()).unwrap().contains("AdaptiveSpatialJoin"), "{query}");
    }
    let in_memory: Vec<_> = queries.iter().map(|q| sorted(&db, None, q).unwrap()).collect();
    assert!(in_memory[0].contains(&"3001,100".to_owned()) && !in_memory[0].iter().any(|r| r.ends_with(",12") || r.ends_with(",13")));
    for work in ["65536", "98304"] {
        for (query, expected) in queries.iter().zip(&in_memory) {
            assert_eq!(&sorted(&db, Some(work), query).unwrap(), expected, "{work}: {query}");
        }
    }
    db.drop_index("SHAPES_G").unwrap();
    for (query, expected) in queries.iter().zip(&in_memory) {
        assert_eq!(&sorted(&db, None, query).unwrap(), expected, "{query}");
    }
}

/// Verifies: [DESIGN] D15, D138: an error of the select list is raised
/// over a spilled join's pairs as over the product's; a spilled join of
/// no pairs ends without error.
#[test]
fn spilled_spatial_joins_raise_as_products_do() {
    let db = crowded();
    let failing = "SELECT 1 / (P.ID - P.ID) FROM WINDOWS W JOIN SHAPES P ON ST_INTERSECTS(P.G, W.G) = 1";
    assert_eq!(sorted(&db, Some("65536"), failing).unwrap_err(), Code::Data);
    let none = format!("{JOIN} AND W.ID < 0");
    assert_eq!(sorted(&db, Some("65536"), &none).unwrap(), Vec::<String>::new());
    db.drop_index("SHAPES_G").unwrap();
    assert_eq!(sorted(&db, Some("65536"), failing).unwrap_err(), Code::Data);
}
