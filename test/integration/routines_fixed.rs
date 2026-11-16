//! The subject routines of a definition are fixed as it is defined
//! [ISO/IEC 9075-2:2023 §4.35.2] (RULES.md D73): a view, a CHECK, a
//! generated column, a trigger, a routine's body, an assertion or a domain
//! constraint invokes the routine it invoked then, though a better overload
//! is made or EXECUTE on one is granted later, across reopening too; and
//! DROP ROUTINE and REVOKE find what invokes a routine by those routines.

use turborel::db::{Database, Session};
use turborel::error::{Code, Result};
use turborel::functions::ScalarFunction;
use turborel::value::{DataType, Decimal, INTEGER_PRECISION, Spelled, Value};

use crate::common::{Scratch, failure, function_calls, rows};

/// The function F of a DOUBLE PRECISION parameter, which returns 1.
const APPROXIMATE: &str = "CREATE FUNCTION F (X DOUBLE PRECISION) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN 1";

/// The function F of an INTEGER parameter, which returns 2: a better
/// subject routine than the DOUBLE PRECISION one for an exact argument
/// (RULES.md D74).
const EXACT: &str = "CREATE FUNCTION F (X INTEGER) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN 2";

/// A direct query of F of the INTEGER column K of T.
const DIRECT: &str = "SELECT F(K) FROM T";

/// Schema A: T (K INTEGER) of row 1, and F of a DOUBLE PRECISION
/// parameter alone, which returns 1.
fn approximate() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1)").unwrap();
    a.run(APPROXIMATE).unwrap();
    a.commit().unwrap();
    db
}

/// Creates F of an INTEGER parameter in `a` and commits, and checks a
/// new query of F of an INTEGER now invokes it.
fn overload(a: &Session) {
    a.run(EXACT).unwrap();
    a.commit().unwrap();
    assert_eq!(rows(a, DIRECT), Ok(vec!["2".to_owned()]), "a new statement invokes the INTEGER one");
}

/// The rows of `query` in `s`, which must succeed.
fn found(s: &Session, query: &str) -> Vec<String> {
    rows(s, query).unwrap()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.35.2, §11.32] a view invoking
/// F as only F (DOUBLE PRECISION) exists invokes it still after F
/// (INTEGER) is made, while a new query of the same text invokes F
/// (INTEGER).
#[test]
fn a_view_keeps_its_subject_routine_4_35_2() {
    let db = approximate();
    let a = db.session("A");
    a.run("CREATE VIEW V (R) AS SELECT F(K) FROM T").unwrap();
    a.commit().unwrap();
    assert_eq!(found(&a, "SELECT R FROM V"), ["1"]);
    overload(&a);
    assert_eq!(found(&a, "SELECT R FROM V"), ["1"], "the view keeps the DOUBLE PRECISION one");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.35.2, §11.9] a table CHECK invoking
/// F as only F (DOUBLE PRECISION) exists invokes it still after F
/// (INTEGER) is made, so a row it passed before passes still.
#[test]
fn a_check_keeps_its_subject_routine_4_35_2() {
    let db = approximate();
    let a = db.session("A");
    a.run("CREATE TABLE C (K INTEGER CHECK (F(K) = 1))").unwrap();
    a.run("INSERT INTO C VALUES (1)").unwrap();
    a.commit().unwrap();
    overload(&a);
    a.run("INSERT INTO C VALUES (2)").unwrap();
    a.commit().unwrap();
    assert_eq!(found(&a, "SELECT K FROM C ORDER BY K"), ["1", "2"]);
}

/// Verifies: [DESIGN] RULES.md D73 a generated column, as a check constraint
/// [ISO/IEC 9075-2:2023 §4.35.2], invoking F as only F (DOUBLE PRECISION)
/// exists invokes it still for rows stored after F (INTEGER) is made.
#[test]
fn a_generated_column_keeps_its_subject_routine_4_35_2() {
    let db = approximate();
    let a = db.session("A");
    a.run("CREATE TABLE G (K INTEGER, R INTEGER GENERATED ALWAYS AS (F(K)))").unwrap();
    a.commit().unwrap();
    overload(&a);
    a.run("INSERT INTO G (K) VALUES (1)").unwrap();
    a.commit().unwrap();
    assert_eq!(found(&a, "SELECT R FROM G"), ["1"], "the column keeps the DOUBLE PRECISION one");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.35.2, §11.49] a trigger's action
/// invoking F as only F (DOUBLE PRECISION) exists invokes it still when
/// fired after F (INTEGER) is made.
#[test]
fn a_trigger_keeps_its_subject_routine_4_35_2() {
    let db = approximate();
    let a = db.session("A");
    a.run("CREATE TABLE L (R INTEGER)").unwrap();
    a.run("CREATE TRIGGER W AFTER INSERT ON T REFERENCING NEW AS NR FOR EACH ROW INSERT INTO L VALUES (F(NR.K))").unwrap();
    a.commit().unwrap();
    overload(&a);
    a.run("INSERT INTO T VALUES (3)").unwrap();
    a.commit().unwrap();
    assert_eq!(found(&a, "SELECT R FROM L"), ["1"], "the trigger keeps the DOUBLE PRECISION one");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.35.2, §11.60] the body of an SQL
