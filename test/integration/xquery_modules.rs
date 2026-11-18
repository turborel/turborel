//! Registered XQuery modules [EXTENSION] (RULES.md D106) and XQuery's
//! Module Feature [XQuery §4.2, §4.11, §5.2.5] in XMLQUERY [ISO/IEC
//! 9075-14:2006 draft §6.18]: CREATE and DROP XQUERY MODULE, the modules a
//! query and a module import, what depends on a module, and their
//! recovery and replay.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session, Standby};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::error::SqlError;
use turborel::log::{Log, Lsn, MemoryLog};

use crate::common::{Scratch, after_crash, rows};
use crate::xml_validate::{one, refused};

/// The library module of target namespace `urn:notes` that NOTES_LIB
/// registers: a function shouting a string, and a variable.
const NOTES: &str = "module namespace n = \"urn:notes\"; declare variable $n:mark := \"!\"; declare function n:shout($s as xs:string) as xs:string { fn:concat(fn:upper-case($s), $n:mark) };";

/// The module of urn:loud, importing NOTES_LIB, whose function shouts
/// twice.
const LOUD: &str = "module namespace l = \"urn:loud\"; import module namespace n = \"urn:notes\"; declare function l:twice($s as xs:string) as xs:string { n:shout(n:shout($s)) };";

/// `text` as a character string literal.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// `CREATE XQUERY MODULE name AS text`.
fn create(name: &str, text: &str) -> String {
    format!("CREATE XQUERY MODULE {name} AS {}", quoted(text))
}

/// Registers `name` of `text` in `s`, and commits.
fn register(s: &Session, name: &str, text: &str) {
    let sql = create(name, text);
    s.run(&sql).unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    s.commit().unwrap();
}

/// A database of schemas A and B, A registering NOTES_LIB.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION B").unwrap();
    register(&db.session("A"), "NOTES_LIB", NOTES);
    db
}

/// The value of the query `query`, as a string.
fn value(s: &Session, query: &str) -> String {
    one(s, &format!("XMLCAST(XMLQUERY({} RETURNING CONTENT NULL ON EMPTY) AS VARCHAR(100))", quoted(query)))
}

/// The XQuery error code of `e`, as its message names it.
fn code_of(e: SqlError) -> String {
    assert_eq!(e.sqlstate().as_str(), "10000", "{e:?}");
    e.msg.split_whitespace().find(|w| w.starts_with("err:")).map(|w| w.trim_end_matches(':').to_owned()).unwrap_or(e.msg)
}

/// The XQuery error code the query `query` fails with, rolled back.
fn code(s: &Session, query: &str) -> String {
    let sql = format!("VALUES (XMLQUERY({} RETURNING CONTENT NULL ON EMPTY))", quoted(query));
    let e = crate::common::fetched(s, &sql).expect_err(&sql);
    s.rollback().unwrap();
    code_of(e)
}

/// The XQuery error code registering `name` of `text` in `s` fails with,
/// rolled back.
fn refused_text(s: &Session, name: &str, text: &str) -> String {
    let e = s.run(&create(name, text)).expect_err(text);
    s.rollback().unwrap();
    code_of(e)
}

