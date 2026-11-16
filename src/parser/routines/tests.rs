//! Unit tests of the Format and the catalog-free Syntax Rules of the
//! `<SQL-invoked routine>` [ISO/IEC 9075-2:2023 §11.60], the `<specific
//! routine designator>` [§10.6], the `<call statement>` [§16.1] and the
//! `<return statement>` [§16.2]: each form read, SQL parameter references
//! as targets [§6.6 SR16], and each refusal with its code.

use super::*;
use crate::ast::{DataAccess, DropObject, GetDiagnostics, PrivilegeObject, ReturnStatement, SchemaStatement, SqlArgument};

/// `text`, parsed by `parser` as one statement: its routine definition and
/// the statement's text, if it is one.
fn parsed_by(parser: Result<Parser>, text: &str) -> Result<(RoutineDefinition, String)> {
    let statement = parser?.sql_statement()?;
    let SqlStatement::Schema { statement: SchemaStatement::Routine(definition), text: written } = statement else {
        return Err(SqlError::new(Code::Api, format!("{text} is no routine definition")));
    };
    Ok((*definition, written))
}

/// `text`, parsed as one statement.
fn parsed(text: &str) -> Result<(RoutineDefinition, String)> {
    parsed_by(Parser::new(text), text)
}

/// The routine definition `text` is, which must parse.
fn routine(text: &str) -> RoutineDefinition {
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

/// Asserts that `text` is refused under `rule` with `code`.
fn refused_with(text: &str, code: Code, rule: &str) {
    let e = refusal(text);
    assert_eq!(e.code, code, "{text}: {e:?}");
    assert_eq!(e.rule.as_deref(), Some(rule), "{text}: {e:?}");
}

/// Asserts that `text` is refused under `rule` as a Syntax Rule, SQLCODE
/// -103.
fn refused_by(text: &str, rule: &str) {
    refused_with(text, Code::SyntaxRule, rule);
}

/// Asserts that `text` is refused under `rule` as valid SQL not
/// supported, SQLCODE -900.
fn unsupported(text: &str, rule: &str) {
    refused_with(text, Code::Unsupported, rule);
}

/// Asserts that `text` breaks the Format, SQLCODE -102.
fn syntax_error(text: &str) {
    let e = refusal(text);
    assert_eq!(e.code, Code::Syntax, "{text}: {e:?}");
}

/// The statement `text` is, parsed alone, or its error.
fn statement(text: &str) -> Result<SqlStatement> {
    Parser::new(text)?.sql_statement()
}

/// The body of the procedure `text` defines, which must parse.
fn body(text: &str) -> SqlStatement {
    *routine(text).body
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60] a `<schema procedure>` is read:
/// its name, each `<parameter mode>` written or not, no `<returns
/// clause>`, its `<SQL-data access indication>`, the implicit NOT
/// DETERMINISTIC [SR9e] and its body, with the definition's and the
/// body's text.
#[test]
fn procedure_is_read_11_60() {
    let text = "CREATE PROCEDURE S.P (IN A INT, OUT B INT, INOUT C CHAR(3), D INT) MODIFIES SQL DATA INSERT INTO T VALUES (A)";
    let (definition, written) = parsed(text).unwrap();
    assert_eq!(definition.kind, RoutineKind::Procedure);
    assert_eq!((definition.name.auth.as_deref(), definition.name.ident.as_str()), (Some("S"), "P"));
    let modes: Vec<_> = definition.parameters.iter().map(|p| (p.name.as_str(), p.mode)).collect();
    let (input, output, both) = (Some(ParameterMode::In), Some(ParameterMode::Out), Some(ParameterMode::InOut));
    assert_eq!(modes, vec![("A", input), ("B", output), ("C", both), ("D", None)]);
    assert_eq!(definition.parameters[2].at, text.find("INOUT").unwrap());
    assert!(definition.returns.is_none() && definition.specific.is_none());
    assert_eq!(definition.access, DataAccess::Modifies);
    assert!(!definition.deterministic && !definition.null_call);
    assert!(matches!(*definition.body, SqlStatement::Insert(_)));
    assert_eq!(definition.body_text, "INSERT INTO T VALUES (A)");
    assert_eq!((definition.text.as_str(), written.as_str()), (text, text));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60, §16.2] a `<schema function>` is
/// read: its `<returns data type>`, each characteristic in any order, and
/// a RETURN body, of a value or NULL [§16.2 SR2].
#[test]
fn function_is_read_11_60() {
    let text = "CREATE FUNCTION F (X INT) RETURNS INT DETERMINISTIC CONTAINS SQL RETURNS NULL ON NULL INPUT SPECIFIC S.F1 LANGUAGE SQL \
                RETURN X + 1";
    let definition = routine(text);
    assert_eq!(definition.kind, RoutineKind::Function);
    assert!(definition.returns.is_some() && definition.deterministic && definition.null_call);
    assert_eq!(definition.specific.map(|s| (s.auth, s.ident)), Some((Some("S".to_owned()), "F1".to_owned())));
    assert_eq!(definition.access, DataAccess::Contains);
    assert!(matches!(*definition.body, SqlStatement::Return(ReturnStatement { value: Some(_), .. })));
    assert_eq!(definition.body_text, "RETURN X + 1");
    let null = routine("CREATE FUNCTION F () RETURNS INT NOT DETERMINISTIC CALLED ON NULL INPUT READS SQL DATA RETURN NULL");
    assert!(!null.deterministic && !null.null_call && null.parameters.is_empty());
    assert_eq!(null.access, DataAccess::Reads);
    let SqlStatement::Return(ReturnStatement { value: None, at }) = *null.body else { panic!("no RETURN NULL") };
    assert_eq!(at, null.text.find("RETURN NULL").unwrap());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR9d, SR9k] a procedure may write
/// DYNAMIC RESULT SETS 0 and OLD SAVEPOINT LEVEL, the implicit values.
#[test]
fn procedure_takes_implicit_characteristics_11_60() {
    let definition = routine("CREATE PROCEDURE P () OLD SAVEPOINT LEVEL DYNAMIC RESULT SETS 0 CONTAINS SQL DELETE FROM T");
    assert_eq!(definition.access, DataAccess::Contains);
}

/// Verifies: [DESIGN] the text a routine descriptor keeps is the
/// `<SQL-invoked routine>` alone, read again on its own to the same
/// routine, the statement's end left out.
#[test]
fn text_reads_again_11_60() {
    let definition = routine("CREATE PROCEDURE P (A INT) MODIFIES SQL DATA UPDATE T SET B = A;");
    assert_eq!(definition.text, "CREATE PROCEDURE P (A INT) MODIFIES SQL DATA UPDATE T SET B = A");
    let again = parsed_by(Parser::new(&definition.text).map(Parser::of_definition), &definition.text).unwrap().0;
    assert_eq!((again.name.ident, again.body_text), ("P".to_owned(), definition.body_text));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.1] a `<schema routine>` is a
/// `<schema element>`, its text alone kept, and the element after it read.
#[test]
fn routine_in_schema_definition_is_read_11_1() {
    let definition = "CREATE FUNCTION F (X INT) RETURNS INT CONTAINS SQL RETURN X";
    let text = format!("CREATE SCHEMA S CREATE TABLE T (A INT) {definition} CREATE TABLE U (A INT)");
    let schema = Parser::new(&text).unwrap().schema().unwrap();
    assert_eq!((schema.routines.len(), schema.tables.len()), (1, 2));
    assert_eq!(schema.routines[0].text, definition);
    let again = parsed_by(Parser::new(definition).map(Parser::of_definition), definition).unwrap().0;
    assert_eq!(again.body_text, "RETURN X");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR9a, SR9f i 2] each `<routine
/// characteristic>` is written at most once, a procedure's `<savepoint
/// level indication>` too.
#[test]
fn characteristic_written_once_11_60() {
    let rule = "ISO/IEC 9075-2:2023 §11.60 SR9a";
    for twice in [
        "LANGUAGE SQL LANGUAGE SQL",
        "PARAMETER STYLE SQL PARAMETER STYLE GENERAL",
        "SPECIFIC A SPECIFIC B",
        "DETERMINISTIC NOT DETERMINISTIC",
        "READS SQL DATA CONTAINS SQL",
        "RETURNS NULL ON NULL INPUT CALLED ON NULL INPUT",
    ] {
        refused_by(&format!("CREATE FUNCTION F () RETURNS INT {twice} RETURN 1"), rule);
    }
    refused_by("CREATE PROCEDURE P () DYNAMIC RESULT SETS 0 DYNAMIC RESULT SETS 0 CONTAINS SQL DELETE FROM T", rule);
    refused_by(
        "CREATE PROCEDURE P () OLD SAVEPOINT LEVEL OLD SAVEPOINT LEVEL MODIFIES SQL DATA DELETE FROM T",
        "ISO/IEC 9075-2:2023 §11.60 SR9f i 2",
    );
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR9g, SR9m ii, SR9m iii] an SQL
/// routine writes an `<SQL-data access indication>` other than NO SQL,
/// and no `<parameter style clause>`.
#[test]
fn sql_routine_characteristics_11_60() {
    refused_by("CREATE PROCEDURE P () DELETE FROM T", "ISO/IEC 9075-2:2023 §11.60 SR9g");
    refused_by("CREATE FUNCTION F () RETURNS INT DETERMINISTIC RETURN 1", "ISO/IEC 9075-2:2023 §11.60 SR9g");
    refused_by("CREATE FUNCTION F () RETURNS INT NO SQL RETURN 1", "ISO/IEC 9075-2:2023 §11.60 SR9m ii");
    refused_by("CREATE FUNCTION F () RETURNS INT CONTAINS SQL PARAMETER STYLE SQL RETURN 1", "ISO/IEC 9075-2:2023 §11.60 SR9m iii");
    refused_by("CREATE PROCEDURE P () PARAMETER STYLE GENERAL MODIFIES SQL DATA DELETE FROM T", "ISO/IEC 9075-2:2023 §11.60 SR9m iii");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR9c, SR9f i 1, SR9j, SR9l] a
/// function has no `<returned result sets characteristic>` and no
/// `<savepoint level indication>`, a procedure no `<null-call clause>`,
/// and NEW SAVEPOINT LEVEL needs MODIFIES SQL DATA. MC/DC of `new &&
/// !modifies`: NEW with READS refused, NEW with MODIFIES past SR9l (to
/// CR31), OLD with READS taken.
#[test]
fn characteristics_fit_the_routine_11_60() {
    refused_by("CREATE FUNCTION F () RETURNS INT CONTAINS SQL DYNAMIC RESULT SETS 0 RETURN 1", "ISO/IEC 9075-2:2023 §11.60 SR9c");
    refused_by("CREATE FUNCTION F () RETURNS INT CONTAINS SQL OLD SAVEPOINT LEVEL RETURN 1", "ISO/IEC 9075-2:2023 §11.60 SR9j");
    refused_by("CREATE PROCEDURE P () CONTAINS SQL CALLED ON NULL INPUT DELETE FROM T", "ISO/IEC 9075-2:2023 §11.60 SR9f i 1");
    refused_by("CREATE PROCEDURE P () READS SQL DATA NEW SAVEPOINT LEVEL SELECT A INTO B FROM T", "ISO/IEC 9075-2:2023 §11.60 SR9l");
    unsupported("CREATE PROCEDURE P () MODIFIES SQL DATA NEW SAVEPOINT LEVEL DELETE FROM T", "ISO/IEC 9075-2:2023 §11.60 CR31");
    routine("CREATE PROCEDURE P (OUT B INT) READS SQL DATA OLD SAVEPOINT LEVEL SELECT A INTO B FROM T");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR13, SR14, SR15, SR16a] a
/// function's parameters have no mode, an SQL routine's are named, no two
/// alike, and none of a ROW type. MC/DC of `named && duplicate`: two
/// unnamed parameters are SR14's, two named alike SR15's, two named apart
/// taken.
#[test]
fn parameters_follow_the_rules_11_60() {
    for mode in ["IN", "OUT", "INOUT"] {
        refused_by(&format!("CREATE FUNCTION F ({mode} X INT) RETURNS INT CONTAINS SQL RETURN X"), "ISO/IEC 9075-2:2023 §11.60 SR13");
    }
    let rule = "ISO/IEC 9075-2:2023 §11.60 SR14";
    refused_by("CREATE FUNCTION F (INT) RETURNS INT CONTAINS SQL RETURN 1", rule);
    refused_by("CREATE FUNCTION F (X INT, BOOLEAN) RETURNS INT CONTAINS SQL RETURN 1", rule);
    refused_by("CREATE PROCEDURE P (OUT DOUBLE PRECISION) CONTAINS SQL DELETE FROM T", rule);
    refused_by("CREATE PROCEDURE P (BOOLEAN DEFAULT TRUE) CONTAINS SQL DELETE FROM T", rule);
    refused_by("CREATE FUNCTION F (BLOB AS LOCATOR) RETURNS INT CONTAINS SQL RETURN 1", rule);
    refused_by("CREATE FUNCTION F (INT, INT) RETURNS INT CONTAINS SQL RETURN 1", rule);
    refused_by("CREATE FUNCTION F (X INT, \"X\" INT) RETURNS INT CONTAINS SQL RETURN 1", "ISO/IEC 9075-2:2023 §11.60 SR15");
    refused_by("CREATE FUNCTION F (X ROW(A INT)) RETURNS INT CONTAINS SQL RETURN 1", "ISO/IEC 9075-2:2023 §11.60 SR16a");
    routine("CREATE FUNCTION F (X INT, \"x\" INT) RETURNS INT CONTAINS SQL RETURN 1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR9m i, SR9v i, SR16b i, SR16e
/// ii, SR23a] an SQL routine has no `<locator indication>`, `<result
/// cast>` or `<external body reference>`, and an output parameter no
/// default.
#[test]
fn sql_routine_has_no_external_parts_11_60() {
    refused_by("CREATE FUNCTION F (X BLOB AS LOCATOR) RETURNS INT CONTAINS SQL RETURN 1", "ISO/IEC 9075-2:2023 §11.60 SR16b i");
    refused_by("CREATE FUNCTION F () RETURNS BLOB AS LOCATOR CONTAINS SQL RETURN NULL", "ISO/IEC 9075-2:2023 §11.60 SR9v i");
    refused_by("CREATE FUNCTION F () RETURNS INT CAST FROM SMALLINT CONTAINS SQL RETURN 1", "ISO/IEC 9075-2:2023 §11.60 SR9m i");
    refused_by("CREATE PROCEDURE P (OUT X INT DEFAULT 1) CONTAINS SQL DELETE FROM T", "ISO/IEC 9075-2:2023 §11.60 SR16e ii");
    refused_by("CREATE PROCEDURE P () CONTAINS SQL EXTERNAL NAME X", "ISO/IEC 9075-2:2023 §11.60 SR23a");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR9i] a routine of a language
/// other than SQL is an external routine, refused with SQLCODE -900
/// before the rules an SQL routine alone keeps.
#[test]
fn external_routine_is_not_supported_11_60() {
    let rule = "ISO/IEC 9075-2:2023 §11.60 SR9i";
    unsupported("CREATE PROCEDURE P () LANGUAGE C CONTAINS SQL EXTERNAL NAME X", rule);
    unsupported("CREATE FUNCTION F (X BLOB AS LOCATOR) RETURNS INT LANGUAGE C NO SQL EXTERNAL", rule);
    unsupported("CREATE FUNCTION F (INT) RETURNS INT PARAMETER STYLE GENERAL LANGUAGE COBOL NO SQL EXTERNAL", rule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 CR1, CR5, CR24, CR25, CR32, CR33,
/// CR34, SR4a, SR7c, SR9r] the features of routines turborel does not
/// support are valid SQL refused with SQLCODE -900. MC/DC of the
/// disjunctions that find them: each of TABLE and ONLY after RETURNS,
/// TABLE and DESCRIPTOR as a parameter type, and each word that begins a
/// `<polymorphic table function body>`, alone true.
#[test]
fn routine_features_are_not_supported_11_60() {
    let rule = |r: &str| format!("ISO/IEC 9075-2:2023 §11.60 {r}");
    unsupported("CREATE PROCEDURE P () CONTAINS SQL DYNAMIC RESULT SETS 1 DELETE FROM T", &rule("CR1"));
    unsupported("CREATE FUNCTION F (X INT RESULT) RETURNS INT CONTAINS SQL RETURN 1", &rule("CR5"));
    unsupported("CREATE PROCEDURE P () CONTAINS SQL SQL SECURITY DEFINER DELETE FROM T", &rule("CR24"));
    unsupported("CREATE FUNCTION F () RETURNS INT CONTAINS SQL SQL SECURITY INVOKER RETURN 1", &rule("CR24"));
    unsupported("CREATE FUNCTION F () RETURNS TABLE (A INT) READS SQL DATA RETURN NULL", &rule("CR25"));
    unsupported("CREATE PROCEDURE P (X INT DEFAULT 1) CONTAINS SQL DELETE FROM T", &rule("CR32"));
    unsupported("CREATE PROCEDURE P (IN X INT DEFAULT 1) CONTAINS SQL DELETE FROM T", &rule("CR32"));
    unsupported("CREATE PROCEDURE P (INOUT X INT DEFAULT 1) CONTAINS SQL DELETE FROM T", &rule("CR33"));
    unsupported("CREATE FUNCTION F (X INT DEFAULT 1) RETURNS INT CONTAINS SQL RETURN X", &rule("CR34"));
    unsupported("CREATE FUNCTION F () RETURNS ONLY PASS THROUGH READS SQL DATA RETURN NULL", &rule("CR25"));
    unsupported("CREATE FUNCTION F (X TABLE) RETURNS INT CONTAINS SQL RETURN 1", &rule("SR4a"));
    unsupported("CREATE FUNCTION F (X DESCRIPTOR) RETURNS INT CONTAINS SQL RETURN 1", &rule("SR4a"));
    for body in ["FULFILL WITH SPECIFIC PROCEDURE Q", "DESCRIBE WITH SPECIFIC PROCEDURE Q", "START WITH Q", "PRIVATE (X INT)"] {
        unsupported(&format!("CREATE FUNCTION F () RETURNS INT CONTAINS SQL {body}"), &rule("SR7c"));
    }
    unsupported("CREATE FUNCTION F (X INT) RETURNS INT CONTAINS SQL STATIC DISPATCH RETURN X", &rule("SR9r"));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60] text that breaks the Format is a
/// syntax error, SQLCODE -102: a function with no `<returns clause>`, a
/// procedure with one, no parameter list, no body, and a body followed by
/// more.
#[test]
fn format_errors_are_syntax_errors_11_60() {
    syntax_error("CREATE FUNCTION F () CONTAINS SQL RETURN 1");
    syntax_error("CREATE PROCEDURE P () RETURNS INT CONTAINS SQL DELETE FROM T");
    syntax_error("CREATE PROCEDURE P CONTAINS SQL DELETE FROM T");
    syntax_error("CREATE PROCEDURE P () CONTAINS SQL");
    syntax_error("CREATE PROCEDURE P () CONTAINS SQL DELETE FROM T DELETE FROM T");
    syntax_error("CREATE PROCEDURE P () READS SQL DELETE FROM T");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR19] an `<SQL-invoked routine>`
/// holds no `<dynamic parameter specification>` or `<embedded variable
/// name>`, even in a statement being prepared.
#[test]
fn body_holds_no_parameter_11_60() {
    let rule = "ISO/IEC 9075-2:2023 §11.60 SR19";
    refused_by("CREATE PROCEDURE P () MODIFIES SQL DATA DELETE FROM T WHERE A = ?", rule);
    refused_by("CREATE PROCEDURE P () MODIFIES SQL DATA DELETE FROM T WHERE A = :X", rule);
    refused_by("CREATE PROCEDURE P () READS SQL DATA SELECT A INTO ? FROM T", rule);
    let text = "CREATE FUNCTION F () RETURNS INT CONTAINS SQL RETURN ?";
    let e = parsed_by(Parser::for_prepare(text, false), text).unwrap_err();
    assert_eq!(e.rule.as_deref(), Some(rule));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.60 SR23g, §14.4 SR1, §14.5 SR3,
/// §14.6 SR1, §14.8 SR1, §14.13 SR1] a body is no `<schema definition>`,
/// and names no cursor, none being declared in its scope.
#[test]
fn body_holds_no_schema_or_cursor_11_60() {
    let procedure = |body: &str| format!("CREATE PROCEDURE P () MODIFIES SQL DATA {body}");
    refused_by(&procedure("CREATE SCHEMA Q"), "ISO/IEC 9075-2:2023 §11.60 SR23g");
    refused_by(&procedure("OPEN C"), "ISO/IEC 9075-2:2023 §14.4 SR1");
    refused_by(&procedure("FETCH C INTO X"), "ISO/IEC 9075-2:2023 §14.5 SR3");
    refused_by(&procedure("CLOSE C"), "ISO/IEC 9075-2:2023 §14.6 SR1");
    refused_by(&procedure("DELETE FROM T WHERE CURRENT OF C"), "ISO/IEC 9075-2:2023 §14.8 SR1");
    refused_by(&procedure("UPDATE T SET A = 1 WHERE CURRENT OF C"), "ISO/IEC 9075-2:2023 §14.13 SR1");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §6.6 SR9a ii 1 C, SR16] a target in a
/// body is an SQL parameter reference, `P` or `R`.`P`, in SELECT INTO
/// and GET DIAGNOSTICS, marked as such. MC/DC of `qualified || !period`
/// in `parameter_reference`: `P` (false, true) and `R.P` (true, false)
/// taken, `Q.P` (false, false) refused.
#[test]
fn parameter_is_a_target_6_6() {
    let text = "CREATE PROCEDURE P (OUT A INT, OUT B INT) READS SQL DATA SELECT X, Y INTO A, P.B FROM T";
    let SqlStatement::Select { targets, .. } = body(text) else { panic!("no SELECT") };
    let read: Vec<_> = targets.iter().map(|t| (t.name.as_str(), t.kind, t.at)).collect();
    assert_eq!(read, vec![("A", TargetKind::Routine, text.find("A,").unwrap()), ("B", TargetKind::Routine, text.find("P.B").unwrap())]);
    let SqlStatement::GetDiagnostics(GetDiagnostics::Statement(items)) =
        body("CREATE PROCEDURE P (OUT N INT) CONTAINS SQL GET DIAGNOSTICS N = ROW_COUNT")
    else {
        panic!("no GET DIAGNOSTICS")
    };
    assert_eq!((items[0].0.name.as_str(), items[0].0.kind), ("N", TargetKind::Routine));
    let rule = "ISO/IEC 9075-2:2023 §6.4 SR7";
    refused_by("CREATE PROCEDURE P (OUT A INT) READS SQL DATA SELECT X INTO Q.A FROM T", rule);
    refused_by("CREATE PROCEDURE P (OUT A INT) READS SQL DATA SELECT X INTO C FROM T", rule);
    refused_by("CREATE PROCEDURE P (OUT A INT) READS SQL DATA SELECT X INTO A.F FROM T", rule);
    refused_by("CREATE PROCEDURE P (OUT A INT) CONTAINS SQL GET DIAGNOSTICS M = ROW_COUNT", rule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §16.1, §10.4] CALL reads its routine's
/// name and each `<SQL argument>`: a value expression, DEFAULT or NULL,
/// each where it is written, or none.
#[test]
fn call_is_read_16_1() {
    let text = "CALL S.P (1, DEFAULT, NULL, A + 1)";
    let Ok(SqlStatement::Call(call)) = statement(text) else { panic!("no CALL") };
    assert_eq!((call.name.auth.as_deref(), call.name.ident.as_str()), (Some("S"), "P"));
    assert!(matches!(call.arguments[0], SqlArgument::Value(_)));
    assert!(matches!(call.arguments[1], SqlArgument::Default(at) if at == text.find("DEFAULT").unwrap()));
    assert!(matches!(call.arguments[2], SqlArgument::Null(at) if at == text.find("NULL").unwrap()));
    assert!(matches!(call.arguments[3], SqlArgument::Value(_)));
    let Ok(SqlStatement::Call(none)) = statement("CALL P ();") else { panic!("no CALL") };
    assert!(none.arguments.is_empty());
    let body = body("CREATE PROCEDURE Q (A INT) MODIFIES SQL DATA CALL P (A)");
    assert!(matches!(body, SqlStatement::Call(_)));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.4 CR1, CR5, CR10] a named argument
/// (Feature T521), a `<generalized expression>` (S023), a `<table
/// argument>` and a `<co-partition clause>` (B200) are valid SQL refused
/// with SQLCODE -900; a CALL that breaks the Format is a syntax error.
#[test]
fn call_features_are_not_supported_10_4() {
    for (text, rule) in [
        ("CALL P (A => 1)", "ISO/IEC 9075-2:2023 §10.4 CR5"),
        ("CALL P (1 AS T)", "ISO/IEC 9075-2:2023 §10.4 CR1"),
        ("CALL P (TABLE (T))", "ISO/IEC 9075-2:2023 §10.4 CR10"),
        ("CALL P (1 COPARTITION (A, B))", "ISO/IEC 9075-2:2023 §10.4 CR10"),
    ] {
        let e = statement(text).unwrap_err();
        assert_eq!((e.code, e.rule.as_deref()), (Code::Unsupported, Some(rule)), "{text}");
    }
    for text in ["CALL P", "CALL P (1", "CALL P (A => )", "CALL P (1,)"] {
        assert_eq!(statement(text).unwrap_err().code, Code::Syntax, "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §16.2 SR1] RETURN is in the body of a
/// function alone: not a statement on its own, nor a procedure's body.
#[test]
fn return_is_in_a_function_alone_16_2() {
    let rule = "ISO/IEC 9075-2:2023 §16.2 SR1";
    let e = statement("RETURN 1").unwrap_err();
    assert_eq!((e.code, e.rule.as_deref()), (Code::SyntaxRule, Some(rule)));
    refused_by("CREATE PROCEDURE P () CONTAINS SQL RETURN 1", rule);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.6, §11.62] DROP takes a `<specific
/// routine designator>`: of each `<routine type>`, by SPECIFIC and a
/// `<specific name>`, or by a name and a `<data type list>` if written,
/// empty too.
#[test]
fn designator_is_read_10_6() {
    let designator = |text: &str| {
        let Ok(SqlStatement::Drop { object: DropObject::Routine(designator), .. }) = statement(text) else { panic!("{text}") };
        designator
    };
    let specific = designator("DROP SPECIFIC PROCEDURE S.P1 CASCADE");
    assert_eq!((specific.routine_type, specific.specific), (RoutineType::Procedure, true));
    assert_eq!((specific.name.auth.as_deref(), specific.name.ident.as_str(), specific.parameters.is_none()), (Some("S"), "P1", true));
    let typed = designator("DROP ROUTINE F (INT, CHAR(3)) RESTRICT");
    assert_eq!((typed.routine_type, typed.specific), (RoutineType::Routine, false));
    assert_eq!(typed.parameters.map(|p| p.len()), Some(2));
    let function = designator("DROP FUNCTION F () CASCADE");
    assert_eq!((function.routine_type, function.parameters.map(|p| p.len())), (RoutineType::Function, Some(0)));
    assert!(designator("DROP SPECIFIC ROUTINE F RESTRICT").parameters.is_none());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §12.3, §10.6] GRANT names a routine by
/// a `<specific routine designator>`, SPECIFIC or not.
#[test]
fn designator_in_grant_is_read_12_3() {
    let Ok(SqlStatement::Schema { statement: SchemaStatement::Grant(grant), .. }) =
        statement("GRANT EXECUTE ON SPECIFIC FUNCTION S.F1 TO PUBLIC")
    else {
        panic!("no GRANT")
    };
    assert!(matches!(grant.object, PrivilegeObject::SpecificRoutine(RoutineType::Function)));
    let Ok(SqlStatement::Schema { statement: SchemaStatement::Grant(typed), .. }) = statement("GRANT EXECUTE ON PROCEDURE P (INT) TO U")
    else {
        panic!("no GRANT")
    };
    assert!(matches!(typed.object, PrivilegeObject::Routine(RoutineType::Procedure)));
    assert_eq!(typed.parameters.map(|p| p.len()), Some(1));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §10.6 SR3a] FOR a user-defined type is
/// of a METHOD alone; SPECIFIC takes no `<data type list>`.
#[test]
fn designator_refusals_10_6() {
    let e = statement("DROP FUNCTION F FOR T CASCADE").unwrap_err();
    assert_eq!((e.code, e.rule.as_deref()), (Code::SyntaxRule, Some("ISO/IEC 9075-2:2023 §10.6 SR3a")));
    assert_eq!(statement("DROP SPECIFIC FUNCTION F (INT) CASCADE").unwrap_err().code, Code::Syntax);
    assert_eq!(statement("DROP SPECIFIC F CASCADE").unwrap_err().code, Code::Syntax);
}