/// function invoking F as only F (DOUBLE PRECISION) exists invokes it
/// still after F (INTEGER) is made, though its argument is an INTEGER.
#[test]
fn a_routine_body_keeps_its_subject_routine_4_35_2() {
    let db = approximate();
    let a = db.session("A");
    a.run("CREATE FUNCTION G (X INTEGER) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN F(X)").unwrap();
    a.commit().unwrap();
    overload(&a);
    assert_eq!(found(&a, "SELECT G(K) FROM T"), ["1"], "the body keeps the DOUBLE PRECISION one");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.35.2, §11.47] an assertion invoking
/// F as only F (DOUBLE PRECISION) exists invokes it still after F
/// (INTEGER) is made, so a row it passed before passes still.
#[test]
fn an_assertion_keeps_its_subject_routine_4_35_2() {
    let db = approximate();
    let a = db.session("A");
    a.run("CREATE ASSERTION ONES CHECK (NOT EXISTS (SELECT * FROM T WHERE F(K) <> 1))").unwrap();
    a.commit().unwrap();
    overload(&a);
    a.run("INSERT INTO T VALUES (4)").unwrap();
    a.commit().unwrap();
    assert_eq!(found(&a, "SELECT K FROM T ORDER BY K"), ["1", "4"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.35.2, §11.34] a domain constraint
/// invoking F as only F (DOUBLE PRECISION) exists invokes it still after
/// F (INTEGER) is made, so a value of a column of the domain it passed
/// before passes still.
#[test]
fn a_domain_constraint_keeps_its_subject_routine_4_35_2() {
    let db = approximate();
    let a = db.session("A");
    a.run("CREATE DOMAIN D AS INTEGER CHECK (F(VALUE) = 1)").unwrap();
    a.run("CREATE TABLE M (K D)").unwrap();
    a.run("INSERT INTO M VALUES (1)").unwrap();
    a.commit().unwrap();
    overload(&a);
    a.run("INSERT INTO M VALUES (2)").unwrap();
    a.commit().unwrap();
    assert_eq!(found(&a, "SELECT K FROM M ORDER BY K"), ["1", "2"]);
}

/// Schema A of `approximate`, and schema B, owned by B, of M (K INTEGER)
/// of row 1, B holding EXECUTE on A.F (DOUBLE PRECISION) alone and its
/// view W invoking it.
fn granted() -> Database {
    let db = approximate();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE M (K INTEGER)").unwrap();
    let (a, b) = (db.session("A"), db.session("B"));
    a.run("GRANT EXECUTE ON FUNCTION F (DOUBLE PRECISION) TO B").unwrap();
    a.commit().unwrap();
    b.run("INSERT INTO M VALUES (1)").unwrap();
    b.run("CREATE VIEW W (R) AS SELECT A.F(K) FROM M").unwrap();
    b.commit().unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.35.2, §9.18 SR6] a view invoking a
/// function of another schema keeps its subject routine when EXECUTE on
/// a better one is granted to its owner later; a new query of its owner
/// chooses among the functions it may execute, F (INTEGER) once granted.
#[test]
fn a_later_grant_leaves_a_view_4_35_2() {
    let db = granted();
    let (a, b) = (db.session("A"), db.session("B"));
    a.run(EXACT).unwrap();
    a.commit().unwrap();
    assert_eq!(found(&b, "SELECT A.F(K) FROM M"), ["1"], "B holds no EXECUTE on the INTEGER one");
    a.run("GRANT EXECUTE ON FUNCTION F (INTEGER) TO B").unwrap();
    a.commit().unwrap();
    assert_eq!(found(&b, "SELECT A.F(K) FROM M"), ["2"], "a new query of B invokes the INTEGER one");
    assert_eq!(found(&b, "SELECT R FROM W"), ["1"], "the view keeps the DOUBLE PRECISION one");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.35.2, §11.60 GR3] a view's subject
/// routine is kept across reopening: after stopping without a checkpoint,
/// by its logged definition, and after a checkpoint, by the snapshot.
#[test]
fn subject_routines_are_kept_across_reopening_4_35_2() {
    let scratch = Scratch::new("routines-fixed-reopen");
    let dir = scratch.path("db");
    {
        let db = Database::open(&dir).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
        let a = db.session("A");
        a.run("INSERT INTO T VALUES (1)").unwrap();
        a.run(APPROXIMATE).unwrap();
        a.run("CREATE VIEW V (R) AS SELECT F(K) FROM T").unwrap();
        a.commit().unwrap();
        a.run(EXACT).unwrap();
        a.commit().unwrap();
    }
    {
        let db = Database::open(&dir).unwrap();
        let a = db.session("A");
        assert_eq!(found(&a, "SELECT R FROM V"), ["1"], "by the logged definition");
        assert_eq!(found(&a, DIRECT), ["2"]);
        db.checkpoint().unwrap();
        drop(a);
        db.close().unwrap();
    }
    let db = Database::open(&dir).unwrap();
    let a = db.session("A");
    assert_eq!(found(&a, "SELECT R FROM V"), ["1"], "by the snapshot");
    assert_eq!(found(&a, DIRECT), ["2"]);
    drop(a);
    db.close().unwrap();
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.35.2, §11.62 SR4c, GR1] DROP ROUTINE
/// finds what invokes a routine by the subject routines of each
/// definition: a view of F (DOUBLE PRECISION) does not keep F (INTEGER)
/// from being dropped with RESTRICT, and runs on; it keeps F (DOUBLE
/// PRECISION) from it, and goes with CASCADE.
#[test]
fn a_drop_finds_the_subject_routine_11_62() {
    let db = approximate();
    let a = db.session("A");
    a.run("CREATE VIEW V (R) AS SELECT F(K) FROM T").unwrap();
    a.commit().unwrap();
    overload(&a);
    a.run("CREATE VIEW U (R) AS SELECT F(K) FROM T").unwrap();
    a.commit().unwrap();
    assert_eq!(failure(a.run("DROP FUNCTION F (DOUBLE PRECISION) RESTRICT")).0, Code::SyntaxRule, "V invokes it");
    assert_eq!(failure(a.run("DROP FUNCTION F (INTEGER) RESTRICT")).0, Code::SyntaxRule, "U invokes it");
    a.run("DROP VIEW U RESTRICT").unwrap();
    a.run("DROP FUNCTION F (INTEGER) RESTRICT").unwrap();
    a.commit().unwrap();
    assert_eq!(found(&a, "SELECT R FROM V"), ["1"]);
    a.run("DROP FUNCTION F (DOUBLE PRECISION) CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT R FROM V"), Err(Code::SyntaxRule), "the view is gone");
}

/// A function registered as a plug-in, named F, of any argument, which
/// returns 9.
struct Nine;

impl ScalarFunction for Nine {
    fn name(&self) -> &'static str {
        "F"
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer })
    }

    fn call(&self, _args: &[Value]) -> Result<Value> {
        Ok(Value::Exact(Decimal::new(9, 0)))
    }
}

