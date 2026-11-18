//! Topology-networks through SQL [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §12]: a network made by ST_InitTopoNet, edited by its procedures and
//! functions, checked by its table functions, in a transaction and through
//! recovery (RULES.md D125).

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::Code;
use turborel::value::Value;

use crate::common::{Scratch, after_crash, failed, rows};

fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db
}

/// The SQLSTATE statement `sql` fails with in `s`.
fn state(s: &Session, sql: &str) -> String {
    s.run(sql).unwrap_err().sqlstate().as_str().to_owned()
}

/// The SQLSTATE query `sql` fails with in `s`.
fn read_state(s: &Session, sql: &str) -> String {
    failed(s, sql).sqlstate().as_str().to_owned()
}

/// The value of function call `call` in `s`, as text: invoked where a
/// function that possibly modifies SQL-data may be, a row of an INSERT.
fn invoked(s: &Session, call: impl AsRef<str>) -> String {
    let call = call.as_ref();
    s.run(&format!("INSERT INTO R VALUES ({call})")).unwrap();
    let got = rows(s, "SELECT N FROM R").unwrap();
    s.run("DELETE FROM R").unwrap();
    got.concat()
}

/// The SQLSTATE function call `call` fails with in `s`.
fn invoke_state(s: &Session, call: &str) -> String {
    state(s, &format!("INSERT INTO R VALUES ({call})"))
}

/// A session of user A with network `N` made and a table R to take
/// values of the network's functions.
fn network(db: &Database) -> Session {
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE R (N INTEGER)").unwrap();
    let a = db.session("A");
    a.run("CALL ST_INITTOPONET('N')").unwrap();
    a
}

/// The ST_Curve of the linestring through `points`.
fn curve(points: &str) -> String {
    format!("TREAT(ST_GEOMFROMTEXT('LINESTRING ({points})') AS ST_CURVE)")
}

/// Nodes 1 to 3 at (0 0), (10 0) and (10 10); links 1 from node 1 to 2
/// and 2 from node 2 to 3, along straight lines.
fn corner(a: &Session) {
    for (x, y) in [(0, 0), (10, 0), (10, 10)] {
        invoked(a, format!("ST_ADDISONETNODE('N', ST_POINT({x}, {y}))"));
    }
    assert_eq!(invoked(a, format!("ST_ADDLINK('N', 1, 2, {})", curve("0 0, 10 0"))), "1");
    assert_eq!(invoked(a, format!("ST_ADDLINK('N', 2, 3, {})", curve("10 0, 10 10"))), "2");
}

/// The links of network N as text: identifier, start, end and curve.
fn links(a: &Session) -> Vec<String> {
    rows(a, "SELECT LINK_ID, START_NODE, END_NODE, ST_ASTEXT(GEOMETRY) FROM \"N\".ST_LINK ORDER BY LINK_ID").unwrap()
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.1, §12.3.1,
/// §12.3.4, §12.3.7, §12.3.17, §12.3.18] ST_InitTopoNet makes a schema of
/// two views; nodes and links added through SQL are read back from them;
/// the spatial network is valid as one, and the logical check lists each
/// primitive that has geometry; a second ST_InitTopoNet is 2FF56.
#[test]
fn a_spatial_network_is_built_and_checked() {
    let db = database();
    let a = network(&db);
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM \"N\".ST_NODE").unwrap(), ["0"]);
    corner(&a);
    let nodes = "SELECT NODE_ID, ST_ASTEXT(GEOMETRY) FROM \"N\".ST_NODE ORDER BY NODE_ID";
    assert_eq!(rows(&a, nodes).unwrap(), ["1,POINT (0 0)", "2,POINT (10 0)", "3,POINT (10 10)"]);
    assert_eq!(links(&a), ["1,1,2,LINESTRING (0 0, 10 0)", "2,2,3,LINESTRING (10 0, 10 10)"]);
    assert_eq!(rows(&a, "SELECT * FROM TABLE(ST_VALIDSPATIALNET('N')) AS V").unwrap(), Vec::<String>::new());
    let logical = rows(&a, "SELECT * FROM TABLE(ST_VALIDLOGICALNET('N')) AS V").unwrap();
    assert_eq!(
        logical,
        [
            "node has geometry,1,NULL",
            "node has geometry,2,NULL",
            "node has geometry,3,NULL",
            "link has geometry,1,NULL",
            "link has geometry,2,NULL"
        ]
    );
    assert_eq!(state(&a, "CALL ST_INITTOPONET('N')"), "2FF56");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.8, §12.3.9,
