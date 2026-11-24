//! Output dynamic parameters of a prepared CALL [ISO/IEC 9075-2:2023
//! §20.7 GR7a, SR5a, §20.10, §20.13 GR6, GR10]: a `?` as the argument of
//! an output parameter, or of one both input and output, of the
//! parameter's type; described as the statement's output, given back by
//! an execution, and put where EXECUTE's `<result using clause>` says.

use turborel::db::{Completion, Database, Session};
use turborel::value::{Decimal, Value};

use crate::common::{dynamic_host, run_with as run, set_text as text};

/// Schema A: T (K, N) of rows (1, 10) and (2, 20); FIND (IN, OUT), BUMP
/// (INOUT) and MIXED (OUT, IN, INOUT); and a session of A.
fn defined() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER, N INTEGER)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    a.run("CREATE PROCEDURE FIND (IN WANTED INTEGER, OUT GOT INTEGER) READS SQL DATA SELECT N INTO GOT FROM T WHERE K = WANTED").unwrap();
    a.run("CREATE PROCEDURE BUMP (INOUT X SMALLINT) READS SQL DATA SELECT X + MAX(N) INTO X FROM T").unwrap();
    a.run(
        "CREATE PROCEDURE MIXED (OUT TOTAL INTEGER, IN TIMES INTEGER, INOUT TAG VARCHAR(10)) READS SQL DATA \
         SELECT SUM(N) * TIMES, TAG || 'X' INTO TOTAL, TAG FROM T",
    )
    .unwrap();
    a.commit().unwrap();
    (db, a)
}

/// The exact number `n`.
fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The values `values` as text.
fn shown(values: &[Value]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

/// Verifies: [ISO/IEC 9075-2:2023 §20.7 GR7a, SR5a, §20.10 GR5, GR6] a
/// prepared CALL takes `?` as the argument of an OUT or INOUT parameter:
/// of its type, an input of an INOUT one, its input parameters described
/// without the OUT ones'; it is described with a column for each output,
/// named and typed as its parameter, in the parameters' order; executed,
/// it gives back their values, null of a row not found.
#[test]
fn a_prepared_call_gives_back_its_outputs_20_7() {
    let (_db, a) = defined();
    let find = a.prepare("CALL FIND(?, ?)").unwrap();
    assert!(!find.is_query() && find.has_outputs());
    assert_eq!(find.parameters().len(), 1, "the OUT parameter's ? is no input");
    let columns: Vec<String> = find.columns().iter().map(|c| format!("{} {}", c.name.as_deref().unwrap_or(""), c.data_type)).collect();
    assert_eq!(columns, ["GOT INTEGER"]);
    let (done, out) = a.execute_prepared_out(&find, &[int(2)]).unwrap();
    assert_eq!((done, shown(&out)), (Completion::Success, vec!["20".to_owned()]));
    assert_eq!(shown(&a.execute_prepared_out(&find, &[int(9)]).unwrap().1), ["NULL"], "no row");
    let bump = a.prepare("CALL BUMP((?))").unwrap();
    assert_eq!(bump.parameters().len(), 1, "an INOUT parameter's ? is an input too");
    assert_eq!(shown(&a.execute_prepared_out(&bump, &[int(5)]).unwrap().1), ["25"]);
    let both = a.prepare("CALL MIXED(?, ?, ?)").unwrap();
    assert_eq!(both.parameters().len(), 2);
    assert_eq!(both.columns().iter().map(|c| c.name.clone().unwrap_or_default()).collect::<Vec<_>>(), ["TOTAL", "TAG"]);
    let out = a.execute_prepared_out(&both, &[int(2), Value::Char("A".into())]).unwrap().1;
    assert_eq!(shown(&out), ["60", "AX"]);
    assert_eq!(a.execute_prepared(&find, &[int(1)]).unwrap(), Completion::Success, "the outputs may be left");
    let named = a.prepare("CALL FIND(GOT => ?, WANTED => 1)").unwrap();
    assert_eq!(shown(&a.execute_prepared_out(&named, &[]).unwrap().1), ["10"]);
    assert!(a.prepare("CALL FIND(?, ? + 1)").is_err(), "§9.18 SR9h iii 1: an expression is no target");
}

/// Verifies: [ISO/IEC 9075-2:2023 §20.13 GR6, GR10, §20.10, §20.14 GR3]
/// EXECUTE of a CALL of output dynamic parameters puts their values where
/// its `<result using clause>` says, SQLSTATE 07007 if it has none; a
/// statement of none takes a result using clause of no target; DESCRIBE
/// OUTPUT describes the outputs and DESCRIBE INPUT the inputs; EXECUTE
/// IMMEDIATE takes no statement of output dynamic parameters (42000).
#[test]
fn execute_puts_the_outputs_where_its_result_clause_says_20_13() {
    let (_db, a) = defined();
    let mut p = dynamic_host();
    text(&mut p, "CALL FIND(?, ?)");
    run(&a, "PREPARE C FROM TEXT", &mut p).unwrap();
    assert_eq!(run(&a, "EXECUTE C INTO VAL USING K", &mut p), Ok(Completion::Success));
    assert_eq!(p.value("VAL").to_string(), "10");
    assert_eq!(run(&a, "EXECUTE C USING K", &mut p), Err("07007".to_owned()));
    run(&a, "ALLOCATE DESCRIPTOR 'D'", &mut p).unwrap();
    run(&a, "DESCRIBE OUTPUT C USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    run(&a, "GET DESCRIPTOR 'D' N = COUNT", &mut p).unwrap();
    run(&a, "GET DESCRIPTOR 'D' VALUE 1 SN = NAME", &mut p).unwrap();
    assert_eq!([p.value("N").to_string(), p.value("SN").to_string()], ["1", "GOT"]);
    run(&a, "DESCRIBE INPUT C USING SQL DESCRIPTOR 'D'", &mut p).unwrap();
    run(&a, "GET DESCRIPTOR 'D' N = COUNT", &mut p).unwrap();
    assert_eq!(p.value("N").to_string(), "1");
    text(&mut p, "INSERT INTO T VALUES (3, 30)");
    run(&a, "PREPARE I FROM TEXT", &mut p).unwrap();
    assert_eq!(run(&a, "EXECUTE I INTO VAL", &mut p), Err("07002".to_owned()), "§20.12: a target for no output");
    text(&mut p, "CALL FIND(1, ?)");
    assert_eq!(run(&a, "EXECUTE IMMEDIATE TEXT", &mut p), Err("42000".to_owned()));
}