/// Verifies: [EXTENSION] RULES.md D106 [XQuery §4.11, §5.2.5]: a query
/// imports a registered XQuery module by its target namespace, at its
/// name, alone or qualified, its location or neither, calls its functions
/// and reads its variables; a module registered imports another, and a
/// query importing it reaches what that one imports through it alone.
#[test]
fn registered_modules_are_imported_4_11() {
    let db = database();
    let a = db.session("A");
    assert_eq!(value(&a, "import module namespace n = \"urn:notes\" at \"NOTES_LIB\"; n:shout(\"hi\")"), "HI!");
    assert_eq!(value(&a, "import module namespace n = \"urn:notes\"; $n:mark"), "!");
    let quiet = NOTES.replace("\"!\"", "\".\"");
    a.run(&format!("CREATE XQUERY MODULE QUIET LOCATION 'quiet.xq' AS {}", quoted(&quiet))).unwrap();
    a.commit().unwrap();
    assert_eq!(value(&a, "import module namespace n = \"urn:notes\" at \"quiet.xq\"; n:shout(\"hi\")"), "HI.");
    assert_eq!(value(&a, "import module namespace n = \"urn:notes\" at \"A.QUIET\"; n:shout(\"hi\")"), "HI.");
    assert_eq!(value(&a, "import module namespace n = \"urn:notes\"; n:shout(\"hi\")"), "HI!", "the least name");
    register(&a, "LOUD_LIB", LOUD);
    assert_eq!(value(&a, "import module namespace l = \"urn:loud\"; l:twice(\"a\")"), "A!!");
    let through = "import module namespace l = \"urn:loud\"; declare namespace n = \"urn:notes\"; n:shout(\"a\")";
    assert_eq!(code(&a, through), "err:XPST0017", "imports are not transitive");
}

/// Verifies: [EXTENSION] RULES.md D106 [XQuery §4.2, §4.11, Appendix F]:
/// CREATE XQUERY MODULE refuses a name registered (SQLCODE -103), a
/// schema not there (3F000), a schema not the user's own (-104), a module
/// of the zero-length target namespace (-103); and, as SQLSTATE 10000, a
/// main module (err:XPST0003), a declaration outside the target namespace
/// (err:XQST0048) and an import of a namespace no module has
/// (err:XQST0059); ROLLBACK undoes it.
#[test]
fn registration_is_refused_4_2() {
    let db = database();
    let a = db.session("A");
    assert_eq!(refused(&a, &create("NOTES_LIB", NOTES)), (-103, "2A000".to_owned()));
    assert_eq!(refused(&a, &create("NOWHERE.M", NOTES)).1, "3F000");
    assert_eq!(refused(&a, &create("B.M", NOTES)).0, -104);
    assert_eq!(refused(&a, &create("EMPTY", "module namespace e = \"\";")).0, -103);
    assert_eq!(refused_text(&a, "MAIN", "1 + 1"), "err:XPST0003");
    assert_eq!(refused_text(&a, "WRONG", "module namespace w = \"urn:w\"; declare function local:f() { 1 };"), "err:XQST0048");
    assert_eq!(refused_text(&a, "LOST", "module namespace l = \"urn:l\"; import module \"urn:none\";"), "err:XQST0059");
    a.run(&create("UNDONE", NOTES)).unwrap();
    a.rollback().unwrap();
    assert_eq!(refused(&a, "DROP XQUERY MODULE UNDONE CASCADE").0, -103, "rolled back");
}

/// Verifies: [EXTENSION] RULES.md D106: a user imports the registered
/// XQuery modules of its own schemas alone, and a module being registered
/// those its schema's owner may use: another's is no module of its target
/// namespace, err:XQST0059.
#[test]
fn modules_are_imported_by_those_who_may_use_them() {
    let db = database();
    let b = db.session("B");
    assert_eq!(code(&b, "import module namespace n = \"urn:notes\"; n:shout(\"b\")"), "err:XQST0059");
    assert_eq!(refused_text(&b, "LOUD_LIB", LOUD), "err:XQST0059");
}

