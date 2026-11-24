//! Default values of a procedure's input parameters (Features T522 and
//! T523) [ISO/IEC 9075-2:2023 §11.60 SR16e, §9.18 SR9, GR3b ii] (RULES.md
//! D979): a CALL that writes DEFAULT, leaves an argument out past those
//! written, or names none of it, takes the parameter's default value; the
//! procedure of fewest parameters that takes the arguments is invoked; a
//! default is checked, and what it names kept, as the procedure is
//! defined; and it is kept with the definitions and shown in
//! INFORMATION_SCHEMA.PARAMETERS.

use turborel::catalog::ParameterMode;
use turborel::db::{Database, Prepared, Session};
use turborel::params::Params;
use turborel::value::{DataType, Decimal, Spelled, Value};

use crate::common::Scratch;

/// Schema A: T (K, N) of rows (1, 10) and (2, 20), L (K, N, TAG) a log, M
/// (K) of rows 7 and 8; PUT (K, N DEFAULT 10 * 2, TAG DEFAULT 'none'),
/// which logs its arguments; and a session of A.
fn defined() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER) CREATE TABLE L (K INTEGER, N INTEGER, TAG VARCHAR(10)) \
         CREATE TABLE M (K INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    a.run("INSERT INTO M VALUES (7), (8)").unwrap();
    a.run(
        "CREATE PROCEDURE PUT (IN K INTEGER, IN N INTEGER DEFAULT 10 * 2, TAG VARCHAR(10) DEFAULT 'none') MODIFIES SQL DATA \
         INSERT INTO L VALUES (K, N, TAG)",
    )
    .unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The rows of `query` in `s`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// The log L, in order of K.
fn logged(s: &Session) -> Vec<String> {
    rows(s, "SELECT K, N, TAG FROM L ORDER BY K")
}

/// The SQLCODE and rule of `sql` run in `s`, which fails.
fn refused(s: &Session, sql: &str) -> (i32, String) {
    let e = s.run(sql).unwrap_err();
    (e.sqlcode(), e.rule.unwrap_or_default())
}

/// An INTEGER host parameter's type.
fn integer() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }
}

/// The exact number `n`.
fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 SR9b, SR9d, SR9h i, GR3b ii,
/// §10.4 CR6] RULES.md D979: an IN parameter takes its default value by
/// DEFAULT, by its argument left out past those written, or by its name
/// not named; NULL is the null value; DEFAULT of a parameter of no
/// default is the null value too. A CALL that leaves out, or names none
/// of, a parameter of no default, or names one the procedure lacks, or
/// writes more arguments than it has parameters, is refused (SQLCODE
/// -103).
#[test]
fn an_in_parameter_takes_its_default_9_18() {
    let (_db, a) = defined();
    for call in ["CALL PUT(1)", "CALL PUT(2, DEFAULT, 'x')", "CALL PUT(3, NULL)", "CALL PUT(TAG => 'y', K => 4)", "CALL PUT(K => 5)"] {
        a.run(call).unwrap();
    }
    a.run("CALL PUT(DEFAULT, 6)").unwrap();
    assert_eq!(logged(&a), ["1,20,none", "2,20,x", "3,NULL,none", "4,20,y", "5,20,none", "NULL,6,none"]);
    let rule = |r: &str| format!("ISO/IEC 9075-2:2023 §9.18 {r}");
    assert_eq!(refused(&a, "CALL PUT()"), (-103, rule("SR9b")), "K has no default");
    assert_eq!(refused(&a, "CALL PUT(1, 2, 'z', 4)"), (-103, rule("SR9b")), "more arguments than parameters");
    assert_eq!(refused(&a, "CALL PUT(N => 1)"), (-103, rule("SR9d")), "K named by none");
    assert_eq!(refused(&a, "CALL PUT(K => 1, Q => 2)"), (-103, rule("SR9d")), "no parameter Q");
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 SR9h iii 2, GR3b ii, §10.4 CR7]
/// RULES.md D979: an INOUT parameter of a default takes it by DEFAULT, by
/// its argument left out, or by its name not named, its value out then
/// assigned to no target; NULL is no target of it (SQLCODE -103), nor
/// may an OUT parameter, of no default, be left out.
#[test]
fn an_inout_parameter_takes_its_default_9_18() {
    let (_db, a) = defined();
    a.run(
        "CREATE PROCEDURE BUMP (OUT Y INTEGER, INOUT X INTEGER DEFAULT 5) READS SQL DATA \
         SELECT X + 1, X * 2 INTO Y, X FROM T WHERE K = 1",
    )
    .unwrap();
    a.commit().unwrap();
    let mut p = Params::new().declare("V", integer(), int(1)).declare("W", integer(), Value::Null);
    a.execute("CALL BUMP(W, V)", &mut p).unwrap();
    assert_eq!((p.value("W").to_string(), p.value("V").to_string()), ("2".to_owned(), "2".to_owned()));
    for call in ["CALL BUMP(W, DEFAULT)", "CALL BUMP(W)", "CALL BUMP(Y => W)"] {
        p = Params::new().declare("V", integer(), int(2)).declare("W", integer(), Value::Null);
        a.execute(call, &mut p).unwrap_or_else(|e| panic!("{call}: {e}"));
        assert_eq!(p.value("W").to_string(), "6", "{call}");
    }
    assert_eq!(p.value("V").to_string(), "2", "V is no target of these");
    let e = a.execute("CALL BUMP(W, NULL)", &mut p).unwrap_err();
    assert_eq!((e.sqlcode(), e.rule.as_deref()), (-103, Some("ISO/IEC 9075-2:2023 §9.18 SR9h iii 2")));
    assert_eq!(a.execute("CALL BUMP(X => V)", &mut p).unwrap_err().sqlcode(), -103, "Y has no default");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.60 SR16e i, ii, SR19, CR34]
