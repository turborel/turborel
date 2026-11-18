//! Unit tests of the savepoints' statements and AND \[NO\] CHAIN
//! [ISO/IEC 9075-2:2023 §17.5 to §17.8]: each form read, and each refusal
//! with its code (RULES.md R-TEST-1, R-TEST-2).

use super::*;
use crate::error::Code;

/// `text`, parsed as one statement.
fn parsed(text: &str) -> Result<SqlStatement> {
    Parser::new(text)?.sql_statement()
}

/// The savepoint's statement `text` is, which must parse as one.
fn savepoint(text: &str) -> SavepointStatement {
    match parsed(text) {
        Ok(SqlStatement::Savepoint(s)) => s,
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

/// Verifies: [ISO/IEC 9075-2:2023 §17.5 Format, §17.6 Format, §17.8 Format, SR2]
/// SAVEPOINT, RELEASE SAVEPOINT and ROLLBACK TO SAVEPOINT each read their
/// `<savepoint name>`, a delimited one as written; ROLLBACK reads WORK and
/// AND NO CHAIN before its `<savepoint clause>`, and AND NO CHAIN alone,
/// as COMMIT does.
#[test]
fn savepoint_statements_are_read_17_5() {
    assert_eq!(savepoint("SAVEPOINT S1"), SavepointStatement::Establish("S1".into()));
    assert_eq!(savepoint("SAVEPOINT \"s 1\""), SavepointStatement::Establish("s 1".into()));
    assert_eq!(savepoint("RELEASE SAVEPOINT S1"), SavepointStatement::Release("S1".into()));
    assert_eq!(savepoint("ROLLBACK TO SAVEPOINT S1"), SavepointStatement::RollbackTo("S1".into()));
    assert_eq!(savepoint("ROLLBACK WORK TO SAVEPOINT S1"), SavepointStatement::RollbackTo("S1".into()));
    assert_eq!(savepoint("ROLLBACK AND NO CHAIN TO SAVEPOINT S1"), SavepointStatement::RollbackTo("S1".into()));
    assert_eq!(savepoint("ROLLBACK TO SAVEPOINT S1").name(), "S1");
    assert!(matches!(parsed("ROLLBACK WORK AND NO CHAIN"), Ok(SqlStatement::Rollback)));
    assert!(matches!(parsed("COMMIT AND NO CHAIN"), Ok(SqlStatement::Commit)));
    assert!(matches!(parsed("COMMIT WORK"), Ok(SqlStatement::Commit)));
}

/// Verifies: [ISO/IEC 9075-2:2023 §17.8 SR1, CR2, §17.7 CR1, §17.5 Format]
/// AND CHAIN with a `<savepoint clause>` breaks SR1; AND CHAIN alone,
/// of ROLLBACK or COMMIT, needs Feature T261, not supported; COMMIT has
/// no `<savepoint clause>`, and a savepoint's statement needs its name.
#[test]
fn and_chain_is_refused_17_8() {
    let both = refusal("ROLLBACK AND CHAIN TO SAVEPOINT S1");
    assert_eq!((both.code, both.rule.as_deref()), (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §17.8 SR1")));
    let rolled = refusal("ROLLBACK AND CHAIN");
    assert_eq!((rolled.code, rolled.rule.as_deref()), (Code::Unsupported, Some("ISO/IEC 9075-2:2023 §17.8 CR2")));
    let committed = refusal("COMMIT WORK AND CHAIN");
    assert_eq!((committed.code, committed.rule.as_deref()), (Code::Unsupported, Some("ISO/IEC 9075-2:2023 §17.7 CR1")));
    for text in ["COMMIT TO SAVEPOINT S1", "ROLLBACK TO S1", "ROLLBACK AND NO S1", "RELEASE SAVEPOINT", "ROLLBACK TO SAVEPOINT 1"] {
        assert_eq!(refusal(text).code, Code::Syntax, "{text}");
    }
}

/// Verifies: [DESIGN] a savepoint's statement begins with SAVEPOINT then a
/// name, or RELEASE then SAVEPOINT. MC/DC of `(word == SAVEPOINT &&
/// named) || (word == RELEASE && released)`: each condition flipped alone
/// with the others holding.
#[test]
fn a_savepoint_statement_is_told_by_two_words() {
    let id = |w: &str| Tok::Identifier(w.to_owned());
    let begins = |a: &Tok, b: &Tok| begins_savepoint_statement(Some(a), Some(b));
    assert!(begins(&id(SAVEPOINT), &id("S1")));
    assert!(!begins(&id(SAVEPOINT), &Tok::KeyWord(kw::FROM)));
    assert!(!begins(&id("SAVEPOINTS"), &id("S1")));
    assert!(begins(&id(RELEASE), &id(SAVEPOINT)));
    assert!(!begins(&id(RELEASE), &id("S1")));
    assert!(!begins(&id("RELEASED"), &id(SAVEPOINT)));
    assert!(!begins_savepoint_statement(Some(&id(SAVEPOINT)), None));
    assert!(!begins_savepoint_statement(Some(&Tok::KeyWord(kw::SELECT)), Some(&id("S1"))));
}

/// Verifies: [DESIGN] SAVEPOINT, RELEASE and CHAIN are no key words:
/// elsewhere each is an identifier, as of a table and its columns.
#[test]
fn the_words_stay_identifiers() {
    let Ok(SqlStatement::Insert(insert)) = parsed("INSERT INTO SAVEPOINT (RELEASE, CHAIN) VALUES (1, 2)") else {
        panic!("not read as an INSERT");
    };
    assert_eq!(insert.table.ident, SAVEPOINT);
    assert_eq!(insert.columns.map(|c| c.into_iter().map(|c| c.name).collect::<Vec<_>>()), Some(vec![RELEASE.to_owned(), CHAIN.to_owned()]));
}