/// Verifies: [EXTENSION] RULES.md D106 [ISO/IEC 9075:1992 §11.2 SR3, GR5]:
/// a registered XQuery module depends on those it imported: DROP XQUERY
/// MODULE RESTRICT of one of them is refused while it stands, and CASCADE
/// drops it too; DROP SCHEMA RESTRICT is refused while the schema holds a
/// module, and CASCADE drops them.
#[test]
fn modules_depend_on_those_they_import() {
    let db = database();
    let a = db.session("A");
    register(&a, "LOUD_LIB", LOUD);
    assert_eq!(refused(&a, "DROP XQUERY MODULE NOTES_LIB RESTRICT").0, -103);
    a.run("DROP XQUERY MODULE NOTES_LIB CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(code(&a, "import module \"urn:loud\"; 1"), "err:XQST0059");
    assert_eq!(refused(&a, "DROP XQUERY MODULE NOTES_LIB CASCADE").0, -103, "gone");
    assert_eq!(refused(&a, "DROP XQUERY MODULE LOUD_LIB CASCADE").0, -103, "gone with it");
    register(&a, "NOTES_LIB", NOTES);
    assert_eq!(refused(&a, "DROP SCHEMA A RESTRICT").0, -103);
    a.run("DROP SCHEMA A CASCADE").unwrap();
    a.commit().unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    register(&a, "NOTES_LIB", NOTES);
}

/// A schema document of target namespace `urn:s` of a complex type `t`.
const TYPES: &str =
    r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:s"><xs:complexType name="t"/></xs:schema>"#;

/// Verifies: [EXTENSION] RULES.md D106 [XQuery §4.11, Appendix F]: a
/// module imported declaring a function of a schema type of a registered
/// XML Schema it imports is imported by a query that imports that schema
/// too, else err:XQST0036; the module depends on the XML Schema, so DROP
/// XML SCHEMA RESTRICT is refused, and CASCADE drops the module.
#[test]
fn modules_depend_on_the_schemas_they_import_4_11() {
    let db = database();
    let a = db.session("A");
    a.run(&format!("CREATE XML SCHEMA TYPES AS '{TYPES}'")).unwrap();
    a.commit().unwrap();
    let typed = "module namespace k = \"urn:k\"; import schema namespace s = \"urn:s\"; declare function k:all($e as element(*, s:t)*) as xs:integer { fn:count($e) };";
    register(&a, "TYPED", typed);
    assert_eq!(code(&a, "import module namespace k = \"urn:k\"; 1"), "err:XQST0036");
    assert_eq!(value(&a, "import schema namespace s = \"urn:s\"; import module namespace k = \"urn:k\"; k:all(())"), "0");
    assert_eq!(refused(&a, "DROP XML SCHEMA TYPES RESTRICT").0, -103);
    a.run("DROP XML SCHEMA TYPES CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(code(&a, "import module \"urn:k\"; 1"), "err:XQST0059");
    assert_eq!(refused(&a, "DROP XQUERY MODULE TYPED CASCADE").0, -103, "gone with it");
}

/// Verifies: [DESIGN] RULES.md D25, D57, D106: registered XQuery modules
/// survive reopening, from the log alone after a failure and after a
/// checkpoint and a close: one imports another still, and DROP RESTRICT
/// of the one it imports is still refused; one registered and not
/// committed at the failure is not, and one dropped stays dropped.
#[test]
fn registered_modules_survive_reopening() {
    for checkpointed in [false, true] {
        let scratch = Scratch::new(if checkpointed { "xquery-module-checkpointed" } else { "xquery-module-failed" });
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let crashed = Database::new();
        crashed.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        crashed.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
        let a = crashed.session("A");
        register(&a, "NOTES_LIB", NOTES);
        register(&a, "LOUD_LIB", LOUD);
        register(&a, "GONE", "module namespace g = \"urn:g\";");
        a.run("DROP XQUERY MODULE GONE RESTRICT").unwrap();
        a.commit().unwrap();
        if checkpointed {
            crashed.checkpoint().unwrap();
            crashed.close().unwrap();
        } else {
            a.run(&create("LATE", NOTES)).unwrap();
        }
        let db = Database::new();
        db.open_files(&if checkpointed { Arc::clone(&fs) } else { after_crash() }, &dir, &OpenOptions::default()).unwrap();
        let s = db.session("A");
        assert_eq!(value(&s, "import module namespace l = \"urn:loud\"; l:twice(\"x\")"), "X!!");
        assert_eq!(refused(&s, "DROP XQUERY MODULE NOTES_LIB RESTRICT").0, -103);
        assert_eq!(code(&s, "import module \"urn:g\"; 1"), "err:XQST0059");
        register(&s, "LATE", NOTES);
        db.close().unwrap();
        drop((db, a, crashed));
    }
}