/// Verifies: [DESIGN] RULES.md D73, D53 a function registered as a plug-in of
/// the name of a view's subject routine, after the view is defined, never
/// takes its place, though a session of a schema of no function of the
/// name invokes it.
#[test]
fn a_registered_function_never_takes_over_4_35_2() {
    let db = approximate();
    let a = db.session("A");
    a.run("CREATE VIEW V (R) AS SELECT F(K) FROM T").unwrap();
    a.commit().unwrap();
    db.register_scalar_function(Box::new(Nine)).unwrap();
    function_calls(&a);
    assert_eq!(found(&a, "SELECT R FROM V"), ["1"]);
    assert_eq!(found(&a, DIRECT), ["1"], "the function of the schema is invoked");
    let b = db.session("B");
    function_calls(&b);
    assert_eq!(found(&b, "SELECT F(1) FROM (VALUES (0)) AS X (Z)"), ["9"], "schema B holds no F");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §4.35.2, §12.7 GR12o, SR3b] a revoke
/// of EXECUTE on a view's subject routine abandons the view: refused with
/// RESTRICT (2B000), and with CASCADE the view is dropped. A revoke of
/// EXECUTE on an overload the view does not invoke leaves it.
#[test]
fn a_revoke_abandons_the_subject_routines_invoker_12_7() {
    let db = granted();
    let (a, b) = (db.session("A"), db.session("B"));
    a.run(EXACT).unwrap();
    a.run("GRANT EXECUTE ON FUNCTION F (INTEGER) TO B").unwrap();
    a.commit().unwrap();
    a.run("REVOKE EXECUTE ON FUNCTION F (INTEGER) FROM B RESTRICT").unwrap();
    a.commit().unwrap();
    assert_eq!(found(&b, "SELECT R FROM W"), ["1"]);
    let restricted = failure(a.run("REVOKE EXECUTE ON FUNCTION F (DOUBLE PRECISION) FROM B RESTRICT"));
    assert_eq!(restricted, (Code::SyntaxRule, "2B000".to_owned()));
    a.run("REVOKE EXECUTE ON FUNCTION F (DOUBLE PRECISION) FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&b, "SELECT R FROM W"), Err(Code::SyntaxRule), "the view is gone");
}
