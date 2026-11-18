//! The spatial reference systems and geometry columns a database defines
//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §19, §20], by the procedures
//! built in, in a transaction and through recovery; and the routines
//! written in Rust that read and write tables: table functions invoked as
//! `TABLE(f(args))` and procedures invoked by CALL (RULES.md
//! D-SPATIAL-CATALOG, D-SPATIAL-ROUTINES).

use turborel::db::{Database, OpenOptions};
use turborel::error::{Code, Result};
use turborel::functions::builtin::{Procedure, ProcedureContext, TableFunction};
use turborel::value::{DataType, Value};

use crate::common::{Scratch, after_crash, rows};

fn database() -> Database {
    let db = Database::new();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    db
}

const DEFINE: &str = "CALL ST_DEFINESRS('LOCAL GRID', 900001, 'ACME', 7, 'GEOGCS[\"grid\",DATUM[\"d\",SPHEROID[\"s\",1,0]],PRIMEM[\"p\",0],UNIT[\"u\",1]]', 'a site grid')";
const SYSTEMS: &str = "SELECT SRS_NAME, ORGANIZATION, ORGANIZATION_COORDSYS_ID, DESCRIPTION FROM ST_INFORMTN_SCHEMA.ST_SPATIAL_REFERENCE_SYSTEMS WHERE SRS_ID > 900000";
const COLUMNS: &str = "SELECT COLUMN_NAME, SRS_NAME, SRS_ID FROM ST_INFORMTN_SCHEMA.ST_GEOMETRY_COLUMNS ORDER BY COLUMN_NAME";

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.2, §20.3, §19.2,
/// §19.3] a system defined and a column registered are rows of the views,
/// undone by ROLLBACK; each constraint of the tables is kept: the name and
/// the identifier unique, ORGANIZATION and its identifier together and
/// unique, a registration's system defined and its column of a geometry
/// type, of the system's SRID; a system a column names is not dropped,
/// nor one supplied.
#[test]
fn systems_and_registrations_keep_their_tables_constraints() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, G ST_GEOMETRY, P ST_POINT(4326))").unwrap();
    let a = db.session("A");
    a.run(DEFINE).unwrap();
    a.rollback().unwrap();
    assert_eq!(rows(&a, SYSTEMS).unwrap(), Vec::<String>::new());
    a.run(DEFINE).unwrap();
    assert_eq!(rows(&a, SYSTEMS).unwrap(), ["LOCAL GRID,ACME,7,a site grid"]);
    a.commit().unwrap();
    let code = |sql: &str| a.run(sql).unwrap_err().code;
    assert_eq!(code(DEFINE), Code::Constraint);
    assert_eq!(
        code(
            "CALL ST_DEFINESRS('OTHER', 900001, CAST(NULL AS VARCHAR(1)), CAST(NULL AS INTEGER), 'GEOGCS[\"x\",DATUM[\"d\",SPHEROID[\"s\",1,0]],PRIMEM[\"p\",0],UNIT[\"u\",1]]', 'x')"
        ),
        Code::Constraint
    );
    assert_eq!(
        code(
            "CALL ST_DEFINESRS('OTHER', 900002, 'ACME', CAST(NULL AS INTEGER), 'GEOGCS[\"x\",DATUM[\"d\",SPHEROID[\"s\",1,0]],PRIMEM[\"p\",0],UNIT[\"u\",1]]', 'x')"
        ),
        Code::Constraint
    );
    assert_eq!(
        code(
            "CALL ST_DEFINESRS('OTHER', 900002, 'EPSG', 4326, 'GEOGCS[\"x\",DATUM[\"d\",SPHEROID[\"s\",1,0]],PRIMEM[\"p\",0],UNIT[\"u\",1]]', 'x')"
        ),
        Code::Constraint
    );
    assert_eq!(
        code(
            "CALL ST_DEFINESRS('WGS 84', 900002, 'X', 1, 'GEOGCS[\"x\",DATUM[\"d\",SPHEROID[\"s\",1,0]],PRIMEM[\"p\",0],UNIT[\"u\",1]]', 'x')"
        ),
        Code::Constraint
    );
    assert_eq!(code("CALL ST_DEFINESRS('OTHER', 900002)"), Code::SyntaxRule);
    a.run("CALL ST_REGISTERGEOMETRYCOLUMN('A', 'T', 'G', 'LOCAL GRID')").unwrap();
    assert_eq!(rows(&a, COLUMNS).unwrap(), ["G,LOCAL GRID,900001", "P,WGS 84,4326"]);
    assert_eq!(code("CALL ST_REGISTERGEOMETRYCOLUMN('A', 'T', 'ID', 'LOCAL GRID')"), Code::SyntaxRule);
    assert_eq!(code("CALL ST_REGISTERGEOMETRYCOLUMN('A', 'T', 'X', 'LOCAL GRID')"), Code::SyntaxRule);
    assert_eq!(code("CALL ST_REGISTERGEOMETRYCOLUMN('A', 'T', 'P', 'LOCAL GRID')"), Code::SyntaxRule);
    assert_eq!(code("CALL ST_REGISTERGEOMETRYCOLUMN('A', 'T', 'G', 'NOWHERE')"), Code::Constraint);
    assert_eq!(code("CALL ST_DROPSRS('LOCAL GRID')"), Code::Constraint);
    assert_eq!(code("CALL ST_DROPSRS('WGS 84')"), Code::SyntaxRule);
    a.run("CALL ST_UNREGISTERGEOMETRYCOLUMN('A', 'T', 'G')").unwrap();
    a.run("CALL ST_DROPSRS('LOCAL GRID')").unwrap();
    assert_eq!(rows(&a, COLUMNS).unwrap(), ["G,NULL,NULL", "P,WGS 84,4326"]);
    a.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE S (G ST_GEOMETRY)").unwrap();
    assert_eq!(a.run("CALL ST_REGISTERGEOMETRYCOLUMN('B', 'S', 'G', 'WGS 84')").unwrap_err().code, Code::Privilege);
}