/// Verifies: [DESIGN] RULES.md D57, D106: a standby replaying its
/// primary's log registers and drops the XQuery modules the primary did,
/// and, promoted, queries them.
#[test]
fn a_standby_replays_registered_modules() {
    let log = Arc::new(MemoryLog::new());
    let primary = Database::new();
    primary.set_log(Arc::clone(&log) as Arc<dyn Log>).unwrap();
    primary.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = primary.session("A");
    register(&a, "NOTES_LIB", NOTES);
    register(&a, "LOUD_LIB", LOUD);
    register(&a, "GONE", "module namespace g = \"urn:g\";");
    a.run("DROP XQUERY MODULE GONE CASCADE").unwrap();
    a.commit().unwrap();
    let standby = Database::new();
    standby.set_log(Arc::new(MemoryLog::new())).unwrap();
    Standby::replay(&standby, &mut log.read_from(Lsn(0))).unwrap();
    let s = standby.session("A");
    assert_eq!(value(&s, "import module namespace l = \"urn:loud\"; l:twice(\"r\")"), "R!!");
    assert_eq!(code(&s, "import module \"urn:g\"; 1"), "err:XQST0059");
    standby.promote(Arc::new(MemoryLog::new())).unwrap();
    assert_eq!(refused(&s, "DROP XQUERY MODULE NOTES_LIB RESTRICT").0, -103);
}

/// The SQLSTATE `sql` completes with in `s`.
fn completed(s: &Session, sql: &str) -> String {
    s.run(sql).unwrap_or_else(|e| panic!("{sql}: {e:?}")).sqlstate().as_str().to_owned()
}

/// Verifies: [EXTENSION] RULES.md D106 [ISO/IEC 9075-2:2023 §12.2 AR1,
/// GR9, §12.3 SR4a]: GRANT USAGE ON XQUERY MODULE lets the grantee import
/// the module, in a query and in a module it registers; USAGE_PRIVILEGES
/// shows its owner's and the grant, of OBJECT_TYPE XQUERY MODULE; a
/// grantee without grant option grants nothing, warning 01007; one
/// holding no USAGE is refused, -104; a privilege other than USAGE -103.
#[test]
fn usage_on_modules_is_granted_12_2() {
    let db = database();
    db.schema("CREATE SCHEMA AUTHORIZATION C").unwrap();
    let (a, b, c) = (db.session("A"), db.session("B"), db.session("C"));
    assert_eq!(completed(&a, "GRANT USAGE ON XQUERY MODULE NOTES_LIB TO B"), "00000");
    a.commit().unwrap();
    assert_eq!(value(&b, "import module namespace n = \"urn:notes\"; n:shout(\"b\")"), "B!");
    register(&b, "LOUD_LIB", LOUD);
    assert_eq!(value(&b, "import module namespace l = \"urn:loud\"; l:twice(\"b\")"), "B!!");
    let usage = "SELECT GRANTOR, GRANTEE, OBJECT_NAME, OBJECT_TYPE, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.USAGE_PRIVILEGES \
                 WHERE OBJECT_TYPE = 'XQUERY MODULE' ORDER BY OBJECT_NAME, GRANTOR";
    assert_eq!(rows(&a, usage).unwrap(), ["A,B,NOTES_LIB,XQUERY MODULE,USAGE,NO", "_SYSTEM,A,NOTES_LIB,XQUERY MODULE,USAGE,YES"]);
    assert_eq!(completed(&b, "GRANT USAGE ON XQUERY MODULE A.NOTES_LIB TO C"), "01007");
    b.commit().unwrap();
    assert_eq!(code(&c, "import module \"urn:notes\"; 1"), "err:XQST0059");
    assert_eq!(refused(&c, "GRANT USAGE ON XQUERY MODULE A.NOTES_LIB TO B").0, -104);
    assert_eq!(refused(&a, "GRANT EXECUTE ON XQUERY MODULE NOTES_LIB TO C").0, -103);
}

