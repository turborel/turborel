//! Nearest ordering of a distance in a unit through a geometry index
//! agrees with a full sort (D85, D135).

use crate::common::rows;
use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::params::Params;

/// A table METRIC of SRID 0 points along a line, a hundred units apart and
/// some at equal distances from (5000, 0) and two from (20050, 0), with an
/// empty and a NULL row, indexed by METRIC_G.
fn metric() -> (Database, Session) {
    let db = super::super::registered();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE METRIC (ID INTEGER, G GEOMETRY(0))").unwrap();
    let a = db.session("A");
    let values: Vec<String> = (0..200_i32).map(|id| format!("({id}, ST_POINT({}, {}, 0))", id * 100, (id % 3) * 40)).collect();
    a.run(&format!("INSERT INTO METRIC VALUES {}", values.join(","))).unwrap();
    a.run("INSERT INTO METRIC VALUES (500, ST_POINT(5000, 40, 0)), (503, ST_POINT(4900, 0, 0)), (504, ST_POINT(20000, 0, 0)), (505, ST_POINT(20100, 0, 0)), (501, ST_GEOMFROMTEXT('POINT EMPTY', 0)), (502, NULL)").unwrap();
    a.commit().unwrap();
    db.create_index("METRIC_G", "A.METRIC", &["G"], "rtree").unwrap();
    (db, a)
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §5.1.41 Description
/// 4] D85, D135: ORDER BY a distance in a unit that converts, with FETCH
/// FIRST, reads the R-tree's nearest candidates as the distance without a
/// unit does, in either orientation, with OFFSET, a filter and ties left
/// to the next sort key: a candidate's bound is converted as its measure
/// is, so one tied with the last row kept is still read. EXPLAIN shows the unit form. A unit whose numbers
/// may overflow where the column's cannot, FOOT, and one that does not
/// convert read the table. Dropping the index gives the same rows and the
/// same error.
#[test]
fn nearest_unit_forms_use_the_index_and_equal_scans() {
    let (db, a) = metric();
    let tied = |order: &str| {
        format!("SELECT ID FROM METRIC ORDER BY ST_DISTANCE(G, ST_POINT(20050, 0, 0), 'KILOMETRE'), ID {order} FETCH FIRST 1 ROW ONLY")
    };
    let near = |order: &str, rest: &str| format!("SELECT ID, {order} AS D FROM METRIC {rest} ORDER BY D, ID DESC FETCH FIRST 6 ROWS ONLY");
    let queries = [
        near("ST_DISTANCE(G, ST_POINT(5000, 0, 0), 'KILOMETRE')", ""),
        near("ST_DISTANCE(ST_POINT(5000, 0, 0), G, 'METRE')", "WHERE ID <> 49"),
        near("ST_DISTANCE(G, ST_POINT(5000, 0, 0), 'KILOMETRE')", "").replace(" FETCH", " OFFSET 3 ROWS FETCH"),
        tied("DESC"),
        tied("ASC"),
    ];
    let mut results = Vec::new();
    for query in &queries {
        let plan = a.physical_plan(query, &Params::new()).unwrap();
        assert!(plan.contains("NearestScan") && plan.contains("METRIC_G"), "{query}\n{plan}");
        results.push(rows(&a, query).unwrap());
    }
    let explained = a.physical_plan(&queries[0], &Params::new()).unwrap();
    assert!(
        explained.contains(
            "NearestScan A.METRIC using METRIC_G [rtree]: ST_DISTANCE(METRIC.G, ST_GEOMFROMTEXT('POINT (5e3 0)', 0), 'KILOMETRE')"
        ),
        "{explained}"
    );
    assert_eq!(&results[0][..4], ["500,4E-2", "50,8E-2", "503,1E-1", "51,1E-1"]);
    assert_eq!(&results[1][..4], ["500,4E1", "50,8E1", "503,1E2", "51,1E2"]);
    assert_eq!(&results[2][..1], &results[0][3..4]);
    assert_eq!(results[3..], [["505"], ["504"]]);
    let feet = near("ST_DISTANCE(G, ST_POINT(5000, 0, 0), 'FOOT')", "");
    let unknown = near("ST_DISTANCE(G, ST_POINT(5000, 0, 0), 'PARSEC')", "");
    for scanned in [&feet, &unknown] {
        assert!(!a.physical_plan(scanned, &Params::new()).unwrap().contains("NearestScan"), "{scanned}");
    }
    let foot = rows(&a, &feet).unwrap();
    let error = rows(&a, &unknown).unwrap_err();
    a.commit().unwrap();
    db.drop_index("METRIC_G").unwrap();
    for (query, expected) in queries.iter().zip(&results) {
        assert!(!a.physical_plan(query, &Params::new()).unwrap().contains("METRIC_G"));
        assert_eq!(&rows(&a, query).unwrap(), expected, "{query}");
    }
    assert_eq!(rows(&a, &feet).unwrap(), foot);
    assert_eq!(rows(&a, &unknown).unwrap_err(), error);
    assert_eq!(error, Code::Data);
}
