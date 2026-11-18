//! Unit tests of the SQL control statements' syntax [ISO/IEC 9075-4:2006
//! draft §14.1 to §14.13, §17.2, §17.3]: each form read, and each refusal
//! the parser makes with its code (RULES.md R-TEST-1, R-TEST-2).

use crate::ast::{
    AssignmentSource, CaseWhen, ConditionValue, ControlStatement, HandlerType, Iteration, LocalDeclaration, SignalValue, SqlStatement,
};
use crate::error::{Code, Result, SqlError};
use crate::parser::Parser;

/// `text`, parsed as one statement.
fn parsed(text: &str) -> Result<SqlStatement> {
    let mut parser = Parser::new(text)?;
    let statement = parser.sql_statement()?;
    parser.end()?;
    Ok(statement)
}

/// The control statement `text` is, which must parse.
fn control(text: &str) -> ControlStatement {
    match parsed(text) {
        Ok(SqlStatement::Control(c)) => *c,
        other => panic!("{text}: {other:?}"),
    }
}

/// The error parsing `text` gives, which must fail.
fn refusal(text: &str) -> SqlError {
    match parsed(text) {
        Ok(statement) => panic!("{text} parsed: {statement:?}"),
        Err(e) => e,
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.1 Format, SR5, §14.3, §14.4, §14.2, §12.1]
/// A compound statement reads its label, ATOMIC, its declarations in
/// order, variables naming several of one type with a default, conditions
/// with and without an SQLSTATE, its cursors, its handlers with their
/// condition values, its statements and its ending label; NOT ATOMIC is
/// the default.
#[test]
fn a_compound_statement_reads_its_parts_14_1() {
    let text = "B: BEGIN ATOMIC DECLARE X, Y INTEGER DEFAULT 0; DECLARE OOPS CONDITION FOR SQLSTATE '75001'; \
                DECLARE NONE CONDITION; DECLARE C CURSOR FOR SELECT 1 FROM T; \
                DECLARE CONTINUE HANDLER FOR SQLSTATE '22012', NOT FOUND SET X = 1; \
                DECLARE UNDO HANDLER FOR SQLEXCEPTION, SQLWARNING, OOPS BEGIN END; SET Y = 2; END B";
    let ControlStatement::Compound(c) = control(text) else { panic!("{text}: no compound statement") };
    assert_eq!((c.label.map(|l| l.name), c.ending.map(|l| l.name), c.atomic), (Some("B".into()), Some("B".into()), true));
    let [LocalDeclaration::Variable(variable), LocalDeclaration::Condition(oops), LocalDeclaration::Condition(none)] =
        c.declarations.as_slice()
    else {
        panic!("{text}: declarations {:?}", c.declarations)
    };
    assert_eq!(variable.names.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["X", "Y"]);
    assert!(variable.default.is_some());
    assert_eq!((oops.sqlstate.as_ref().map(|s| s.0.as_str()), none.sqlstate.as_ref()), (Some("75001"), None));
    assert_eq!(c.cursors.iter().map(|k| k.name.name.as_str()).collect::<Vec<_>>(), ["C"]);
    let kinds: Vec<(HandlerType, Vec<ConditionValue>)> =
        c.handlers.iter().map(|h| (h.kind, h.conditions.iter().map(|(v, _)| v.clone()).collect())).collect();
    assert_eq!(
        kinds,
        [
            (HandlerType::Continue, vec![ConditionValue::Sqlstate("22012".into()), ConditionValue::NotFound]),
            (HandlerType::Undo, vec![ConditionValue::Exception, ConditionValue::Warning, ConditionValue::Name("OOPS".into())]),
        ]
    );
    assert_eq!(c.statements.len(), 1);
    let ControlStatement::Compound(plain) = control("BEGIN END") else { panic!("BEGIN END: no compound statement") };
    assert!(!plain.atomic && plain.label.is_none() && plain.statements.is_empty());
    let ControlStatement::Compound(not) = control("BEGIN NOT ATOMIC END") else { panic!("NOT ATOMIC: no compound statement") };
    assert!(!not.atomic);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.1 SR3, SR4, Format]
/// An ending label other than the beginning label is refused, as an
/// ending label with no beginning label, and a declaration after a
/// statement, a cursor before a variable, or a handler before a cursor.
#[test]
fn a_compound_statement_refuses_its_misplaced_parts_14_1() {
    for text in [
        "A: BEGIN END B",
        "BEGIN END B",
        "BEGIN SET X = 1; DECLARE X INTEGER; END",
        "BEGIN DECLARE C CURSOR FOR SELECT 1 FROM T; DECLARE X INTEGER; END",
        "BEGIN DECLARE EXIT HANDLER FOR NOT FOUND BEGIN END; DECLARE C CURSOR FOR SELECT 1 FROM T; END",
    ] {
        let e = refusal(text);
        assert!(matches!(e.code, Code::Syntax | Code::SyntaxRule), "{text}: {e:?}");
    }
    for text in
        ["BEGIN INSERT INTO T VALUES (1); DECLARE Y INTEGER; END", "BEGIN DECLARE C CURSOR FOR SELECT 1 FROM T; DECLARE X INTEGER; END"]
    {
        let e = refusal(text);
        assert!(e.code == Code::Syntax && e.msg.contains("before its statements"), "{text}: {e:?}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.5 Format; P006]
/// An assignment reads one target or a list of them, a field of a target,
/// and a value, NULL, or a row of values.
#[test]
fn assignments_read_their_targets_and_sources_14_5() {
    let body = |s: &str| format!("BEGIN DECLARE X, Y INTEGER; {s}; END");
    let first = |text: &str| {
        let ControlStatement::Compound(c) = control(text) else { panic!("{text}") };
        match c.statements.into_iter().next() {
            Some(SqlStatement::Control(a)) => {
                let ControlStatement::Assignment(assignment) = *a else { panic!("{text}: no assignment") };
                assignment
            }
            other => panic!("{text}: {other:?}"),
        }
    };
    let one = first(&body("SET X = Y + 1"));
    assert_eq!((one.targets.len(), matches!(one.source, AssignmentSource::Value(_))), (1, true));
    assert!(matches!(first(&body("SET X = NULL")).source, AssignmentSource::Null(_)));
    let two = first(&body("SET (X, Y) = (1, 2)"));
    assert_eq!((two.targets.len(), matches!(two.source, AssignmentSource::Row(..))), (2, true));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.6, §14.7 Format; P004, P008]
/// A simple CASE statement reads a row operand and lists of WHEN
/// operands; a searched one its conditions; IF its ELSEIF branches and
/// ELSE.
#[test]
fn case_and_if_statements_read_their_branches_14_6() {
    let ControlStatement::Case(simple) =
        control("CASE (1, 2) WHEN (1, 2), (3, 4) THEN SIGNAL SQLSTATE '75001'; ELSE SIGNAL SQLSTATE '75001'; END CASE")
    else {
        panic!("no simple CASE")
    };
    assert_eq!(simple.operand.as_ref().map(Vec::len), Some(2));
    assert!(matches!(simple.whens.first(), Some((CaseWhen::Operands(list), _)) if list.len() == 2));
    assert!(simple.otherwise.is_some());
    let ControlStatement::Case(searched) =
        control("CASE WHEN 1 = 1 THEN SIGNAL SQLSTATE '75001'; WHEN 2 = 2 THEN SIGNAL SQLSTATE '75001'; END CASE")
    else {
        panic!("no searched CASE")
    };
    assert!(searched.operand.is_none() && searched.otherwise.is_none());
    assert!(searched.whens.iter().all(|(w, _)| matches!(w, CaseWhen::Condition(_))));
    let ControlStatement::If(i) =
        control("IF 1 = 1 THEN SIGNAL SQLSTATE '75001'; ELSEIF 2 = 2 THEN SIGNAL SQLSTATE '75001'; ELSE SIGNAL SQLSTATE '75001'; END IF")
    else {
        panic!("no IF")
    };
    assert_eq!((i.branches.len(), i.otherwise.is_some()), (2, true));
    assert!(matches!(refusal("CASE 1 WHEN 1 THEN SIGNAL SQLSTATE '75001'; END IF").code, Code::Syntax));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.8 to §14.13 Format]
/// LOOP, WHILE and REPEAT read their labels, conditions and statements;
/// FOR its loop variable, cursor name and cursor specification; END
/// followed by another statement's word is refused, as an empty
/// statement list, a REPEAT with no UNTIL, LEAVE of a label no statement
/// it is in has, and ITERATE of a compound statement's.
#[test]
fn iterations_read_their_labels_14_10() {
    let ControlStatement::Iterated(l) = control("L: LOOP LEAVE L; END LOOP L") else { panic!("no LOOP") };
    assert_eq!((l.label.map(|x| x.name), l.ending.map(|x| x.name)), (Some("L".into()), Some("L".into())));
    assert!(matches!(l.iteration, Iteration::Loop));
    let ControlStatement::Iterated(w) = control("W: WHILE 1 = 1 DO ITERATE W; END WHILE") else { panic!("no WHILE") };
    assert!(matches!(w.iteration, Iteration::While(_)) && w.ending.is_none());
    let ControlStatement::Iterated(r) = control("R: REPEAT LEAVE R; UNTIL 1 = 1 END REPEAT") else { panic!("no REPEAT") };
    assert!(matches!(r.iteration, Iteration::Repeat(_)));
    let ControlStatement::For(f) = control("F: FOR V AS K CURSOR FOR SELECT 1 FROM T DO LEAVE F; END FOR F") else { panic!("no FOR") };
    assert_eq!((f.variable.map(|x| x.name), f.cursor.map(|x| x.name)), (Some("V".into()), Some("K".into())));
    assert!(matches!(refusal("L: LOOP LEAVE L; END WHILE").code, Code::Syntax));
    // An empty statement list, or a REPEAT's with no UNTIL, ends at END.
    for (text, wanted) in [
        ("IF 1 = 1 THEN END IF", "expected an <SQL procedure statement>, found END"),
        ("REPEAT SIGNAL SQLSTATE '01001'; END REPEAT", "expected UNTIL, found END"),
    ] {
        let refused = refusal(text);
        assert!(refused.code == Code::Syntax && refused.msg.contains(wanted), "{text}: {refused:?}");
    }
    // [§14.8 SR1, §14.9 SR1] of a label in scope; ITERATE of an iteration's.
    for text in ["LEAVE L", "ITERATE L", "B: BEGIN ITERATE B; END B", "L: LOOP LEAVE M; END LOOP"] {
        assert_eq!(refusal(text).code, Code::SyntaxRule, "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §17.2, §17.3 Format, SR2, §8.2]
/// SIGNAL reads a condition name or an SQLSTATE value and its items;
/// RESIGNAL may give neither; an item named twice is refused, as a label
/// before SIGNAL and a condition no compound statement declares [SR1a ii].
#[test]
fn signal_and_resignal_read_their_values_17_2() {
    let signal = |text: &str| match parsed(text) {
        Ok(SqlStatement::Signal(s)) => *s,
        other => panic!("{text}: {other:?}"),
    };
    let first = signal("SIGNAL SQLSTATE '75001' SET MESSAGE_TEXT = 'no', TABLE_NAME = 'T'");
    assert!(!first.resignal && matches!(&first.value, Some(SignalValue::Sqlstate(v, _)) if v == "75001") && first.items.len() == 2);
    let ControlStatement::Compound(c) = control("BEGIN DECLARE OOPS CONDITION; SIGNAL OOPS; END") else { panic!("no compound") };
    assert!(
        matches!(c.statements.first(), Some(SqlStatement::Signal(s)) if matches!(&s.value, Some(SignalValue::Name(n)) if n.name == "OOPS"))
    );
    let r = signal("RESIGNAL");
    assert!(r.resignal && r.value.is_none() && r.items.is_empty());
    assert_eq!(signal("RESIGNAL SET MESSAGE_TEXT = 'again'").items.len(), 1);
    for text in ["SIGNAL SQLSTATE '75001' SET MESSAGE_TEXT = 'a', MESSAGE_TEXT = 'b'", "L: SIGNAL SQLSTATE '75001'", "SIGNAL OOPS"] {
        assert_eq!(refusal(text).code, Code::SyntaxRule, "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §4.10.6]
/// A compound statement, an assignment and the other control statements
/// but SIGNAL and RESIGNAL are not preparable.
#[test]
fn control_statements_are_not_preparable_4_10_6() {
    let prepared = |text: &str| Parser::for_prepare(text, false).and_then(|mut p| p.sql_statement());
    for text in ["BEGIN END", "L: LOOP LEAVE L; END LOOP L", "IF 1 = 1 THEN SIGNAL SQLSTATE '75001'; END IF"] {
        assert_eq!(prepared(text).map(drop).map_err(|e| e.code), Err(Code::SyntaxRule), "{text}");
    }
    assert!(matches!(prepared("SIGNAL SQLSTATE '75001'"), Ok(SqlStatement::Signal(_))));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.1 SR6, §14.13 SR9; ISO/IEC 9075-2:2023
/// §9.18 GR8b iv 2] ROLLBACK TO SAVEPOINT is read in an ATOMIC compound
/// statement and in a routine's body, where ROLLBACK alone is refused,
/// but no FOR holds one; SAVEPOINT and RELEASE SAVEPOINT are read in
/// each.
#[test]
fn savepoints_in_a_program_14_1() {
    control("BEGIN ATOMIC SAVEPOINT S; ROLLBACK TO SAVEPOINT S; RELEASE SAVEPOINT S; END");
    assert!(parsed("CREATE PROCEDURE P () MODIFIES SQL DATA BEGIN SAVEPOINT S; ROLLBACK WORK TO SAVEPOINT S; END").is_ok());
    control("BEGIN FOR SELECT 1 AS X FROM (VALUES (1)) AS O (Y) DO SAVEPOINT S; RELEASE SAVEPOINT S; END FOR; END");
    let held = refusal("BEGIN FOR SELECT 1 AS X FROM (VALUES (1)) AS O (Y) DO ROLLBACK TO SAVEPOINT S; END FOR; END");
    assert!(held.code == Code::SyntaxRule && held.rule.as_deref().is_some_and(|r| r.contains("§14.13 SR9")), "{held:?}");
    let atomic = refusal("BEGIN ATOMIC ROLLBACK AND NO CHAIN; END");
    assert!(atomic.code == Code::SyntaxRule && atomic.msg.contains("ATOMIC"), "{atomic:?}");
    let chained = refusal("BEGIN ATOMIC ROLLBACK AND CHAIN TO SAVEPOINT S; END");
    assert_eq!(chained.code, Code::SyntaxRule, "{chained:?}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §14.1 SR6, §14.13 SR9, §4.10.4; ISO/IEC
/// 9075-2:2023 §11.60 SR23f] COMMIT or ROLLBACK in an ATOMIC compound
/// statement breaks SR6, in a FOR SR9, and in a routine's body is refused
/// (IA078); in an SQL control statement NOT ATOMIC run directly it is
/// read.
#[test]
fn commit_and_rollback_in_a_program_14_1() {
    let atomic = refusal("BEGIN ATOMIC COMMIT; END");
    assert!(atomic.code == Code::SyntaxRule && atomic.msg.contains("ATOMIC"), "{atomic:?}");
    let held = refusal("BEGIN FOR SELECT 1 AS X FROM (VALUES (1)) AS O (Y) DO ROLLBACK; END FOR; END");
    assert!(held.code == Code::SyntaxRule && held.rule.as_deref().is_some_and(|r| r.contains("SR9")), "{held:?}");
    let body = refusal("CREATE PROCEDURE P () MODIFIES SQL DATA BEGIN ROLLBACK; END");
    assert!(body.code == Code::SyntaxRule && body.msg.contains("IA078"), "{body:?}");
    for text in ["BEGIN COMMIT; END", "BEGIN NOT ATOMIC IF 1 = 1 THEN ROLLBACK WORK; END IF; END", "L: LOOP COMMIT; LEAVE L; END LOOP"] {
        control(text);
    }
}
