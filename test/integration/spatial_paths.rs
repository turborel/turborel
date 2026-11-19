//! Shortest paths through SQL [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §13.1]: ST_ShortestUndPath and ST_ShortestDirPath over a table of
//! paths named by its name and its columns' names.

use turborel::db::{Database, Session};
use turborel::error::Code;
use turborel::value::Value;

use crate::common::{failed, rows};

/// A session of user A with table ROADS: the four sides of the square of
/// side 10 at the origin, of weight 10 each, and its diagonal from the
/// origin, of weight 25; each with the point it is walked from, the
/// second and the diagonal from their ends; and a row of a point, which
/// is no path.
fn roads() -> Session {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A \
         CREATE TABLE ROADS (ID INTEGER, GEOM ST_GEOMETRY, LEN DOUBLE PRECISION, FROM_PT ST_POINT)",
    )
    .unwrap();
    let a = db.session("A");
    for (id, line, len, from) in [
        (1, "0 0, 10 0", 10, "0, 0"),
        (2, "10 0, 10 10", 10, "10, 10"),
        (3, "0 0, 0 10", 10, "0, 0"),
        (4, "0 10, 10 10", 10, "0, 10"),
        (5, "0 0, 10 10", 25, "10, 10"),
    ] {
        a.run(&format!("INSERT INTO ROADS VALUES ({id}, ST_GEOMFROMTEXT('LINESTRING ({line})'), {len}, ST_POINT({from}))")).unwrap();
    }
    a.run("INSERT INTO ROADS VALUES (6, ST_POINT(0, 0), 1, ST_POINT(0, 0))").unwrap();
    a
}

/// The rows of `call`, a table function's invocation, as text.
fn paths(a: &Session, call: &str) -> Vec<String> {
    rows(a, &format!("SELECT SHORTEST_PATH, TOTAL_WEIGHT FROM TABLE({call}) AS P ORDER BY TOTAL_WEIGHT")).unwrap()
}

