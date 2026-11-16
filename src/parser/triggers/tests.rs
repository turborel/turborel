//! Unit tests of the Format and the catalog-free Syntax Rules of the
//! `<trigger definition>` [ISO/IEC 9075-2:2023 §11.49] and the `<drop
//! trigger statement>` [§11.50]: every action time, event, transition
//! name, level and action form, new transition variable column references
//! as targets [§6.4 SR7, §6.6 SR13], and each refusal with its code.

use super::*;
use crate::ast::{GetDiagnostics, SchemaStatement, TargetKind};

/// `text`, parsed as one statement: its trigger definition and the
/// statement's text, if it is one.
fn parsed(text: &str) -> Result<(TriggerDefinition, String)> {
    let statement = Parser::new(text)?.sql_statement()?;
    let SqlStatement::Schema { statement: SchemaStatement::Trigger(definition), text: written } = statement else {
        return Err(SqlError::new(Code::Api, format!("{text} is no trigger definition")));
    };
    Ok((definition, written))
}

/// The trigger definition `text` is, which must parse.
fn trigger(text: &str) -> TriggerDefinition {
    match parsed(text) {
        Ok((definition, _)) => definition,
        Err(e) => panic!("{text}: {e:?}"),
    }
}

/// The error parsing `text` gives, which must fail.
fn refusal(text: &str) -> SqlError {
    match parsed(text) {
        Ok(definition) => panic!("{text} parsed: {definition:?}"),
        Err(e) => e,
    }
}

/// Asserts that `text` is refused under `rule` as a Syntax Rule, SQLCODE
/// -103.
fn refused_by(text: &str, rule: &str) {
    let e = refusal(text);
    assert_eq!(e.code, Code::SyntaxRule, "{text}: {e:?}");
    assert_eq!(e.rule.as_deref(), Some(rule), "{text}: {e:?}");
}

/// Asserts that `text` breaks the Format, SQLCODE -102.
fn syntax_error(text: &str) {
    let e = refusal(text);
    assert_eq!(e.code, Code::Syntax, "{text}: {e:?}");
}

/// The name of a transition name, if written.
fn named(name: Option<&(String, usize)>) -> Option<&str> {
    name.map(|(name, _)| name.as_str())
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49] `<trigger action
/// time>` reads BEFORE, AFTER and INSTEAD OF, with the trigger's name and
/// subject table.
#[test]
fn each_action_time_is_read_11_49() {
    let before = trigger("CREATE TRIGGER S.TR BEFORE INSERT ON S.T REFERENCING NEW AS X FOR EACH ROW SELECT A INTO X.Y FROM U");
    assert_eq!(before.time, ActionTime::Before);
    assert_eq!((before.name.auth.as_deref(), before.name.ident.as_str()), (Some("S"), "TR"));
    assert_eq!(before.table.ident, "T");
    assert_eq!(trigger("CREATE TRIGGER TR AFTER INSERT ON T DELETE FROM U").time, ActionTime::After);
    assert_eq!(trigger("CREATE TRIGGER TR INSTEAD OF INSERT ON V DELETE FROM U").time, ActionTime::InsteadOf);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49] `<trigger event>`