/// Verifies: [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.2, §20.3] the
/// systems defined and the columns registered survive a checkpoint, a
/// crash and a reopening; a registration goes with its column dropped;
/// one of a transaction that did not commit is not recovered.
#[test]
fn systems_and_registrations_survive_recovery() {
    let scratch = Scratch::new("spatial-catalog-recovery");
    let dir = scratch.path("db");
    let open = || {
        let db = database();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, G ST_GEOMETRY, H ST_GEOMETRY)").unwrap();
    let a = db.session("A");
    a.run(DEFINE).unwrap();
    a.commit().unwrap();
    db.checkpoint().unwrap();
    a.run("CALL ST_REGISTERGEOMETRYCOLUMN('A', 'T', 'G', 'LOCAL GRID')").unwrap();
    a.run("CALL ST_REGISTERGEOMETRYCOLUMN('A', 'T', 'H', 'WGS 84')").unwrap();
    a.commit().unwrap();
    a.run("ALTER TABLE T DROP COLUMN H RESTRICT").unwrap();
    a.commit().unwrap();
    a.run("CALL ST_DEFINESRS('LOST', 900009, 'ACME', 9, 'GEOGCS[\"lost\",DATUM[\"d\",SPHEROID[\"s\",1,0]],PRIMEM[\"p\",0],UNIT[\"u\",1]]', 'never committed')").unwrap();
    let crashed = a;
    drop(db);
    let db = open();
    let a = db.session("A");
    assert_eq!(rows(&a, SYSTEMS).unwrap(), ["LOCAL GRID,ACME,7,a site grid"]);
    assert_eq!(rows(&a, COLUMNS).unwrap(), ["G,LOCAL GRID,900001"]);
    a.commit().unwrap();
    db.checkpoint().unwrap();
    db.close().unwrap();
    drop((a, db, crashed));
    let db = open();
    assert_eq!(rows(&db.session("A"), COLUMNS).unwrap(), ["G,LOCAL GRID,900001"]);
}

/// A procedure that copies the rows of one table it reads into another,
/// as a topology's routines write their tables.
#[derive(Debug)]
struct Copy;

