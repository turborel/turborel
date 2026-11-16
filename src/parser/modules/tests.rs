//! Unit tests of the syntax of `<SQL-server module definition>` and DROP
//! MODULE [ISO/IEC 9075-4:2006 draft §9.21, §9.22, §9.24, §12.8]: each part
//! read, MODULE.T named in the module's own schema, the texts kept, and
//! each refusal the parser makes with its code (RULES.md R-TEST-1,
//! R-TEST-2).

use crate::ast::{DropBehavior, DropObject, ModuleDefinition, PrivilegeObject, RoutineKind, SchemaStatement, SqlStatement};
use crate::catalog::{QualifiedName, module_schema};
use crate::error::{Code, Result, SqlError};
use crate::parser::Parser;

/// `text`, parsed as one statement.
fn parsed(text: &str) -> Result<SqlStatement> {
    let mut parser = Parser::new(text)?;
    let statement = parser.sql_statement()?;
    parser.end()?;
    Ok(statement)
}

/// The module definition `text` is, which must parse.
fn module(text: &str) -> ModuleDefinition {
    match parsed(text) {
        Ok(SqlStatement::Schema { statement: SchemaStatement::Module(m), .. }) => *m,
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

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §9.21 Format, §9.24, §12.8 GR3b]
/// A module definition reads its name, NAMES ARE, SCHEMA and PATH, its
/// declared tables named in the module's own schema, then its routines,
/// each ended by a semicolon, each text the `<schema routine>` it would
/// be; the module's text is all of it.
#[test]
fn a_module_reads_its_parts_9_21() {
    let text = "CREATE MODULE S.M NAMES ARE SQL_TEXT SCHEMA LIB PATH LIB, S \
                DECLARE LOCAL TEMPORARY TABLE MODULE.N (V INTEGER) ON COMMIT PRESERVE ROWS \
                PROCEDURE P (IN X INTEGER) MODIFIES SQL DATA INSERT INTO MODULE.N VALUES (X); \
                DECLARE FUNCTION F () RETURNS INTEGER READS SQL DATA RETURN (SELECT COUNT(*) FROM MODULE.N); \
                END MODULE";
    let m = module(text);
    assert_eq!((m.name.auth.as_deref(), m.name.ident.as_str(), m.schema.as_deref()), (Some("S"), "M", Some("LIB")));
    assert_eq!(m.path, Some(vec![(None, "LIB".to_owned()), (None, "S".to_owned())]));
    assert_eq!(m.path_text.as_deref(), Some("PATH LIB, S"));
    let own = module_schema(&QualifiedName { auth: "S".into(), ident: "M".into() });
    let [n] = m.temporaries.as_slice() else { panic!("{:?}", m.temporaries) };
    assert_eq!((n.name.auth.as_deref(), n.name.ident.as_str()), (Some(own.as_str()), "N"));
    let kinds: Vec<RoutineKind> = m.routines.iter().map(|r| r.kind).collect();
    assert_eq!(kinds, [RoutineKind::Procedure, RoutineKind::Function]);
    assert!(m.routines.iter().all(|r| r.text.starts_with("CREATE ")), "{:?}", m.routines);
    assert!(m.routines.first().is_some_and(|r| r.text.starts_with("CREATE PROCEDURE P")), "{:?}", m.routines);
    assert_eq!(m.text, text);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §9.21 Format, SR8]
/// A module of no SCHEMA, PATH or declared table reads none; one of no
/// routine, or of a routine with no semicolon after it, or of no END
/// MODULE, is refused as a syntax error (SQLCODE -102); NAMES ARE takes a
/// character set's name.
#[test]
fn a_module_needs_its_routines_and_end_9_21() {
    let m = module("CREATE MODULE M FUNCTION F () RETURNS INTEGER CONTAINS SQL RETURN 1; END MODULE");
    assert_eq!((m.schema, m.path, m.path_text, m.temporaries.len(), m.routines.len()), (None, None, None, 0, 1));
    for text in [
        "CREATE MODULE M END MODULE",
        "CREATE MODULE M FUNCTION F () RETURNS INTEGER CONTAINS SQL RETURN 1 END MODULE",
        "CREATE MODULE M FUNCTION F () RETURNS INTEGER CONTAINS SQL RETURN 1;",
        "CREATE MODULE M NAMES ARE FUNCTION F () RETURNS INTEGER CONTAINS SQL RETURN 1; END MODULE",
    ] {
        let e = refusal(text);
        assert_eq!((e.code, e.sqlcode()), (Code::Syntax, -102_i32), "{text}: {e:?}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-4:2006 draft §9.22 Format, §10.2 Format]
/// DROP MODULE reads its name and drop behavior, which it needs; a
/// privilege names MODULE and its name.
#[test]
fn drop_module_and_module_privileges_9_22() {
    let Ok(SqlStatement::Drop { object: DropObject::Module(name), behavior }) = parsed("DROP MODULE S.M CASCADE") else {
        panic!("DROP MODULE");
    };
    assert_eq!((name.auth.as_deref(), name.ident.as_str(), behavior), (Some("S"), "M", DropBehavior::Cascade));
    let Ok(SqlStatement::Schema { statement: SchemaStatement::Grant(p), .. }) = parsed("GRANT EXECUTE ON MODULE M TO B") else {
        panic!("GRANT ON MODULE");
    };
    assert_eq!((p.object, p.table.ident.as_str()), (PrivilegeObject::Module, "M"));
    let e = refusal("DROP MODULE M");
    assert_eq!(e.code, Code::Syntax, "{e:?}");
}