/// reads INSERT, DELETE, UPDATE and UPDATE OF a `<trigger column list>`,
/// each column where it is written.
#[test]
fn each_event_is_read_11_49() {
    assert!(matches!(trigger("CREATE TRIGGER TR AFTER INSERT ON T DELETE FROM U").event, TriggerEvent::Insert));
    assert!(matches!(trigger("CREATE TRIGGER TR AFTER DELETE ON T DELETE FROM U").event, TriggerEvent::Delete));
    let TriggerEvent::Update(none) = trigger("CREATE TRIGGER TR AFTER UPDATE ON T DELETE FROM U").event else { panic!() };
    assert_eq!(none, Vec::new());
    let text = "CREATE TRIGGER TR AFTER UPDATE OF A, B ON T DELETE FROM U";
    let TriggerEvent::Update(columns) = trigger(text).event else { panic!() };
    assert_eq!(columns, vec![("A".to_owned(), text.find("A,").unwrap()), ("B".to_owned(), text.find("B ON").unwrap())]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49] each `<transition
/// table or variable>` is read, with and without ROW and AS.
#[test]
fn each_transition_name_is_read_11_49() {
    let both = trigger("CREATE TRIGGER TR AFTER UPDATE ON T REFERENCING OLD ROW AS O NEW ROW AS N FOR EACH ROW DELETE FROM U");
    assert_eq!((named(both.referencing.old_row.as_ref()), named(both.referencing.new_row.as_ref())), (Some("O"), Some("N")));
    let bare = trigger("CREATE TRIGGER TR AFTER UPDATE ON T REFERENCING OLD O NEW AS N FOR EACH ROW DELETE FROM U");
    assert_eq!((named(bare.referencing.old_row.as_ref()), named(bare.referencing.new_row.as_ref())), (Some("O"), Some("N")));
    let row = trigger("CREATE TRIGGER TR AFTER UPDATE ON T REFERENCING NEW ROW N FOR EACH ROW DELETE FROM U");
    assert_eq!(named(row.referencing.new_row.as_ref()), Some("N"));
    let tables = trigger("CREATE TRIGGER TR AFTER UPDATE ON T REFERENCING OLD TABLE AS OT NEW TABLE NT DELETE FROM U");
    assert_eq!((named(tables.referencing.old_table.as_ref()), named(tables.referencing.new_table.as_ref())), (Some("OT"), Some("NT")));
    assert!(tables.referencing.old_row.is_none() && tables.referencing.new_row.is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR8] FOR EACH ROW and
/// FOR EACH STATEMENT are read, and STATEMENT is implicit if neither is.
#[test]
fn statement_level_is_implicit_11_49() {
    assert_eq!(trigger("CREATE TRIGGER TR AFTER INSERT ON T FOR EACH ROW DELETE FROM U").level, TriggerLevel::Row);
    assert_eq!(trigger("CREATE TRIGGER TR AFTER INSERT ON T FOR EACH STATEMENT DELETE FROM U").level, TriggerLevel::Statement);
    assert_eq!(trigger("CREATE TRIGGER TR AFTER INSERT ON T DELETE FROM U").level, TriggerLevel::Statement);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49] a `<triggered when
/// clause>` is read; none if not written.
#[test]
fn when_clause_is_read_11_49() {
    let when = trigger("CREATE TRIGGER TR AFTER INSERT ON T REFERENCING NEW AS N FOR EACH ROW WHEN (N.A > 0) DELETE FROM U");
    assert!(when.when.is_some());
    assert!(trigger("CREATE TRIGGER TR AFTER INSERT ON T DELETE FROM U").when.is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49] a `<triggered SQL
/// statement>` is one `<SQL procedure statement>` or a `<compound
/// triggered SQL statement>` of several, each of the kinds a trigger may
/// hold; the definition's text and the action's are kept as written.
#[test]
fn single_and_compound_actions_are_read_11_49() {
    let single = "CREATE TRIGGER TR AFTER INSERT ON T INSERT INTO U VALUES (1)";
    let (definition, text) = parsed(&format!("{single};")).unwrap();
    assert_eq!(definition.action.len(), 1);
    assert_eq!((definition.text.as_str(), text.as_str()), (single, single));
    assert_eq!(definition.action_text, "INSERT INTO U VALUES (1)");
    let block = "BEGIN ATOMIC INSERT INTO U VALUES (1); UPDATE U SET A = 2 WHERE A = 1; DELETE FROM U; \
                 MERGE INTO U USING V ON U.A = V.A WHEN MATCHED THEN DELETE; END";
    let compound = trigger(&format!("CREATE TRIGGER TR AFTER INSERT ON T {block}"));
    assert_eq!(compound.action.len(), 4);
    assert!(matches!(compound.action[1], SqlStatement::UpdateSearched { .. }));
    assert!(matches!(compound.action[3], SqlStatement::Merge(_)));
    assert_eq!(compound.action_text, block);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.4 SR7, §6.6 SR13] in a
/// BEFORE row trigger, N.C, N its new transition variable name, is a
/// target of SELECT ... INTO and of GET DIAGNOSTICS: the column C of the
/// new row.
#[test]
fn new_transition_column_is_a_target_6_6() {
    let text = "CREATE TRIGGER TR BEFORE INSERT ON T REFERENCING NEW AS N FOR EACH ROW \
                BEGIN ATOMIC SELECT A INTO N.B FROM U; GET DIAGNOSTICS N.C = ROW_COUNT; END";
    let definition = trigger(text);
    let SqlStatement::Select { targets, .. } = &definition.action[0] else { panic!("{:?}", definition.action) };
    let target = &targets[0];
    assert_eq!((target.name.as_str(), target.kind, target.at), ("B", TargetKind::Transition, text.find("N.B").unwrap()));
    let SqlStatement::GetDiagnostics(GetDiagnostics::Statement(items)) = &definition.action[1] else { panic!() };
    assert_eq!((items[0].0.name.as_str(), items[0].0.kind), ("C", TargetKind::Transition));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.50] DROP TRIGGER reads
/// a `<trigger name>`, with no `<drop behavior>`.
#[test]
fn drop_trigger_is_read_11_50() {
    let statement = Parser::new("DROP TRIGGER S.TR").unwrap().sql_statement().unwrap();
    let SqlStatement::Drop { object: DropObject::Trigger(name), behavior } = statement else { panic!("{statement:?}") };
    assert_eq!((name.ident.as_str(), behavior), ("TR", DropBehavior::Restrict));
    assert_eq!(Parser::new("DROP TRIGGER TR CASCADE").unwrap().sql_statement().unwrap_err().code, Code::Syntax);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.1] a `<trigger
/// definition>` is a `<schema element>`, its text alone kept, and the
/// element after it read.
#[test]
fn trigger_in_schema_definition_is_read_11_1() {
    let definition = "CREATE TRIGGER TR AFTER INSERT ON T INSERT INTO U VALUES (1)";
    let text = format!("CREATE SCHEMA S CREATE TABLE T (A INT) {definition} CREATE TABLE U (A INT)");
    let schema = Parser::new(&text).unwrap().schema().unwrap();
    assert_eq!((schema.triggers.len(), schema.tables.len()), (1, 2));
    assert_eq!(schema.triggers[0].text, definition);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR6a] no column is
/// written twice in a `<trigger column list>`.
#[test]
fn trigger_column_list_has_no_duplicate_11_49() {
    refused_by("CREATE TRIGGER TR AFTER UPDATE OF A, B, A ON T DELETE FROM U", "ISO/IEC 9075-2:2023 §11.49 SR6a");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR7b] each of OLD
/// ROW, NEW ROW, OLD TABLE and NEW TABLE is written at most once, OLD and
/// OLD ROW being the same.
#[test]
fn transition_kind_written_once_11_49() {
    let rule = "ISO/IEC 9075-2:2023 §11.49 SR7b";
    refused_by("CREATE TRIGGER TR AFTER UPDATE ON T REFERENCING OLD A OLD ROW B FOR EACH ROW DELETE FROM U", rule);
    refused_by("CREATE TRIGGER TR AFTER UPDATE ON T REFERENCING NEW A NEW B FOR EACH ROW DELETE FROM U", rule);
    refused_by("CREATE TRIGGER TR AFTER UPDATE ON T REFERENCING OLD TABLE A OLD TABLE B DELETE FROM U", rule);
    refused_by("CREATE TRIGGER TR AFTER UPDATE ON T REFERENCING NEW TABLE A NEW TABLE B DELETE FROM U", rule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR7c] an INSERT
/// trigger names no OLD ROW or OLD TABLE, a DELETE trigger no NEW ROW or
/// NEW TABLE; an UPDATE trigger names any.
#[test]
fn event_limits_transition_names_11_49() {
    let rule = "ISO/IEC 9075-2:2023 §11.49 SR7c";
    refused_by("CREATE TRIGGER TR AFTER INSERT ON T REFERENCING OLD ROW O FOR EACH ROW DELETE FROM U", rule);
    refused_by("CREATE TRIGGER TR AFTER INSERT ON T REFERENCING OLD TABLE O DELETE FROM U", rule);
    refused_by("CREATE TRIGGER TR AFTER DELETE ON T REFERENCING NEW ROW N FOR EACH ROW DELETE FROM U", rule);
    refused_by("CREATE TRIGGER TR AFTER DELETE ON T REFERENCING NEW TABLE N DELETE FROM U", rule);
    trigger("CREATE TRIGGER TR AFTER INSERT ON T REFERENCING NEW ROW N NEW TABLE NT FOR EACH ROW DELETE FROM U");
    trigger("CREATE TRIGGER TR AFTER DELETE ON T REFERENCING OLD ROW O OLD TABLE OT FOR EACH ROW DELETE FROM U");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR7d] no two
/// transition names are equivalent.
#[test]
fn transition_names_differ_11_49() {
    refused_by(
        "CREATE TRIGGER TR AFTER UPDATE ON T REFERENCING OLD ROW X NEW TABLE X FOR EACH ROW DELETE FROM U",
        "ISO/IEC 9075-2:2023 §11.49 SR7d",
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR9] OLD ROW or NEW
/// ROW needs FOR EACH ROW, so not FOR EACH STATEMENT nor the implicit
/// STATEMENT. MC/DC of `(level, row)`: Statement with a row name refused,
/// Row with one taken, Statement with only a table name taken.
#[test]
fn old_row_needs_for_each_row_11_49() {
    let rule = "ISO/IEC 9075-2:2023 §11.49 SR9";
    refused_by("CREATE TRIGGER TR AFTER UPDATE ON T REFERENCING OLD ROW O DELETE FROM U", rule);
    refused_by("CREATE TRIGGER TR AFTER UPDATE ON T REFERENCING NEW N FOR EACH STATEMENT DELETE FROM U", rule);
    trigger("CREATE TRIGGER TR AFTER UPDATE ON T REFERENCING OLD ROW O FOR EACH ROW DELETE FROM U");
    trigger("CREATE TRIGGER TR AFTER UPDATE ON T REFERENCING OLD TABLE O DELETE FROM U");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR10] a `<triggered
/// action>` holds no `<dynamic parameter specification>`, `<embedded
/// variable name>` or `<host parameter name>`, even in a statement being
/// prepared. MC/DC of `identifier && period` in `transition_target`: an
/// identifier alone (true, false) and ? (false) are SR10's, both true
/// §6.4 SR7's.
#[test]
fn action_holds_no_parameter_11_49() {
    let rule = "ISO/IEC 9075-2:2023 §11.49 SR10";
    refused_by("CREATE TRIGGER TR AFTER INSERT ON T DELETE FROM U WHERE A = ?", rule);
    refused_by("CREATE TRIGGER TR AFTER INSERT ON T DELETE FROM U WHERE A = :X", rule);
    refused_by("CREATE TRIGGER TR AFTER INSERT ON T SELECT A INTO X FROM U", rule);
    refused_by("CREATE TRIGGER TR AFTER INSERT ON T GET DIAGNOSTICS X = ROW_COUNT", rule);
    let e = Parser::for_prepare("CREATE TRIGGER TR AFTER INSERT ON T DELETE FROM U WHERE A = ?", false).unwrap().sql_statement();
    assert_eq!(e.unwrap_err().rule.as_deref(), Some(rule));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.4 SR7, §6.6 SR13] a
/// target that is a column reference is a new transition variable column
/// reference: not of an AFTER trigger, nor qualified by another name.
#[test]
fn column_target_is_new_transition_column_6_4() {
    let rule = "ISO/IEC 9075-2:2023 §6.4 SR7";
    refused_by("CREATE TRIGGER TR AFTER INSERT ON T REFERENCING NEW AS N FOR EACH ROW SELECT A INTO N.B FROM U", rule);
    refused_by("CREATE TRIGGER TR BEFORE INSERT ON T REFERENCING NEW AS N FOR EACH ROW SELECT A INTO M.B FROM U", rule);
    refused_by("CREATE TRIGGER TR BEFORE UPDATE ON T REFERENCING OLD AS O FOR EACH ROW GET DIAGNOSTICS O.B = ROW_COUNT", rule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR13] turborel
/// refuses at parse, as implementation-defined (IA046) lets it, SQL
/// schema, transaction, connection, session and dynamic statements in a
/// `<triggered SQL statement>`.
#[test]
fn action_holds_no_schema_or_session_statement_11_49() {
    let rule = "ISO/IEC 9075-2:2023 §11.49 SR13";
    for action in [
        "CREATE TABLE X (A INT)",
        "DROP TABLE X CASCADE",
        "GRANT SELECT ON X TO PUBLIC",
        "COMMIT",
        "ROLLBACK",
        "SET TRANSACTION READ ONLY",
        "CONNECT TO DEFAULT",
        "SET SCHEMA 'S'",
        "PREPARE P FROM 'SELECT 1'",
        "EXECUTE IMMEDIATE 'DELETE FROM U'",
        "GET DESCRIPTOR 'D' :X = COUNT",
    ] {
        refused_by(&format!("CREATE TRIGGER TR AFTER INSERT ON T {action}"), rule);
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.4 SR1, §14.5 SR3, §14.6
/// SR1, §14.8 SR1, §14.13 SR1] no cursor is declared in a `<triggered
/// action>`'s scope, so it holds no OPEN, FETCH, CLOSE or positioned
/// UPDATE or DELETE.
#[test]
fn action_names_no_cursor_14_4() {
    refused_by("CREATE TRIGGER TR AFTER INSERT ON T OPEN C", "ISO/IEC 9075-2:2023 §14.4 SR1");
    refused_by("CREATE TRIGGER TR AFTER INSERT ON T FETCH C INTO X", "ISO/IEC 9075-2:2023 §14.5 SR3");
    refused_by("CREATE TRIGGER TR AFTER INSERT ON T CLOSE C", "ISO/IEC 9075-2:2023 §14.6 SR1");
    refused_by("CREATE TRIGGER TR AFTER INSERT ON T DELETE FROM U WHERE CURRENT OF C", "ISO/IEC 9075-2:2023 §14.8 SR1");
    refused_by("CREATE TRIGGER TR AFTER INSERT ON T UPDATE U SET A = 1 WHERE CURRENT OF C", "ISO/IEC 9075-2:2023 §14.13 SR1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR14a] a BEFORE
/// trigger's action holds no statement that possibly modifies SQL-data,
/// as turborel chooses (IA047); an AFTER trigger's may.
#[test]
fn before_trigger_modifies_no_data_11_49() {
    let rule = "ISO/IEC 9075-2:2023 §11.49 SR14a";
    for action in
        ["INSERT INTO U VALUES (1)", "UPDATE U SET A = 1", "DELETE FROM U", "MERGE INTO U USING V ON U.A = V.A WHEN MATCHED THEN DELETE"]
    {
        refused_by(&format!("CREATE TRIGGER TR BEFORE INSERT ON T {action}"), rule);
        trigger(&format!("CREATE TRIGGER TR AFTER INSERT ON T {action}"));
    }
    refused_by(
        "CREATE TRIGGER TR BEFORE INSERT ON T REFERENCING NEW N FOR EACH ROW BEGIN ATOMIC SELECT A INTO N.A FROM U; DELETE FROM U; END",
        rule,
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR14b] a BEFORE
/// trigger names no OLD TABLE or NEW TABLE.
#[test]
fn before_trigger_names_no_table_11_49() {
    let rule = "ISO/IEC 9075-2:2023 §11.49 SR14b";
    refused_by("CREATE TRIGGER TR BEFORE UPDATE ON T REFERENCING OLD TABLE OT FOR EACH ROW SELECT A INTO X.Y FROM U", rule);
    refused_by("CREATE TRIGGER TR BEFORE UPDATE ON T REFERENCING NEW TABLE NT FOR EACH ROW SELECT A INTO X.Y FROM U", rule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR15a, SR15b] an
/// INSTEAD OF trigger has no `<triggered when clause>` and no `<trigger
/// column list>`.
#[test]
fn instead_of_trigger_has_no_when_or_columns_11_49() {
    refused_by("CREATE TRIGGER TR INSTEAD OF INSERT ON V WHEN (1 = 1) DELETE FROM U", "ISO/IEC 9075-2:2023 §11.49 SR15a");
    refused_by("CREATE TRIGGER TR INSTEAD OF UPDATE OF A ON V DELETE FROM U", "ISO/IEC 9075-2:2023 §11.49 SR15b");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49] text that breaks the
/// Format is a syntax error, SQLCODE -102: no action time or event, BEGIN
/// without ATOMIC, an empty block, a statement no `<SQL procedure
/// statement>`, REFERENCING with no name, and FOR EACH of neither ROW nor
/// STATEMENT.
#[test]
fn format_errors_are_syntax_errors_11_49() {
    syntax_error("CREATE TRIGGER TR INSERT ON T DELETE FROM U");
    syntax_error("CREATE TRIGGER TR AFTER SELECT ON T DELETE FROM U");
    syntax_error("CREATE TRIGGER TR AFTER INSERT ON T BEGIN DELETE FROM U; END");
    syntax_error("CREATE TRIGGER TR AFTER INSERT ON T BEGIN ATOMIC END");
    syntax_error("CREATE TRIGGER TR AFTER INSERT ON T BEGIN ATOMIC DELETE FROM U END");
    syntax_error("CREATE TRIGGER TR AFTER INSERT ON T DECLARE C CURSOR FOR SELECT A FROM U");
    syntax_error("CREATE TRIGGER TR AFTER INSERT ON T REFERENCING FOR EACH ROW DELETE FROM U");
    syntax_error("CREATE TRIGGER TR AFTER INSERT ON T FOR EACH TABLE DELETE FROM U");
    syntax_error("CREATE TRIGGER TR AFTER INSERT ON T DELETE FROM U DELETE FROM U");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §14.10] TRUNCATE TABLE in an
/// AFTER trigger's action is valid SQL not implemented, SQLCODE -900, and
/// in a BEFORE trigger's refused by §11.49 SR14a.
#[test]
fn truncate_in_action_is_not_implemented_14_10() {
    assert_eq!(refusal("CREATE TRIGGER TR AFTER INSERT ON T TRUNCATE TABLE U").code, Code::Unsupported);
    refused_by("CREATE TRIGGER TR BEFORE INSERT ON T TRUNCATE TABLE U", "ISO/IEC 9075-2:2023 §11.49 SR14a");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.49 SR13, §16.1, §16.2 SR1] a
/// `<triggered SQL statement>` may be a CALL, alone or in a compound
/// statement, of any trigger; RETURN, of a function's body alone, it may
/// not be.
#[test]
fn action_may_call_a_procedure_11_49() {
    let definition = trigger("CREATE TRIGGER TR AFTER INSERT ON T CALL P (1, DEFAULT)");
    assert!(matches!(definition.action.as_slice(), [SqlStatement::Call(_)]));
    let text = "CREATE TRIGGER TR BEFORE INSERT ON T REFERENCING NEW N FOR EACH ROW BEGIN ATOMIC CALL P (N.A); CALL Q (); END";
    assert_eq!(trigger(text).action.len(), 2);
    syntax_error("CREATE TRIGGER TR AFTER INSERT ON T RETURN 1");
}
