//! Unit tests of the Format of the `<get diagnostics statement>`
//! [ISO/IEC 9075:1992 §18.1; ISO/IEC 9075-2:2023 §23.1]: statement and
//! condition information, CONDITION in EXCEPTION's place, `<all
//! information>` with each `<all qualifier>`, and the text refused.

use std::collections::HashSet;

use super::*;
use crate::ast::SqlStatement;

/// The parameters the statements below may name: CONDITION among them.
fn declared() -> HashSet<String> {
    ["T", "U", "N", "CONDITION"].map(str::to_owned).into_iter().collect()
}

/// `text`, parsed as a whole statement of a procedure declaring
/// `declared`'s parameters: a `<get diagnostics statement>`, if it parses.
fn parsed(text: &str) -> Result<GetDiagnostics> {
    let statement = Parser::for_procedure(text, false, declared())?.sql_statement()?;
    let SqlStatement::GetDiagnostics(get) = statement else {
        return Err(SqlError::new(Code::Api, format!("{text} is no GET DIAGNOSTICS")));
    };
    Ok(get)
}

/// The `<all qualifier>` of `text`, an `<all information>`, and its
/// target.
fn all(text: &str) -> (String, AllQualifier) {
    match parsed(text) {
        Ok(GetDiagnostics::All { target, qualifier }) => (target.name, qualifier),
        other => panic!("{text}: {other:?}"),
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1] `<statement
/// information>` reads each item, those SQL:2023 adds and Feature T511's
/// among them, in order.
#[test]
fn statement_information_reads_each_item_23_1() {
    let names = StatementItem::EVERY.map(StatementItem::name).join(", T = ");
    let Ok(GetDiagnostics::Statement(items)) = parsed(&format!("GET DIAGNOSTICS T = {names}")) else { panic!("{names}") };
    let read: Vec<StatementItem> = items.iter().map(|(_, item)| *item).collect();
    assert_eq!(read, StatementItem::EVERY);
    let Ok(GetDiagnostics::Statement(one)) = parsed("GET DIAGNOSTICS N = TRANSACTION_ACTIVE") else { panic!() };
    assert_eq!(one.first().map(|(t, item)| (t.name.as_str(), *item)), Some(("N", StatementItem::TransactionActive)));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1] `<condition
/// information>` begins with CONDITION, as SQL:2023 writes it, or with
/// SQL-92's EXCEPTION [ISO/IEC 9075:1992 §18.1], and reads each item; a
/// parameter named CONDITION is a target where = follows it.
#[test]
fn condition_information_after_condition_or_exception_23_1() {
    let names = ConditionItem::EVERY.map(ConditionItem::name).join(", T = ");
    for word in ["CONDITION", "EXCEPTION"] {
        let Ok(GetDiagnostics::Condition { number, items }) = parsed(&format!("GET DIAGNOSTICS {word} N T = {names}")) else {
            panic!("{word}")
        };
        assert!(matches!(number, ValueSpecification::Parameter(ParameterSpecification { ref name, .. }) if name == "N"), "{word}");
        let read: Vec<ConditionItem> = items.iter().map(|(_, item)| *item).collect();
        assert_eq!(read, ConditionItem::EVERY, "{word}");
    }
    let Ok(GetDiagnostics::Condition { number, .. }) = parsed("GET DIAGNOSTICS CONDITION 2 T = MESSAGE_TEXT") else { panic!() };
    assert!(matches!(number, ValueSpecification::Literal(_)));
    let Ok(GetDiagnostics::Statement(items)) = parsed("GET DIAGNOSTICS CONDITION = NUMBER, T = MORE") else { panic!() };
    let targets: Vec<&str> = items.iter().map(|(t, _)| t.name.as_str()).collect();
    assert_eq!(targets, ["CONDITION", "T"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1] `<all information>`
/// reads its target and each `<all qualifier>`: none, STATEMENT,
/// CONDITION alone, and CONDITION with a literal or a parameter as its
/// number.
#[test]
fn all_information_with_each_qualifier_23_1() {
    assert!(matches!(all("GET DIAGNOSTICS T = ALL"), (t, AllQualifier::Everything) if t == "T"));
    assert!(matches!(all("GET DIAGNOSTICS T = ALL STATEMENT").1, AllQualifier::Statement));
    assert!(matches!(all("GET DIAGNOSTICS T = ALL CONDITION").1, AllQualifier::Condition(None)));
    assert!(matches!(all("GET DIAGNOSTICS T = ALL CONDITION 1").1, AllQualifier::Condition(Some(ValueSpecification::Literal(_)))));
    assert!(matches!(all("GET DIAGNOSTICS U = ALL CONDITION N").1, AllQualifier::Condition(Some(ValueSpecification::Parameter(_)))));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §23.1] what breaks the
/// Format is a syntax error, SQLCODE -102: ALL among other items or with
/// a second target, a qualifier not STATEMENT or CONDITION, STATEMENT with
/// a number, CONDITION with no number before its items, and an item of no
/// name; a target not declared is -103 [X3.135-1986 §7.3 SR3].
#[test]
fn text_breaking_the_format_refused_23_1() {
    let code = |text: &str| parsed(text).err().map(|e| e.code);
    for text in [
        "GET DIAGNOSTICS T = NUMBER, U = ALL",
        "GET DIAGNOSTICS T = ALL, U = NUMBER",
        "GET DIAGNOSTICS T = ALL ROWS",
        "GET DIAGNOSTICS T = ALL STATEMENT 1",
        "GET DIAGNOSTICS T = ALL CONDITION 1 2",
        "GET DIAGNOSTICS CONDITION T = MESSAGE_TEXT",
        "GET DIAGNOSTICS T = TRANSACTIONS",
        "GET DIAGNOSTICS CONDITION 1 T = NUMBER",
    ] {
        assert_eq!(code(text), Some(Code::Syntax), "{text}");
    }
    assert_eq!(code("GET DIAGNOSTICS X = ALL"), Some(Code::SyntaxRule));
}