/// RULES.md D979: a default is one its parameter's type takes by store
/// assignment, else refused as the procedure is defined (SQLCODE -103);
/// it sees none of the procedure's parameters; an OUT parameter has none
/// (-103); a function's parameter's is of Feature T525 (-900).
#[test]
fn a_default_is_checked_as_the_procedure_is_defined_11_60() {
    let (_db, a) = defined();
    let rule = |r: &str| format!("ISO/IEC 9075-2:2023 §11.60 {r}");
    let made = |head: &str| refused(&a, &format!("CREATE PROCEDURE P ({head}) CONTAINS SQL GET DIAGNOSTICS N = NUMBER"));
    assert_eq!(made("IN X INTEGER DEFAULT 'a', OUT N INTEGER"), (-103, rule("SR16e i, §9.2 SR2")));
    assert_eq!(made("INOUT N INTEGER DEFAULT DATE '2026-01-01'"), (-103, rule("SR16e i, §9.2 SR2")));
    assert_eq!(made("OUT N INTEGER DEFAULT 1").0, -103, "SR16e ii");
    assert!(a.run("CREATE PROCEDURE P (IN X INTEGER, IN Z INTEGER DEFAULT X) MODIFIES SQL DATA DELETE FROM L").is_err(), "no parameter X in scope");
    assert!(a.run("CREATE PROCEDURE P (IN X INTEGER DEFAULT (SELECT MAX(K) FROM NOWHERE)) MODIFIES SQL DATA DELETE FROM L").is_err());
    let function = refused(&a, "CREATE FUNCTION F (X INTEGER DEFAULT 1) RETURNS INTEGER CONTAINS SQL RETURN X");
    assert_eq!(function, (-900, rule("CR34")));
    a.run("CREATE PROCEDURE P (IN X SMALLINT DEFAULT 1.0, IN D DATE DEFAULT CURRENT_DATE) MODIFIES SQL DATA DELETE FROM L").unwrap();
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 GR3b ii 2, §11.31 SR7f, §11.23
/// SR7d, §11.62 SR4c ii] RULES.md D979: a default's subquery is bound as
/// the procedure's owner, so a caller of EXECUTE alone takes it; a table,
/// a column and a function it names are dropped only with CASCADE, which
/// drops the procedure.
#[test]
fn what_a_default_names_is_kept_with_the_procedure_11_31() {
    let (db, a) = defined();
    a.run("CREATE FUNCTION TWICE (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X * 2").unwrap();
    a.run("CREATE PROCEDURE LATEST (IN K INTEGER DEFAULT (SELECT MAX(K) FROM M)) MODIFIES SQL DATA INSERT INTO L VALUES (K, 0, 'm')")
        .unwrap();
    a.run("CREATE PROCEDURE DOUBLED (IN K INTEGER DEFAULT TWICE(21)) MODIFIES SQL DATA INSERT INTO L VALUES (K, 0, 'f')").unwrap();
    a.run("GRANT EXECUTE ON PROCEDURE LATEST TO B").unwrap();
    a.commit().unwrap();
    let b = db.session("B");
    b.run("CALL A.LATEST()").unwrap();
    b.commit().unwrap();
    a.run("CALL DOUBLED()").unwrap();
    assert_eq!(logged(&a), ["8,0,m", "42,0,f"]);
    assert_eq!(refused(&a, "DROP TABLE M RESTRICT").0, -103);
    assert_eq!(refused(&a, "ALTER TABLE M DROP COLUMN K RESTRICT").0, -103);
    assert_eq!(refused(&a, "DROP FUNCTION TWICE RESTRICT").0, -103);
    a.run("DROP FUNCTION TWICE CASCADE").unwrap();
    a.run("DROP TABLE M CASCADE").unwrap();
    a.commit().unwrap();
    let left = rows(&a, "SELECT ROUTINE_NAME FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA = 'A' ORDER BY ROUTINE_NAME");
    assert_eq!(left, ["PUT"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 SR9b, SR9e to SR9g, §11.60 SR20a]
/// RULES.md D979: of procedures of one name, a CALL invokes, of those that
/// take its arguments, the one of fewest parameters, by position or by
/// name; a procedure of as many parameters as another of its name is
/// refused, whatever their defaults (SQLCODE -103).
#[test]
fn a_call_invokes_the_procedure_of_fewest_parameters_9_18() {
    let (_db, a) = defined();
    a.run("CREATE PROCEDURE PICK (IN W INTEGER, OUT G INTEGER) SPECIFIC PICK_TWO CONTAINS SQL SET G = 2").unwrap();
    a.run(
        "CREATE PROCEDURE PICK (IN W INTEGER, OUT G INTEGER, IN B INTEGER DEFAULT 100) SPECIFIC PICK_THREE CONTAINS SQL SET G = 3",
    )
    .unwrap();
    a.commit().unwrap();
    let mut p = Params::new().declare("V", integer(), Value::Null);
    let mut picked = |call: &str| {
        a.execute(call, &mut p).unwrap();
        p.value("V").to_string()
    };
    assert_eq!(picked("CALL PICK(1, V)"), "2");
    assert_eq!(picked("CALL PICK(1, V, 5)"), "3");
    assert_eq!(picked("CALL PICK(G => V, W => 1)"), "2");
    assert_eq!(picked("CALL PICK(B => 2, W => 1, G => V)"), "3");
    let twin = refused(&a, "CREATE PROCEDURE PICK (IN W INTEGER DEFAULT 1, OUT G INTEGER) CONTAINS SQL SET G = 1");
    assert_eq!(twin, (-103, "ISO/IEC 9075-2:2023 §11.60 SR20a".to_owned()));
}

/// Each of `p`'s `?`s: its parameter's mode and ordinal position.
fn modes(p: &Prepared) -> Vec<(ParameterMode, Option<u32>)> {
    p.parameters().iter().map(|d| (d.mode, d.ordinal)).collect()
}

/// Verifies: [ISO/IEC 9075-2:2023 §20.10 GR7e, §9.18 SR9h i] RULES.md
/// D979: a prepared CALL that leaves defaulted parameters out describes
/// its `?`s by the ordinal positions of their parameters, and each
/// execution, the first bound and the later ones run straight, takes the
/// defaults.
#[test]
fn a_prepared_call_takes_its_defaults_20_10() {
    let (_db, a) = defined();
    let put = a.prepare("CALL PUT(?)").unwrap();
    let named = a.prepare("CALL PUT(TAG => ?, K => ?)").unwrap();
    let in_ = ParameterMode::In;
    assert_eq!(modes(&put), [(in_, Some(1))]);
    assert_eq!(modes(&named), [(in_, Some(3)), (in_, Some(1))]);
    for k in 1..=3 {
        a.execute_prepared(&put, &[int(k)]).unwrap();
    }
    a.execute_prepared(&named, &[Value::Char("n".into()), int(4)]).unwrap();
    a.execute_prepared(&named, &[Value::Char("m".into()), int(5)]).unwrap();
    assert_eq!(logged(&a), ["1,20,none", "2,20,none", "3,20,none", "4,20,n", "5,20,m"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §9.18 GR3b ii, §10.4 CR6] RULES.md D979:
/// a CALL in a compound statement, compiled with the procedure that holds
/// it, and one in a triggered action take defaults as a CALL run alone
/// does; a procedure written in Rust takes NULL and DEFAULT as the null
/// value, as its parameters have no default.
#[test]
fn a_compiled_call_takes_its_defaults_9_18() {
    let (_db, a) = defined();
    a.run("CREATE PROCEDURE BOTH_WAYS () MODIFIES SQL DATA BEGIN CALL PUT(1); CALL PUT(K => 2, TAG => 'in'); CALL PUT(3, DEFAULT); END")
        .unwrap();
    a.run("CREATE TRIGGER NOTED AFTER INSERT ON M FOR EACH ROW CALL PUT(9)").unwrap();
    a.commit().unwrap();
    a.run("CALL BOTH_WAYS()").unwrap();
    a.run("INSERT INTO M VALUES (1)").unwrap();
    assert_eq!(logged(&a), ["1,20,none", "2,20,in", "3,20,none", "9,20,none"]);
    let grid = "'GEOGCS[\"grid\",DATUM[\"d\",SPHEROID[\"s\",1,0]],PRIMEM[\"p\",0],UNIT[\"u\",1]]'";
    a.run(&format!("CALL ST_DEFINESRS('LOCAL GRID', 900001, NULL, DEFAULT, {grid}, NULL)")).unwrap();
    let defined = rows(&a, "SELECT AUTH_NAME, AUTH_SRID FROM SPATIAL_REF_SYS WHERE SRID = 900001");
    assert_eq!(defined, ["NULL,NULL"]);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.60 GR3c vi, §4.35.2] RULES.md D979:
/// INFORMATION_SCHEMA.PARAMETERS shows each default's text in
/// PARAMETER_DEFAULT, null of a parameter of none; a procedure's defaults
/// are kept with the definitions, after stopping without a checkpoint by
/// its logged definition and after one by the snapshot.
#[test]
fn defaults_are_shown_and_kept_across_reopening_11_60() {
    let scratch = Scratch::new("parameter-defaults");
    let dir = scratch.path("db");
    let shown = |a: &Session| {
        rows(
            a,
            "SELECT PARAMETER_NAME, PARAMETER_DEFAULT FROM INFORMATION_SCHEMA.PARAMETERS WHERE SPECIFIC_SCHEMA = 'A' \
             ORDER BY ORDINAL_POSITION",
        )
    };
    {
        let db = Database::open(&dir).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE L (K INTEGER, N INTEGER, TAG VARCHAR(10))").unwrap();
        let a = db.session("A");
        a.run("CREATE PROCEDURE KEPT (IN K INTEGER, IN N INTEGER DEFAULT 6 * 7, INOUT TAG VARCHAR(10) DEFAULT NULL) MODIFIES SQL DATA \
               INSERT INTO L VALUES (K, N, TAG)")
            .unwrap();
        a.commit().unwrap();
        assert_eq!(shown(&a), ["K,NULL", "N,6 * 7", "TAG,NULL"]);
    }
    for checkpoint in [false, true] {
        let db = Database::open(&dir).unwrap();
        let a = db.session("A");
        assert_eq!(shown(&a), ["K,NULL", "N,6 * 7", "TAG,NULL"], "checkpoint {checkpoint}");
        a.run("CALL KEPT(1)").unwrap();
        a.commit().unwrap();
        if checkpoint {
            db.checkpoint().unwrap();
        }
        drop(a);
        db.close().unwrap();
    }
    let db = Database::open(&dir).unwrap();
    let a = db.session("A");
    assert_eq!(rows(&a, "SELECT K, N, TAG FROM L"), ["1,42,NULL", "1,42,NULL"]);
    let null_default = rows(&a, "SELECT PARAMETER_DEFAULT IS NULL FROM INFORMATION_SCHEMA.PARAMETERS WHERE PARAMETER_NAME = 'TAG'");
    assert_eq!(null_default, ["FALSE"], "the text NULL, not the null value");
    drop(a);
    db.close().unwrap();
}