/// §12.3.12, §12.3.13] a logical network split into new links and into a
/// modified one, both in the split link's direction, then healed by a new
/// link and into the first; a ROLLBACK undoes the rows.
#[test]
fn a_logical_network_is_split_and_healed() {
    let db = database();
    let a = network(&db);
    for _ in 0..2 {
        invoked(&a, "ST_ADDISONETNODE('N', CAST(NULL AS ST_POINT))");
    }
    assert_eq!(invoked(&a, "ST_ADDLINK('N', 1, 2, CAST(NULL AS ST_CURVE))"), "1");
    a.commit().unwrap();
    assert_eq!(invoked(&a, "ST_NEWLOGLINKSPLIT('N', 1)"), "3");
    assert_eq!(links(&a), ["2,1,3,NULL", "3,3,2,NULL"]);
    assert_eq!(invoked(&a, "ST_MODLOGLINKSPLIT('N', 3)"), "4");
    assert_eq!(links(&a), ["2,1,3,NULL", "3,3,4,NULL", "4,4,2,NULL"]);
    assert_eq!(invoked(&a, "ST_NEWLINKHEAL('N', 3, 4)"), "5");
    assert_eq!(links(&a), ["2,1,3,NULL", "5,3,2,NULL"]);
    a.run("CALL ST_MODLINKHEAL('N', 5, 2)").unwrap();
    assert_eq!(links(&a), ["5,1,2,NULL"]);
    assert_eq!(rows(&a, "SELECT NODE_ID FROM \"N\".ST_NODE ORDER BY NODE_ID").unwrap(), ["1", "2"]);
    assert_eq!(rows(&a, "SELECT * FROM TABLE(ST_VALIDLOGICALNET('N')) AS V").unwrap(), Vec::<String>::new());
    let spatial = rows(&a, "SELECT * FROM TABLE(ST_VALIDSPATIALNET('N')) AS V").unwrap();
    assert_eq!(spatial, ["missing node geometry,1,NULL", "missing node geometry,2,NULL", "missing link geometry,5,NULL"]);
    a.rollback().unwrap();
    assert_eq!(links(&a), ["1,1,2,NULL"]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.10 to
/// §12.3.13] a spatial link split at a point within it cuts its curve
/// there, and healing joins the curves again in the first link's
/// direction, whichever ends the two links meet at.
#[test]
fn a_spatial_link_is_split_and_healed() {
    let db = database();
    let a = network(&db);
    corner(&a);
    assert_eq!(invoked(&a, "ST_NEWGEOLINKSPLIT('N', 1, ST_POINT(4, 0))"), "4");
    assert_eq!(links(&a), ["2,2,3,LINESTRING (10 0, 10 10)", "3,1,4,LINESTRING (0 0, 4 0)", "4,4,2,LINESTRING (4 0, 10 0)"]);
    assert_eq!(invoked(&a, "ST_MODGEOLINKSPLIT('N', 2, ST_POINT(10, 5))"), "5");
    assert_eq!(links(&a)[0], "2,2,5,LINESTRING (10 0, 10 5)");
    assert_eq!(links(&a)[3], "5,5,3,LINESTRING (10 5, 10 10)");
    assert_eq!(invoked(&a, "ST_NEWLINKHEAL('N', 3, 4)"), "6");
    assert_eq!(links(&a)[2], "6,1,2,LINESTRING (0 0, 4 0, 10 0)");
    a.run("CALL ST_MODLINKHEAL('N', 5, 2)").unwrap();
    assert_eq!(links(&a), ["5,2,3,LINESTRING (10 0, 10 5, 10 10)", "6,1,2,LINESTRING (0 0, 4 0, 10 0)"]);
    assert_eq!(rows(&a, "SELECT * FROM TABLE(ST_VALIDSPATIALNET('N')) AS V").unwrap(), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.2, §12.3.3,
/// §12.3.5, §12.3.6] an isolated node moved and removed, one a link ends
/// at refused; a link's curve changed, one whose ends are not its nodes'
/// refused; a link removed.
#[test]
fn nodes_and_links_are_moved_changed_and_removed() {
    let db = database();
    let a = network(&db);
    corner(&a);
    let n = invoked(&a, "ST_ADDISONETNODE('N', ST_POINT(5, 5))");
    a.run(&format!("CALL ST_MOVEISONETNODE('N', {n}, ST_POINT(6, 6))")).unwrap();
    assert_eq!(rows(&a, &format!("SELECT ST_ASTEXT(GEOMETRY) FROM \"N\".ST_NODE WHERE NODE_ID = {n}")).unwrap(), ["POINT (6 6)"]);
    assert_eq!(state(&a, "CALL ST_MOVEISONETNODE('N', 2, ST_POINT(6, 6))"), "2FF50");
    assert_eq!(state(&a, "CALL ST_REMISONETNODE('N', 2)"), "2FF50");
    a.run(&format!("CALL ST_REMISONETNODE('N', {n})")).unwrap();
    a.run(&format!("CALL ST_CHANGELINKGEOM('N', 1, {})", curve("0 0, 5 -1, 10 0"))).unwrap();
    assert_eq!(links(&a)[0], "1,1,2,LINESTRING (0 0, 5 -1, 10 0)");
    assert_eq!(state(&a, &format!("CALL ST_CHANGELINKGEOM('N', 1, {})", curve("1 0, 10 0"))), "2FF57");
    assert_eq!(state(&a, &format!("CALL ST_CHANGELINKGEOM('N', 1, {})", curve("0 0, 9 0"))), "2FF33");
    a.run("CALL ST_REMOVELINK('N', 2)").unwrap();
    assert_eq!(links(&a), ["1,1,2,LINESTRING (0 0, 5 -1, 10 0)"]);
    a.run("CALL ST_REMISONETNODE('N', 3)").unwrap();
    a.run("CALL ST_CHANGELINKGEOM('N', 1, CAST(NULL AS ST_CURVE))").unwrap();
    assert_eq!(links(&a), ["1,1,2,NULL"]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3, §22] the
/// exceptions the routines raise through SQL, each in its Description's
/// order: a null argument, a name no network has, a node or link that is
/// not there, a link of the wrong kind, a point off a link, links that do
/// not meet or meet others, an empty network, mixed systems.
#[test]
fn exceptions_of_the_routines() {
    let db = database();
    let a = network(&db);
    assert_eq!(read_state(&a, "SELECT * FROM TABLE(ST_VALIDSPATIALNET('N')) AS V"), "2FF31");
    assert_eq!(read_state(&a, "SELECT * FROM TABLE(ST_VALIDLOGICALNET('N')) AS V"), "2FF31");
    corner(&a);
    a.commit().unwrap();
    let add = |name: &str| format!("ST_ADDISONETNODE({name}, ST_POINT(5, 5))");
    assert_eq!(invoke_state(&a, &add("CAST(NULL AS VARCHAR(1))")), "2FF03");
    assert_eq!(invoke_state(&a, &add("''")), "2FF64");
    assert_eq!(invoke_state(&a, &add("'NOWHERE'")), "2FF64");
    assert_eq!(state(&a, "CALL ST_INITTOPONET(CAST(NULL AS VARCHAR(1)))"), "2FF03");
    assert_eq!(state(&a, "CALL ST_REMISONETNODE('N', CAST(NULL AS INTEGER))"), "2FF03");
    assert_eq!(invoke_state(&a, &format!("ST_ADDLINK('N', 1, CAST(NULL AS INTEGER), {})", curve("0 0, 10 10"))), "2FF03");
    assert_eq!(invoke_state(&a, &format!("ST_ADDLINK('N', 1, 9, {})", curve("0 0, 10 10"))), "2FF46");
    assert_eq!(invoke_state(&a, &format!("ST_ADDLINK('N', 1, 3, {})", curve("0 1, 10 10"))), "2FF57");
    assert_eq!(invoke_state(&a, &format!("ST_ADDLINK('N', 1, 3, {})", curve("0 0, 10 11"))), "2FF33");
    assert_eq!(invoke_state(&a, "ST_NEWLOGLINKSPLIT('N', 9)"), "2FF45");
    assert_eq!(invoke_state(&a, "ST_MODLOGLINKSPLIT('N', 1)"), "2FF49");
    assert_eq!(invoke_state(&a, "ST_NEWGEOLINKSPLIT('N', 1, CAST(NULL AS ST_POINT))"), "2FF03");
    assert_eq!(invoke_state(&a, "ST_NEWGEOLINKSPLIT('N', 1, ST_POINT(5, 1))"), "2FF55");
    assert_eq!(invoke_state(&a, "ST_MODGEOLINKSPLIT('N', 1, ST_POINT(10, 0))"), "2FF55");
    assert_eq!(invoke_state(&a, "ST_NEWLINKHEAL('N', 1, 1)"), "2FF41");
    assert_eq!(invoke_state(&a, "ST_NEWLINKHEAL('N', 1, 9)"), "2FF45");
    assert_eq!(invoke_state(&a, "ST_ADDISONETNODE('N', ST_POINT(1, 1, 4326))"), "2FF10");
    invoked(&a, "ST_ADDISONETNODE('N', CAST(NULL AS ST_POINT))");
    assert_eq!(invoked(&a, "ST_ADDLINK('N', 4, 4, CAST(NULL AS ST_CURVE))"), "3");
    assert_eq!(state(&a, &format!("CALL ST_CHANGELINKGEOM('N', 3, {})", curve("0 0, 1 1"))), "2FF58");
    assert_eq!(state(&a, &format!("CALL ST_CHANGELINKGEOM('N', 9, {})", curve("0 0, 1 1"))), "2FF58");
    assert_eq!(invoke_state(&a, "ST_NEWGEOLINKSPLIT('N', 3, ST_POINT(0, 0))"), "2FF38");
    assert_eq!(invoked(&a, format!("ST_ADDLINK('N', 2, 1, {})", curve("10 0, 0 0"))), "4");
    assert_eq!(invoke_state(&a, "ST_NEWLINKHEAL('N', 1, 2)"), "2FF53");
    assert_eq!(state(&a, "CALL ST_MODLINKHEAL('N', 2, 3)"), "2FF41");
    assert_eq!(failed(&a, "SELECT ST_ADDISONETNODE('N', ST_POINT(1, 1)) FROM R").code, Code::SyntaxRule);
    assert_eq!(a.run("CALL ST_ADDLINK('N', 1, 2, CAST(NULL AS ST_CURVE))").unwrap_err().code, Code::SyntaxRule);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.1.1, §12.3
/// Description 2c] a user not granted SELECT on a network's views is
/// refused with 2FF65; one granted it edits the network by its routines,
/// which write the base tables the user may not.
#[test]
fn privileges_of_a_network() {
    let db = database();
    let a = network(&db);
    corner(&a);
    a.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE R (N INTEGER)").unwrap();
    let b = db.session("B");
    assert_eq!(invoke_state(&b, "ST_ADDISONETNODE('N', ST_POINT(5, 5))"), "2FF65");
    assert_eq!(read_state(&b, "SELECT * FROM TABLE(ST_VALIDSPATIALNET('N')) AS V"), "2FF65");
    a.run("GRANT SELECT ON \"N\".ST_NODE TO B").unwrap();
    a.run("GRANT SELECT ON \"N\".ST_LINK TO B").unwrap();
    a.commit().unwrap();
    assert_eq!(invoked(&b, "ST_ADDISONETNODE('N', ST_POINT(5, 5))"), "4");
    assert_eq!(b.run("INSERT INTO \"N\".ST_NET_NODE VALUES (9, ST_POINT(1, 1))").unwrap_err().code, Code::Privilege);
    b.commit().unwrap();
    assert_eq!(rows(&a, "SELECT ST_ASTEXT(GEOMETRY) FROM \"N\".ST_NODE WHERE NODE_ID = 4").unwrap(), ["POINT (5 5)"]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.16] a
/// collection made a network: each curve a link between nodes at its
/// ends, shared where ends meet, each point a node, crossings not noded;
/// a network not empty, or not there, is refused.
#[test]
fn a_network_is_made_of_a_collection() {
    let db = database();
    let a = network(&db);
    let g = "TREAT(ST_GEOMFROMTEXT('GEOMETRYCOLLECTION (LINESTRING (0 0, 10 10), LINESTRING (10 10, 20 0), \
             LINESTRING (0 10, 10 0), POINT (5 0), POINT (20 0))') AS ST_GEOMCOLLECTION)";
    a.run(&format!("CALL ST_SPATNETFROMGEOM('N', {g})")).unwrap();
    let nodes = "SELECT NODE_ID, ST_ASTEXT(GEOMETRY) FROM \"N\".ST_NODE ORDER BY NODE_ID";
    assert_eq!(
        rows(&a, nodes).unwrap(),
        ["1,POINT (0 0)", "2,POINT (10 10)", "3,POINT (20 0)", "4,POINT (0 10)", "5,POINT (10 0)", "6,POINT (5 0)"]
    );
    let ends = "SELECT LINK_ID, START_NODE, END_NODE FROM \"N\".ST_LINK ORDER BY LINK_ID";
    assert_eq!(rows(&a, ends).unwrap(), ["1,1,2", "2,2,3", "3,4,5"]);
    assert_eq!(rows(&a, "SELECT * FROM TABLE(ST_VALIDSPATIALNET('N')) AS V").unwrap(), Vec::<String>::new());
    assert_eq!(state(&a, &format!("CALL ST_SPATNETFROMGEOM('N', {g})")), "2FF42");
    assert_eq!(state(&a, &format!("CALL ST_SPATNETFROMGEOM('NOWHERE', {g})")), "2FF47");
    assert_eq!(state(&a, "CALL ST_SPATNETFROMGEOM('N', CAST(NULL AS ST_GEOMCOLLECTION))"), "2FF03");
    a.run("CALL ST_INITTOPONET('M')").unwrap();
    let area = "TREAT(ST_GEOMFROMTEXT('GEOMETRYCOLLECTION (POLYGON ((0 0, 1 0, 1 1, 0 0)))') AS ST_GEOMCOLLECTION)";
    assert_eq!(state(&a, &format!("CALL ST_SPATNETFROMGEOM('M', {area})")), "2FF02");
    a.commit().unwrap();
    db.schema("CREATE SCHEMA \"V\" AUTHORIZATION A CREATE VIEW ST_NODE AS SELECT 1 AS NODE_ID, ST_POINT(0, 0) AS GEOMETRY FROM A.R")
        .unwrap();
    assert_eq!(state(&a, &format!("CALL ST_SPATNETFROMGEOM('V', {g})")), "2FF48");
}

/// A topology `T` of §11.1 made by hand, of A: its views ST_NODE and
/// ST_EDGE over tables of two nodes and one edge between them.
fn topology(db: &Database, a: &Session) {
    a.commit().unwrap();
    db.schema(
        "CREATE SCHEMA \"T\" AUTHORIZATION A \
         CREATE TABLE NODES (NODE_ID INTEGER, CONTAINING_FACE INTEGER, GEOMETRY ST_POINT) \
         CREATE TABLE EDGES (EDGE_ID INTEGER, START_NODE INTEGER, END_NODE INTEGER, GEOMETRY ST_CURVE) \
         CREATE VIEW ST_NODE AS SELECT NODE_ID, CONTAINING_FACE, GEOMETRY FROM NODES \
         CREATE VIEW ST_EDGE AS SELECT EDGE_ID, START_NODE, END_NODE, GEOMETRY FROM EDGES",
    )
    .unwrap();
    a.run("INSERT INTO \"T\".NODES VALUES (3, NULL, ST_POINT(0, 0)), (7, NULL, ST_POINT(3, 4))").unwrap();
    a.run(&format!("INSERT INTO \"T\".EDGES VALUES (5, 3, 7, {})", curve("0 0, 3 4"))).unwrap();
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.14, §12.3.15]
/// a topology's nodes and edges made a network's nodes and links, of the
/// same identifiers: with no geometry in a logical network, with theirs in
/// a spatial one; the checks of what is not there, or not empty.
#[test]
fn networks_are_made_of_a_topology() {
    let db = database();
    let a = network(&db);
    topology(&db, &a);
    a.run("CALL ST_LOGINETFROMTGEO('N', 'T')").unwrap();
    assert_eq!(links(&a), ["5,3,7,NULL"]);
    assert_eq!(rows(&a, "SELECT NODE_ID, GEOMETRY FROM \"N\".ST_NODE ORDER BY NODE_ID").unwrap(), ["3,NULL", "7,NULL"]);
    assert_eq!(state(&a, "CALL ST_LOGINETFROMTGEO('N', 'T')"), "2FF42");
    a.run("CALL ST_INITTOPONET('S')").unwrap();
    a.run("CALL ST_SPATNETFROMTGEO('S', 'T')").unwrap();
    let spatial = "SELECT LINK_ID, START_NODE, END_NODE, ST_ASTEXT(GEOMETRY) FROM \"S\".ST_LINK";
    assert_eq!(rows(&a, spatial).unwrap(), ["5,3,7,LINESTRING (0 0, 3 4)"]);
    assert_eq!(rows(&a, "SELECT * FROM TABLE(ST_VALIDSPATIALNET('S')) AS V").unwrap(), Vec::<String>::new());
    assert_eq!(state(&a, "CALL ST_SPATNETFROMTGEO('S', CAST(NULL AS VARCHAR(1)))"), "2FF03");
    assert_eq!(state(&a, "CALL ST_SPATNETFROMTGEO('S', '')"), "2FF62");
    assert_eq!(state(&a, "CALL ST_SPATNETFROMTGEO('NOWHERE', 'T')"), "2FF47");
    assert_eq!(state(&a, "CALL ST_SPATNETFROMTGEO('S', 'NOWHERE')"), "2FF47");
    a.commit().unwrap();
    db.schema("CREATE SCHEMA \"U\" AUTHORIZATION A CREATE TABLE X (N INTEGER)").unwrap();
    assert_eq!(state(&a, "CALL ST_SPATNETFROMTGEO('S', 'U')"), "2FF48");
    assert_eq!(state(&a, "CALL ST_SPATNETFROMTGEO('U', 'T')"), "2FF48");
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE R (N INTEGER)").unwrap();
    a.run("GRANT SELECT ON \"S\".ST_NODE TO B").unwrap();
    a.run("GRANT SELECT ON \"S\".ST_LINK TO B").unwrap();
    a.commit().unwrap();
    assert_eq!(state(&db.session("B"), "CALL ST_SPATNETFROMTGEO('S', 'T')"), "2FF63");
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.18 Description
/// 2d ii 4 to 6] rows written by the schema's owner past the routines are
/// found: a curve whose ends are not its nodes', and geometries of two
/// systems.
#[test]
fn a_spatial_check_finds_mismatches() {
    let db = database();
    let a = network(&db);
    corner(&a);
    a.run("UPDATE \"N\".ST_NET_NODE SET GEOMETRY = ST_POINT(0, 1) WHERE NODE_ID = 1").unwrap();
    a.run("UPDATE \"N\".ST_NET_NODE SET GEOMETRY = ST_POINT(10, 10, 4326) WHERE NODE_ID = 3").unwrap();
    let found = rows(&a, "SELECT * FROM TABLE(ST_VALIDSPATIALNET('N')) AS V").unwrap();
    assert_eq!(found, ["geometry start mismatch,1,1", "geometry end mismatch,2,3", "mixed SRIDs,NULL,NULL"]);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.1, §12.3] a
/// network's schema, views and rows survive a checkpoint, a crash and a
/// reopening; an edit of a transaction that did not commit is not
/// recovered, and the network is edited again after.
#[test]
fn a_network_survives_recovery() {
    let scratch = Scratch::new("spatial-network-recovery");
    let dir = scratch.path("db");
    let open = || {
        let db = database();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    let a = network(&db);
    a.commit().unwrap();
    db.checkpoint().unwrap();
    corner(&a);
    a.commit().unwrap();
    invoked(&a, "ST_NEWGEOLINKSPLIT('N', 1, ST_POINT(4, 0))");
    let crashed = a;
    drop(db);
    let db = open();
    let a = db.session("A");
    assert_eq!(links(&a), ["1,1,2,LINESTRING (0 0, 10 0)", "2,2,3,LINESTRING (10 0, 10 10)"]);
    assert_eq!(invoked(&a, "ST_MODGEOLINKSPLIT('N', 1, ST_POINT(5, 0))"), "4");
    a.commit().unwrap();
    db.close().unwrap();
    drop((a, db, crashed));
    let db = open();
    let a = db.session("A");
    assert_eq!(links(&a)[0], "1,1,4,LINESTRING (0 0, 5 0)");
    assert_eq!(rows(&a, "SELECT * FROM TABLE(ST_VALIDSPATIALNET('N')) AS V").unwrap(), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3 Definitional
/// Rule 1, §20.5] ST_MaxNetworkName is the most characters of a name, a
/// name of so many taken and one of more refused as it is assigned to the
/// parameter of that length (22001).
#[test]
fn the_longest_network_name() {
    let db = database();
    let a = network(&db);
    let sizing = "SELECT SUPPORTED_VALUE FROM ST_INFORMTN_SCHEMA.ST_SIZINGS WHERE VARIABLE_NAME = 'ST_MaxNetworkName'";
    assert_eq!(rows(&a, sizing).unwrap(), ["128"]);
    a.run(&format!("CALL ST_INITTOPONET('{}')", "M".repeat(128))).unwrap();
    assert_eq!(state(&a, &format!("CALL ST_INITTOPONET('{}')", "M".repeat(129))), "22001");
}