/// Verifies: [EXTENSION] RULES.md D106 [ISO/IEC 9075-2:2023 §12.7 GR9,
/// GR12a, GR12q, GR12ai]: REVOKE USAGE ON XQUERY MODULE abandons the
/// modules of a schema whose owner it leaves without USAGE that imported
/// it: RESTRICT is refused, SQLSTATE 2B000, and CASCADE drops them; the
/// grantee then imports it no more; a grant not made warns 01006.
#[test]
fn revoking_usage_abandons_what_imported_a_module_12_7() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    a.run("GRANT USAGE ON XQUERY MODULE NOTES_LIB TO B").unwrap();
    a.commit().unwrap();
    register(&b, "LOUD_LIB", LOUD);
    assert_eq!(refused(&a, "REVOKE USAGE ON XQUERY MODULE NOTES_LIB FROM B RESTRICT"), (-103, "2B000".to_owned()));
    assert_eq!(completed(&a, "REVOKE USAGE ON XQUERY MODULE NOTES_LIB FROM B CASCADE"), "00000");
    a.commit().unwrap();
    assert_eq!(refused(&b, "DROP XQUERY MODULE LOUD_LIB CASCADE").0, -103, "abandoned and dropped");
    assert_eq!(code(&b, "import module \"urn:notes\"; 1"), "err:XQST0059");
    assert_eq!(completed(&a, "REVOKE USAGE ON XQUERY MODULE NOTES_LIB FROM B RESTRICT"), "01006");
}

/// `CREATE VIEW name (S) AS VALUES` the string the query `query` gives.
fn view(name: &str, query: &str) -> String {
    format!("CREATE VIEW {name} (S) AS VALUES (XMLCAST(XMLQUERY({} RETURNING CONTENT NULL ON EMPTY) AS VARCHAR(100)))", quoted(query))
}

/// The query that shouts `v` by the module of urn:notes.
const SHOUT_V: &str = "import module namespace n = \"urn:notes\"; n:shout(\"v\")";

