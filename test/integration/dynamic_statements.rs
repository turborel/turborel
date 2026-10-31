//! Dynamic SQL in SQL text [ISO/IEC 9075:1992 §17]: PREPARE, EXECUTE,
//! EXECUTE IMMEDIATE, DEALLOCATE PREPARE and DESCRIBE, run by a session
//! with its program's parameters (RULES.md D57); the names of prepared
//! statements [§5.4], and what each statement refuses.

use turborel::db::{Completion, Session};
use turborel::error::Code;
use turborel::value::Value;

use crate::common::{code_with as code, dynamic_host, number, rows, run_with as run, set_text as text, setup};

/// The DIAGNOSTICS item `item`, text, of the last statement.
fn diagnostic(s: &Session, item: &str) -> String {
    let mut p = dynamic_host();
    s.execute(&format!("GET DIAGNOSTICS TXT = {item}"), &mut p).unwrap();
    p.value("TXT").to_string().trim_end().to_owned()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.6 GR7, §17.10 GR6, GR8,
/// §17.9 GR5, §18.1 GR1c, GR1d] PREPARE prepares the statement its
/// variable holds, by a `<statement name>`; EXECUTE runs it with the
/// values of its USING arguments, each cast to its parameter's type; the
/// diagnostics area names EXECUTE, and INSERT as the statement prepared.
#[test]
fn a_statement_prepared_by_name_is_executed_17_6_17_10() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    text(&mut p, "INSERT INTO T (ID, NAME) VALUES (?, ?)");
    assert_eq!(run(&s, "PREPARE S1 FROM TEXT", &mut p), Ok(Completion::Success));
    assert_eq!(diagnostic(&s, "COMMAND_FUNCTION"), "PREPARE");
    p.set("ID", number(7));
    p.set("NM", Value::Char("EVE".to_owned()));
    assert_eq!(run(&s, "EXECUTE S1 USING ID, NM", &mut p), Ok(Completion::Success));
    assert_eq!((diagnostic(&s, "COMMAND_FUNCTION"), diagnostic(&s, "DYNAMIC_FUNCTION")), ("EXECUTE".to_owned(), "INSERT".to_owned()));
    p.set("ID", number(8));
    p.set("NI", number(-1));
    assert_eq!(run(&s, "EXECUTE S1 USING ID, NM INDICATOR NI", &mut p), Ok(Completion::Success), "a null value");
    assert_eq!(rows(&s, "SELECT ID, NAME FROM T WHERE ID > 6 ORDER BY ID").unwrap(), ["7,EVE   ", "8,NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.4 SR13, SR17, GR12, GR13,
/// §17.6 GR6, GR8] an extended statement name is its value without
/// leading or trailing spaces, LOCAL if no scope is written, and LOCAL and
/// GLOBAL names are apart; one prepared again replaces the first; a value
/// that is no `<identifier>` is an invalid SQL statement name (26000); a
/// literal is no extended name (SQLCODE -103).
#[test]
fn extended_statement_names_5_4_17_6() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    p.set("SN", Value::Char("  S2 ".to_owned()));
    text(&mut p, "DELETE FROM T WHERE ID = 3");
    assert_eq!(run(&s, "PREPARE GLOBAL SN FROM TEXT", &mut p), Ok(Completion::Success));
    assert_eq!(run(&s, "EXECUTE SN", &mut p), Err("26000"), "LOCAL is implicit, and apart");
    assert_eq!(run(&s, "EXECUTE LOCAL SN", &mut p), Err("26000"));
    text(&mut p, "DELETE FROM T WHERE ID = 2");
    assert_eq!(run(&s, "PREPARE GLOBAL SN FROM TEXT", &mut p), Ok(Completion::Success), "GR6: the first deallocated");
    assert_eq!(run(&s, "EXECUTE GLOBAL SN", &mut p), Ok(Completion::Success));
    assert_eq!(rows(&s, "SELECT ID FROM T ORDER BY ID").unwrap(), ["1", "3"]);
    p.set("SN", Value::Char("NOT ONE".to_owned()));
    assert_eq!(run(&s, "PREPARE SN FROM TEXT", &mut p), Err("26000"), "GR8");
    assert_eq!(code(&s, "EXECUTE GLOBAL 'S2'", &mut p), Code::SyntaxRule, "§5.4 SR13");
    p.set("SN", Value::Char("S2".to_owned()));
    assert_eq!(run(&s, "EXECUTE GLOBAL SN", &mut p), Ok(Completion::NoData), "the name trimmed is S2's: no row to delete");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.6 SR1, SR2, GR4, GR4q,
/// §17.7 GR1, §17.10 GR1, GR4, §17.9 GR5a, GR5e] what PREPARE, EXECUTE
/// and DEALLOCATE PREPARE refuse: a literal for the statement (SQLCODE
/// -103) or a variable of no character string (-103); a statement that is
/// not preparable, or has a comment (37000); one not prepared (26000);
/// dynamic parameters with no using clause (07004), or arguments not one
/// for each (07001), or of a type not cast to its (07006).
#[test]
fn what_prepare_and_execute_refuse_17_6_17_10() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    assert_eq!(code(&s, "PREPARE S FROM 'DELETE FROM T'", &mut p), Code::SyntaxRule, "SR1");
    assert_eq!(code(&s, "PREPARE S FROM K", &mut p), Code::SyntaxRule, "SR2");
    for refused in ["COMMIT AND CHAIN", "PREPARE X FROM TEXT", "GET DIAGNOSTICS N = NUMBER", "DELETE FROM T -- all\n"] {
        text(&mut p, refused);
        assert_eq!(run(&s, "PREPARE S FROM TEXT", &mut p), Err("37000"), "{refused}");
    }
    assert_eq!(run(&s, "EXECUTE S", &mut p), Err("26000"), "§17.10 GR1");
    assert_eq!(run(&s, "DEALLOCATE PREPARE S", &mut p), Err("26000"), "§17.7 GR1");
    text(&mut p, "UPDATE T SET AMT = ? WHERE ID = ?");
    run(&s, "PREPARE S FROM TEXT", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE S", &mut p), Err("07004"), "§17.10 GR4");
    assert_eq!(run(&s, "EXECUTE S USING K", &mut p), Err("07001"), "§17.9 GR5a");
    p.set("TXT", Value::Char("x".to_owned()));
    assert_eq!(run(&s, "EXECUTE S USING TXT, K", &mut p), Err("22018"), "GR5f: CAST's own");
    text(&mut p, "SELECT ID FROM T WHERE CAST(? AS DATE) IS NOT NULL");
    run(&s, "PREPARE D FROM TEXT", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE D INTO N USING K", &mut p), Err("07006"), "§17.9 GR5e: no INTEGER is a DATE");
    assert_eq!(run(&s, "DEALLOCATE PREPARE S", &mut p), Ok(Completion::Success));
    assert_eq!(run(&s, "EXECUTE S USING K, K", &mut p), Err("26000"), "deallocated");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.9 SR1, §17.10 SR2, §17.8
/// Format] a `<result using clause>` is of INTO, and comes first; a
/// `<parameter using clause>`, OPEN's and DESCRIBE's are of USING (SQLCODE
/// -103); DESCRIBE takes a descriptor alone (-102).
#[test]
fn using_and_into_where_they_stand_17_9() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    for broken in [
        "EXECUTE S USING K INTO N",
        "EXECUTE S USING K USING N",
        "EXECUTE S INTO N INTO K",
        "DESCRIBE S INTO SQL DESCRIPTOR DN",
        "OPEN C INTO K",
        "FETCH C USING N",
        "FETCH GLOBAL CN USING N",
    ] {
        assert_eq!(code(&s, broken, &mut p), Code::SyntaxRule, "{broken}");
    }
    assert_eq!(code(&s, "DESCRIBE S USING K", &mut p), Code::Syntax);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.10 GR2, GR5, GR7, GR8a,
/// §13.5 GR2] EXECUTE of a `<dynamic single row select statement>` puts
/// its row into the arguments of its result using clause; none, no data;
/// more than one, a cardinality violation (21000); one with none, 07007; a
/// `<dynamic select statement>` of more than a `<query specification>`
/// cannot be executed (07003); the diagnostics area names SELECT.
#[test]
fn a_single_row_select_executed_into_arguments_17_10() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    text(&mut p, "SELECT ID, NAME FROM T WHERE ID >= ?");
    run(&s, "PREPARE Q FROM TEXT", &mut p).unwrap();
    p.set("K", number(2));
    assert_eq!(run(&s, "EXECUTE Q INTO N, NM INDICATOR NI USING K", &mut p), Err("21000"));
    p.set("K", number(3));
    assert_eq!(run(&s, "EXECUTE Q INTO N, NM INDICATOR NI USING K", &mut p), Ok(Completion::Success));
    assert_eq!((p.value("N").to_string(), p.indicator_value("NI")), ("3".to_owned(), Some(-1)));
    assert_eq!(diagnostic(&s, "DYNAMIC_FUNCTION"), "SELECT");
    p.set("K", number(9));
    assert_eq!(run(&s, "EXECUTE Q INTO N, NM INDICATOR NI USING K", &mut p), Ok(Completion::NoData));
    assert_eq!(run(&s, "EXECUTE Q USING K", &mut p), Err("07007"), "GR5");
    assert_eq!(run(&s, "EXECUTE Q INTO N USING K", &mut p), Err("07002"), "§17.9 GR6a");
    text(&mut p, "SELECT ID FROM T ORDER BY ID");
    run(&s, "PREPARE O FROM TEXT", &mut p).unwrap();
    assert_eq!(run(&s, "EXECUTE O INTO N", &mut p), Err("07003"), "GR2");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.11 GR3, GR4, §18.1 GR1c,
/// GR1d, §4.22.6] EXECUTE IMMEDIATE prepares and executes the statement,
/// a `<literal>` too, in direct SQL as well; the diagnostics area names
/// EXECUTE IMMEDIATE and the statement; it refuses a query, a dynamic
/// parameter and a comment (37000); a statement prepared so begins a
/// transaction as the statement does.
#[test]
fn execute_immediate_17_11() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    assert_eq!(run(&s, "EXECUTE IMMEDIATE 'DELETE FROM T WHERE ID = 3'", &mut p), Ok(Completion::Success));
    let named = (diagnostic(&s, "COMMAND_FUNCTION"), diagnostic(&s, "DYNAMIC_FUNCTION"));
    assert_eq!(named, ("EXECUTE IMMEDIATE".to_owned(), "DELETE WHERE".to_owned()));
    assert!(s.direct("EXECUTE IMMEDIATE 'UPDATE T SET AMT = 0 WHERE ID = 2'").is_ok(), "direct SQL");
    assert_eq!(rows(&s, "SELECT ID, AMT FROM T ORDER BY ID").unwrap(), ["1,10.50", "2,0.00"]);
    s.commit().unwrap();
    text(&mut p, "SET TRANSACTION READ ONLY");
    assert_eq!(run(&s, "EXECUTE IMMEDIATE TEXT", &mut p), Ok(Completion::Success));
    assert!(s.transaction_mode().is_none(), "SET TRANSACTION begins none");
    for refused in ["SELECT ID FROM T", "DELETE FROM T WHERE ID = ?", "DELETE FROM T -- all\n", "EXECUTE IMMEDIATE 'COMMIT'"] {
        text(&mut p, refused);
        assert_eq!(run(&s, "EXECUTE IMMEDIATE TEXT", &mut p), Err("37000"), "{refused}");
    }
    assert_eq!(code(&s, "EXECUTE IMMEDIATE K", &mut p), Code::SyntaxRule, "§17.11 SR1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.22.6, §17.6 GR10, §17.8
/// GR1] PREPARE is transaction initiating; a statement prepared stays
/// after its transaction ends; DESCRIBE of a statement not prepared is
/// refused (26000).
#[test]
fn prepared_statements_outlive_their_transaction_17_6() {
    let db = setup();
    let s = db.session("APP");
    let mut p = dynamic_host();
    text(&mut p, "DELETE FROM T WHERE ID = ?");
    run(&s, "PREPARE S FROM TEXT", &mut p).unwrap();
    assert!(s.transaction_mode().is_some(), "§4.22.6");
    s.rollback().unwrap();
    p.set("K", number(1));
    assert_eq!(run(&s, "EXECUTE S USING K", &mut p), Ok(Completion::Success), "GR10");
    assert_eq!(run(&s, "ALLOCATE DESCRIPTOR 'D'", &mut p), Ok(Completion::Success));
    assert_eq!(run(&s, "DESCRIBE NONE USING SQL DESCRIPTOR 'D'", &mut p), Err("26000"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §17.6 GR4a] a statement of
/// dynamic SQL is no `<preparable statement>` (37000), through the API
/// too; nor is it executed by `execute_prepared`.
#[test]
fn dynamic_statements_are_not_prepared_17_6() {
    let db = setup();
    let s = db.session("APP");
    for refused in ["PREPARE S FROM X", "EXECUTE S", "ALLOCATE DESCRIPTOR 'D'", "DEALLOCATE PREPARE S"] {
        assert_eq!(s.prepare(refused).err().map(|e| e.sqlstate().as_str()), Some("37000"), "{refused}");
    }
}
