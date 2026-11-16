//! Prepared polygon indexes follow each execution's geometry parameter,
//! including after plan learning and with constant folding disabled.

use super::*;

fn polygon(hole: bool) -> Value {
    let mut points = Vec::new();
    for n in 0..16_u32 {
        points.push(format!("{n} 0"));
    }
    for n in 0..16_u32 {
        points.push(format!("16 {n}"));
    }
    for n in (1..=16_u32).rev() {
        points.push(format!("{n} 16"));
    }
    for n in (0..=16_u32).rev() {
        points.push(format!("0 {n}"));
    }
    let interior = if hole { ", (1.5 1.5, 2.5 1.5, 2.5 2.5, 1.5 2.5, 1.5 1.5)" } else { "" };
    Geometry::from_wkt(&format!("POLYGON (({}){interior})", points.join(", ")), 0).unwrap().to_value()
}

/// Verifies: [DESIGN] D15, D45, D82: a repeated SQL query uses the
/// current polygon's hole and boundary after plan learning. Prepared
/// interval indexes and ordinary calls return identical complete rows;
/// replacing a polygon by NULL, EMPTY or another SRID preserves behavior.
#[test]
fn prepared_polygon_queries_use_each_executions_index() {
    let db = registered();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE SHAPES (ID INTEGER, G GEOMETRY(0))").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO SHAPES VALUES (1, ST_POINT(1, 1)), (2, ST_POINT(2, 2)), (3, ST_POINT(16, 8))").unwrap();
    for folding in ["ON", "OFF"] {
        a.run(&format!("SET OPTIMIZER.CONSTANT_FOLDING = '{folding}'")).unwrap();
        let query = a.prepare("SELECT ID, ST_DISTANCE(G, ?) FROM SHAPES WHERE ST_INTERSECTS(G, ?) = 1 ORDER BY ID").unwrap();
        for n in 0..16_u32 {
            let hole = n % 2 == 0;
            let value = polygon(hole);
            let mut cursor = a.open_prepared(&query, &[value.clone(), value]).unwrap();
            let mut found = Vec::new();
            while let Some(row) = cursor.fetch().unwrap() {
                assert_eq!(row[1].as_f64().unwrap().to_bits(), 0.0_f64.to_bits());
                found.push(row[0].to_string());
            }
            assert_eq!(found, if hole { vec!["1", "3"] } else { vec!["1", "2", "3"] });
        }
        for value in [Value::Null, Geometry::from_wkt("POLYGON EMPTY", 0).unwrap().to_value()] {
            assert!(a.open_prepared(&query, &[value.clone(), value]).unwrap().fetch().unwrap().is_none());
        }
        let wrong = Geometry::from_value(&polygon(true)).unwrap().with_srid(4326).to_value();
        let result = a.open_prepared(&query, &[wrong.clone(), wrong]).and_then(|mut cursor| cursor.fetch());
        assert_eq!(result.unwrap_err().code, Code::Data);
    }
}