/// The SQLSTATE `call` fails with.
fn refused(a: &Session, call: &str) -> String {
    failed(a, &format!("SELECT * FROM TABLE({call}) AS P")).sqlstate().as_str().to_owned()
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §13.1.1 Description
/// 4, 5, 8, 11] the undirected shortest paths by the weight column, two
/// of the same total both returned; by the curves' lengths, the diagonal;
/// the row of a point ignored.
#[test]
fn undirected_shortest_paths() {
    let a = roads();
    let weighted = "ST_SHORTESTUNDPATH('ROADS', 'ID', 'GEOM', 'LEN', ST_POINT(0, 0), ST_POINT(10, 10))";
    let mut found = paths(&a, weighted);
    found.sort();
    assert_eq!(found, ["ARRAY[1, 2],2E1", "ARRAY[3, 4],2E1"]);
    let by_length = "ST_SHORTESTUNDPATH('ROADS', 'ID', 'GEOM', ST_POINT(10, 10), ST_POINT(0, 0))";
    assert_eq!(paths(&a, by_length), ["ARRAY[5],1.4142135623730951E1"]);
    let back = "ST_SHORTESTUNDPATH('ROADS', 'ID', 'GEOM', 'LEN', ST_POINT(10, 0), ST_POINT(0, 10))";
    assert_eq!(paths(&a, back).len(), 2);
    let itself = "ST_SHORTESTUNDPATH(' ROADS ', 'ID', 'GEOM', 'LEN', ST_POINT(0, 0), ST_POINT(0, 0))";
    assert_eq!(paths(&a, itself), ["ARRAY[],0E0"]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §13.1.2 Description
/// 4, 5, 10] the directed shortest paths: each path walked from its start
/// point only, the diagonal from its end; no row where no path leads.
#[test]
fn directed_shortest_paths() {
    let a = roads();
    let call =
        |from: &str, to: &str| format!("ST_SHORTESTDIRPATH('ROADS', 'ID', 'GEOM', 'FROM_PT', 'LEN', ST_POINT({from}), ST_POINT({to}))");
    assert_eq!(paths(&a, &call("0, 0", "10, 10")), ["ARRAY[3, 4],2E1"]);
    assert_eq!(paths(&a, &call("10, 10", "0, 0")), ["ARRAY[5],2.5E1"]);
    assert_eq!(paths(&a, &call("0, 10", "0, 0")), ["ARRAY[4, 5],3.5E1"]);
    assert_eq!(paths(&a, &call("10, 0", "0, 0")), Vec::<String>::new());
    let by_length = "ST_SHORTESTDIRPATH('ROADS', 'ID', 'GEOM', 'FROM_PT', ST_POINT(0, 10), ST_POINT(10, 0))";
    assert_eq!(paths(&a, by_length), ["ARRAY[4, 2],2E1"]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §13.1.1 Description
/// 2, 7, 9; §13.1.2 Description 2] the exceptions: a name that is no
/// table name, a table or column that is not there, a point at no path's
/// end, a weight that is negative, a start point at neither end, two
/// paths of one identifier; a table of no rows gives none; a name not
/// known as the invocation is bound is refused.
#[test]
fn exceptions_of_the_path_functions() {
    let a = roads();
    let und = |table: &str, id: &str, from: &str| {
        format!("ST_SHORTESTUNDPATH('{table}', '{id}', 'GEOM', 'LEN', ST_POINT({from}), ST_POINT(10, 10))")
    };
    assert_eq!(refused(&a, &und("1 ROADS", "ID", "0, 0")), "2FF02");
    assert_eq!(refused(&a, &und("NOWHERE", "ID", "0, 0")), "2FF02");
    assert_eq!(refused(&a, &und("ROADS", "id", "0, 0")), "2FF02");
    assert_eq!(refused(&a, &und("ROADS", "ID", "5, 5")), "2FF02");
    a.run("CREATE TABLE EMPTY_ROADS (ID INTEGER, GEOM ST_GEOMETRY, LEN DOUBLE PRECISION)").unwrap();
    assert_eq!(paths(&a, &und("EMPTY_ROADS", "ID", "5, 5")), Vec::<String>::new());
    a.run("UPDATE ROADS SET FROM_PT = ST_POINT(5, 5) WHERE ID = 1").unwrap();
    let dir = "ST_SHORTESTDIRPATH('ROADS', 'ID', 'GEOM', 'FROM_PT', 'LEN', ST_POINT(0, 0), ST_POINT(10, 10))";
    assert_eq!(refused(&a, dir), "2FF02");
    a.run("UPDATE ROADS SET LEN = -1 WHERE ID = 2").unwrap();
    assert_eq!(refused(&a, &und("ROADS", "ID", "0, 0")), "2FF02");
    a.run("UPDATE ROADS SET LEN = 10 WHERE ID = 2").unwrap();
    a.run("INSERT INTO ROADS VALUES (3, ST_GEOMFROMTEXT('LINESTRING (0 0, 1 1)'), 1, ST_POINT(0, 0))").unwrap();
    assert_eq!(refused(&a, &und("ROADS", "ID", "0, 0")), "2FF05");
    let lateral =
        "SELECT * FROM (VALUES ('ROADS')) AS X (N), TABLE(ST_SHORTESTUNDPATH(X.N, 'ID', 'GEOM', ST_POINT(0, 0), ST_POINT(1, 1))) AS P";
    assert_eq!(failed(&a, lateral).code, Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.1.3, §13.1.1]
/// the shortest path over a network's view of links, by their curves'
/// lengths, a qualified table name naming the view.
#[test]
fn shortest_path_over_a_network() {
    let a = roads();
    a.run("CALL ST_INITTOPONET('N')").unwrap();
    let g = "TREAT(ST_GEOMFROMTEXT('MULTILINESTRING ((0 0, 3 4), (3 4, 3 10), (0 0, 0 10), (0 10, 3 10))') AS ST_GEOMCOLLECTION)";
    a.run(&format!("CALL ST_SPATNETFROMGEOM('N', {g})")).unwrap();
    let call = "ST_SHORTESTUNDPATH('\"N\".ST_LINK', 'LINK_ID', 'GEOMETRY', ST_POINT(0, 0), ST_POINT(3, 10))";
    assert_eq!(paths(&a, call), ["ARRAY[1, 2],1.1E1"]);
}
