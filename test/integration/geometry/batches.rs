//! Scalar spatial filters retain row-path answers and delayed errors.

use super::*;

/// Verifies: [DESIGN] D15, D31, D85: direct batch scalar comparisons
/// preserve results and first-row behavior with both argument orders,
/// empty geometry, NULL and a later SRID error. EXISTS can finish at its
/// first match; opening a complete query, even with FETCH FIRST, still
/// reports the later error, with batches on or off.
#[test]
fn batch_spatial_calls_match_rows_and_preserve_error_prefixes() {
    for engine in ["rowstore", "columnstore"] {
        let db = registered();
        db.schema(&format!("CREATE SCHEMA AUTHORIZATION A CREATE TABLE SHAPES (ID INTEGER, G GEOMETRY) USING {engine}")).unwrap();
        let a = db.session("A");
        a.run("INSERT INTO SHAPES VALUES (1, ST_POINT(1, 1)), (2, ST_GEOMFROMTEXT('POINT EMPTY')), (3, NULL), (4, ST_POINT(9, 9))")
            .unwrap();
        a.commit().unwrap();
        for batches in ["OFF", "ON"] {
            a.run(&format!("SET EXECUTOR.BATCHES = '{batches}'")).unwrap();
            for arguments in ["G, ST_MAKEENVELOPE(0, 0, 2, 2)", "ST_MAKEENVELOPE(0, 0, 2, 2), G"] {
                let filter = format!("ST_INTERSECTS({arguments})");
                assert_eq!(rows(&a, &format!("SELECT ID FROM SHAPES WHERE {filter} = 1")).unwrap(), ["1"]);
                assert_eq!(rows(&a, &format!("SELECT ID FROM SHAPES WHERE 1 <= {filter}")).unwrap(), ["1"]);
                // An empty value relates as NULL [ISO/IEC 13249-3:2016 §5.1.48].
                assert_eq!(rows(&a, &format!("SELECT ID FROM SHAPES WHERE {filter} <> 1 ORDER BY ID")).unwrap(), ["4"]);
            }
            a.run("INSERT INTO SHAPES VALUES (5, ST_POINT(1, 1, 4326))").unwrap();
            let sql = "SELECT ID FROM SHAPES WHERE ST_INTERSECTS(G, ST_POINT(1, 1)) = 1";
            let query = a.prepare(&format!("{sql} FOR READ ONLY")).unwrap();
            assert_eq!(a.open_prepared(&query, &[]).err().unwrap().code, Code::Data);
            assert_eq!(rows(&a, &format!("VALUES CASE WHEN EXISTS ({sql}) THEN 1 ELSE 0 END")).unwrap(), ["1"]);
            assert_eq!(rows(&a, &format!("{sql} FETCH FIRST 1 ROW ONLY")).unwrap_err(), Code::Data);
            assert_eq!(rows(&a, sql).unwrap_err(), Code::Data);
            a.rollback().unwrap();
        }
    }
}