/// Verifies: [EXTENSION] RULES.md D106 [XQuery §4.11]: a view whose query
/// imports a registered XQuery module keeps the module it imported as it
/// was defined, not one of a lesser name registered later, and depends on
/// it: DROP XQUERY MODULE RESTRICT is refused, CASCADE drops the view; a
/// function's body likewise; and a module an XML Schema dropped with
/// CASCADE takes, takes the view importing it.
#[test]
fn definitions_keep_and_depend_on_the_modules_they_import() {
    let db = database();
    let a = db.session("A");
    a.run(&view("V", SHOUT_V)).unwrap();
    a.commit().unwrap();
    register(&a, "ALT", &NOTES.replace("\"!\"", "\"?\""));
    assert_eq!(rows(&a, "SELECT S FROM V").unwrap(), ["V!"], "the module it imported, not the least name now");
    assert_eq!(value(&a, SHOUT_V), "V?", "a statement imports the least name");
    a.run("DROP XQUERY MODULE ALT RESTRICT").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(&a, "DROP XQUERY MODULE NOTES_LIB RESTRICT").0, -103);
    let function = format!(
        "CREATE FUNCTION SHOUTED() RETURNS VARCHAR(100) CONTAINS SQL RETURN XMLCAST(XMLQUERY({} RETURNING CONTENT NULL ON EMPTY) AS VARCHAR(100))",
        quoted(SHOUT_V)
    );
    a.run(&function).unwrap();
    a.commit().unwrap();
    assert_eq!(one(&a, "SHOUTED()"), "V!");
    a.run("DROP VIEW V CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(&a, "DROP XQUERY MODULE NOTES_LIB RESTRICT").0, -103, "the function imports it");
    a.run("DROP XQUERY MODULE NOTES_LIB CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(&a, "VALUES (SHOUTED())").0, -103, "dropped with it, so none of its name");
}

/// Verifies: [EXTENSION] RULES.md D106 [ISO/IEC 9075-2:2023 §12.7 GR12a,
/// GR12q]: REVOKE USAGE ON XQUERY MODULE abandons a view of the grantee
/// whose query imported the module: RESTRICT is refused, 2B000, and
/// CASCADE drops it; a registered XML Schema dropped with CASCADE takes the
/// module importing it and a view importing that module.
#[test]
fn what_imports_a_module_is_abandoned_with_it_12_7() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    a.run("GRANT USAGE ON XQUERY MODULE NOTES_LIB TO B").unwrap();
    a.commit().unwrap();
    b.run(&view("BV", SHOUT_V)).unwrap();
    b.commit().unwrap();
    assert_eq!(refused(&a, "REVOKE USAGE ON XQUERY MODULE NOTES_LIB FROM B RESTRICT"), (-103, "2B000".to_owned()));
    a.run("REVOKE USAGE ON XQUERY MODULE NOTES_LIB FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(&b, "SELECT S FROM BV").0, -103, "abandoned and dropped");
    a.run(&format!("CREATE XML SCHEMA TYPES AS '{TYPES}'")).unwrap();
    a.commit().unwrap();
    register(&a, "TYPED", "module namespace k = \"urn:k\"; import schema namespace s = \"urn:s\"; declare function k:one() { 1 };");
    a.run(&view("KV", "import module namespace k = \"urn:k\"; k:one()")).unwrap();
    a.commit().unwrap();
    assert_eq!(refused(&a, "DROP XML SCHEMA TYPES RESTRICT").0, -103);
    a.run("DROP XML SCHEMA TYPES CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(&a, "SELECT S FROM KV").0, -103, "gone with the module");
}

/// Verifies: [EXTENSION] [DESIGN] RULES.md D25, D106: a view keeps the
/// module it imported through a checkpoint and reopening, its horizon read
/// back with it.
#[test]
fn a_view_keeps_its_modules_through_a_checkpoint() {
    let scratch = Scratch::new("xquery-module-view");
    let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
    let first = Database::new();
    first.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
    first.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = first.session("A");
    register(&a, "NOTES_LIB", NOTES);
    a.run(&view("V", SHOUT_V)).unwrap();
    a.commit().unwrap();
    register(&a, "ALT", &NOTES.replace("\"!\"", "\"?\""));
    first.checkpoint().unwrap();
    first.close().unwrap();
    let db = Database::new();
    db.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
    let s = db.session("A");
    assert_eq!(rows(&s, "SELECT S FROM V").unwrap(), ["V!"]);
    assert_eq!(refused(&s, "DROP XQUERY MODULE NOTES_LIB RESTRICT").0, -103);
    db.close().unwrap();
    drop((db, a, first));
}

/// Verifies: [EXTENSION] RULES.md D106: a generated column whose
/// expression imports a registered XQuery module keeps its table from
/// losing it: DROP XQUERY MODULE is refused even with CASCADE, as no drop
/// changes a column's values.
#[test]
fn a_generated_column_keeps_its_module() {
    let db = database();
    let a = db.session("A");
    let generated = format!(
        "CREATE TABLE G (K INTEGER, S VARCHAR(100) GENERATED ALWAYS AS (XMLCAST(XMLQUERY({} RETURNING CONTENT NULL ON EMPTY) AS VARCHAR(100))))",
        quoted(SHOUT_V)
    );
    a.run(&generated).unwrap();
    a.run("INSERT INTO G (K) VALUES (1)").unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT S FROM G").unwrap(), ["V!"]);
    assert_eq!(refused(&a, "DROP XQUERY MODULE NOTES_LIB CASCADE").0, -103);
}