impl Procedure for Copy {
    fn name(&self) -> &'static str {
        "COPY_ABOVE"
    }
    fn parameters(&self, count: usize) -> Option<Vec<DataType>> {
        (count == 1).then(|| vec![DataType::Exact { precision: 10, scale: 0, spelled: turborel::value::Spelled::Integer }])
    }
    fn call(&self, args: &[Value], cx: &mut dyn ProcedureContext) -> Result<()> {
        for row in cx.rows("SELECT ID FROM A.T WHERE ID > ?", args)? {
            cx.run("INSERT INTO A.U VALUES (?)", &row)?;
        }
        Ok(())
    }
}

/// A table function of the pairs of a table's rows and their doubles,
/// reading the table its argument names.
#[derive(Debug)]
struct Doubled;

impl TableFunction for Doubled {
    fn name(&self) -> &'static str {
        "DOUBLED"
    }
    fn parameters(&self, count: usize) -> Option<Vec<DataType>> {
        (count == 1).then(|| vec![DataType::varying(128)])
    }
    fn columns(&self) -> Vec<(String, DataType)> {
        let int = DataType::Exact { precision: 10, scale: 0, spelled: turborel::value::Spelled::Integer };
        vec![("N".into(), int.clone()), ("TWICE".into(), int)]
    }
    fn reads(&self, known: &[Option<&Value>]) -> Result<Vec<String>> {
        let [Some(Value::Char(table))] = known else {
            return Err(turborel::error::SqlError::new(Code::SyntaxRule, "DOUBLED names its table by a literal"));
        };
        Ok(vec![format!("SELECT ID FROM {table}")])
    }
    fn rows(&self, _: &[Value], read: &[Vec<Vec<Value>>]) -> Result<Vec<Vec<Value>>> {
        let ints = read.first().into_iter().flatten().filter_map(|r| r.first()?.as_f64());
        let int = |v: f64| Value::Exact(turborel::value::Decimal::new(v as i128, 0));
        Ok(ints.map(|n| vec![int(n), int(n * 2.0)]).collect())
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §15.1, §9.18 GR3, GR4] a procedure
/// written in Rust reads and writes tables in its caller's transaction,
/// undone by ROLLBACK, and by the CALL that fails; [§7.6 SR8] a table
/// function written in Rust reads the table its argument names, with the
/// privileges of the statement's user, laterally of the rows before it.
#[test]
fn rust_routines_read_and_write_tables() {
    let db = database();
    db.register_procedure(Box::new(Copy)).unwrap();
    db.register_table_function(Box::new(Doubled)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER) CREATE TABLE U (ID INTEGER PRIMARY KEY)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1), (2), (3)").unwrap();
    a.commit().unwrap();
    a.run("CALL COPY_ABOVE(1)").unwrap();
    assert_eq!(rows(&a, "SELECT ID FROM U ORDER BY ID").unwrap(), ["2", "3"]);
    a.rollback().unwrap();
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM U").unwrap(), ["0"]);
    a.run("INSERT INTO U VALUES (3)").unwrap();
    assert_eq!(a.run("CALL COPY_ABOVE(1)").unwrap_err().code, Code::Constraint);
    assert_eq!(rows(&a, "SELECT ID FROM U").unwrap(), ["3"]);
    let call = a.prepare("CALL COPY_ABOVE(?)").unwrap();
    a.execute_prepared(&call, &[Value::Exact(turborel::value::Decimal::new(3, 0))]).unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT N, TWICE FROM TABLE(DOUBLED('A.T')) AS D ORDER BY N").unwrap(), ["1,2", "2,4", "3,6"]);
    let lateral = "SELECT U.ID, D.TWICE FROM U, TABLE(DOUBLED('A.U')) AS D (N, TWICE) WHERE D.N = U.ID";
    assert_eq!(rows(&a, lateral).unwrap(), ["3,6"]);
    assert_eq!(rows(&a, "SELECT * FROM TABLE(DOUBLED(CAST(? AS VARCHAR(9)))) AS D").unwrap_err(), Code::SyntaxRule);
    assert_eq!(rows(&a, "SELECT * FROM TABLE(DOUBLED('A.T', 1)) AS D").unwrap_err(), Code::SyntaxRule);
    let b = db.session("B");
    assert_eq!(rows(&b, "SELECT * FROM TABLE(DOUBLED('A.T')) AS D").unwrap_err(), Code::Privilege);
    assert_eq!(db.register_table_function(Box::new(Doubled)).unwrap_err().code, Code::Api);
}
