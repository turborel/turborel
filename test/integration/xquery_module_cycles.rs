//! ALTER XQUERY MODULE [EXTENSION] (RULES.md D106) and the cycles of
//! module imports XQuery allows [XQuery §4.11]: modules of one namespace
//! importing one another, a cycle of modules not all of one namespace
//! refused (err:XQST0073), what depends on a module kept binding, and a
//! cycle dropped, revoked, recovered, checkpointed and replayed.

use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session, Standby};
use turborel::disk::io::{FileSystem, OsFileSystem};
use turborel::log::{Log, Lsn, MemoryLog};

use crate::common::{Scratch, after_crash, rows};
use crate::xml_validate::refused;
use crate::xquery_modules::{NOTES, code, completed, database, quoted, register, value};

/// The module EVEN of urn:m, as first registered: no import, so ODD may
/// import it.
const EVEN_ALONE: &str = "module namespace m = \"urn:m\"; declare variable $m:zero := 0; declare function m:even($n) { $n = $m:zero };";

/// EVEN as altered to import ODD, its namespace's other module, each
/// calling the other.
const EVEN: &str = "module namespace m = \"urn:m\"; import module \"urn:m\"; declare variable $m:zero := 0; declare function m:even($n) { if ($n = $m:zero) then fn:true() else m:odd($n - 1) };";

/// ODD of urn:m, importing EVEN.
const ODD: &str = "module namespace m = \"urn:m\"; import module \"urn:m\"; declare function m:odd($n) { if ($n = $m:zero) then fn:false() else m:even($n - 1) };";

/// `ALTER XQUERY MODULE name AS text`.
fn alter(name: &str, text: &str) -> String {
    format!("ALTER XQUERY MODULE {name} AS {}", quoted(text))
}

/// EVEN and ODD registered in `s`, then EVEN altered to import ODD: a
/// cycle of one namespace.
fn cycle(s: &Session) {
    register(s, "EVEN", EVEN_ALONE);
    register(s, "ODD", ODD);
    assert_eq!(completed(s, &alter("EVEN", EVEN)), "00000");
    s.commit().unwrap();
}

/// The XQuery error code `sql`, a statement, fails with, rolled back.
fn refused_code(s: &Session, sql: &str) -> String {
    let e = s.run(sql).expect_err(sql);
    s.rollback().unwrap();
    assert_eq!(e.sqlstate().as_str(), "10000", "{sql}: {e:?}");
    e.msg.split_whitespace().find(|w| w.starts_with("err:")).map(|w| w.trim_end_matches(':').to_owned()).unwrap_or(e.msg)
}

/// Verifies: [EXTENSION] RULES.md D106 [XQuery §4.11]: ALTER XQUERY MODULE
/// lets a module import one registered after it, so modules of one
/// namespace import one another, their functions calling each other's
/// across the cycle, from either module; and a cycle of three.
#[test]
fn modules_of_one_namespace_import_one_another_4_11() {
    let db = database();
    let a = db.session("A");
    cycle(&a);
    assert_eq!(value(&a, "import module namespace m = \"urn:m\" at \"EVEN\"; m:even(10)"), "true");
    assert_eq!(value(&a, "import module namespace m = \"urn:m\" at \"ODD\"; m:odd(7)"), "true");
    let one = "module namespace c = \"urn:c\"; declare function c:one($n) { if ($n = 0) then \"done\" else $n };";
    register(&a, "C1", one);
    register(&a, "C2", "module namespace c = \"urn:c\"; import module \"urn:c\" at \"C1\"; declare function c:two($n) { c:one($n - 1) };");
    register(&a, "C3", "module namespace c = \"urn:c\"; import module \"urn:c\" at \"C2\"; declare function c:three($n) { c:two($n) };");
    let closing = "module namespace c = \"urn:c\"; import module \"urn:c\" at \"C3\"; declare function c:one($n) { if ($n = 0) then \"done\" else c:three($n) };";
    a.run(&alter("C1", closing)).unwrap();
    a.commit().unwrap();
    assert_eq!(value(&a, "import module namespace c = \"urn:c\" at \"C2\"; c:two(4)"), "done");
}

/// Verifies: [EXTENSION] RULES.md D106 [XQuery §4.11, Appendix F]: a cycle
/// of modules not all of one namespace is err:XQST0073 as the ALTER that
/// would close it is made.
#[test]
fn a_cycle_of_namespaces_is_refused_4_11() {
    let db = database();
    let a = db.session("A");
    register(&a, "A_MOD", "module namespace a = \"urn:a\"; declare function a:f() { 1 };");
    register(&a, "B_MOD", "module namespace b = \"urn:b\"; import module namespace a = \"urn:a\"; declare function b:g() { a:f() };");
    let closing = "module namespace a = \"urn:a\"; import module namespace b = \"urn:b\"; declare function a:f() { 1 };";
    assert_eq!(refused_code(&a, &alter("A_MOD", closing)), "err:XQST0073");
}

/// Verifies: [EXTENSION] RULES.md D106: ALTER XQUERY MODULE is refused,
/// SQLCODE -103, when a module importing the module, or a view keeping it,
/// would no longer analyse or bind with the new text, as of a function
/// they call taken away or a namespace changed; a text they still bind
/// with is taken, and they read it; LOCATION, if written, replaces the
/// module's; ROLLBACK undoes it; a name of no module is -103.
#[test]
fn alter_keeps_what_depends_on_a_module_binding() {
    let db = database();
    let a = db.session("A");
    register(
        &a,
        "LOUD_LIB",
        "module namespace l = \"urn:loud\"; import module namespace n = \"urn:notes\"; declare function l:twice($s) { n:shout(n:shout($s)) };",
    );
    let view = "CREATE VIEW V (S) AS VALUES (XMLCAST(XMLQUERY('import module namespace n = \"urn:notes\"; $n:mark' RETURNING CONTENT NULL ON EMPTY) AS VARCHAR(10)))";
    a.run(view).unwrap();
    a.commit().unwrap();
    let no_shout = "module namespace n = \"urn:notes\"; declare variable $n:mark := \"!\";";
    assert_eq!(refused(&a, &alter("NOTES_LIB", no_shout)).0, -103, "LOUD_LIB calls n:shout");
    assert_eq!(refused(&a, &alter("NOTES_LIB", &NOTES.replace("urn:notes", "urn:other"))).0, -103, "a namespace no longer imported");
    let no_mark = "module namespace n = \"urn:notes\"; declare function n:shout($s as xs:string) as xs:string { fn:upper-case($s) };";
    assert_eq!(refused(&a, &alter("NOTES_LIB", no_mark)).0, -103, "V reads $n:mark");
    a.run(&alter("NOTES_LIB", &NOTES.replace("\"!\"", "\"?\""))).unwrap();
    a.rollback().unwrap();
    assert_eq!(rows(&a, "SELECT S FROM V").unwrap(), ["!"], "rolled back");
    a.run(&format!("ALTER XQUERY MODULE NOTES_LIB LOCATION 'loud.xq' AS {}", quoted(&NOTES.replace("\"!\"", "\"?\"")))).unwrap();
    a.commit().unwrap();
    assert_eq!(rows(&a, "SELECT S FROM V").unwrap(), ["?"]);
    assert_eq!(value(&a, "import module namespace l = \"urn:loud\"; l:twice(\"a\")"), "A??");
    assert_eq!(value(&a, "import module namespace n = \"urn:notes\" at \"loud.xq\"; $n:mark"), "?");
    assert_eq!(refused(&a, &alter("NOWHERE", NOTES)).0, -103);
}

/// Verifies: [EXTENSION] RULES.md D106 [ISO/IEC 9075-2:2023 §12.7 GR12a]:
/// DROP XQUERY MODULE RESTRICT of a module of a cycle is refused, as the
/// other depends on it; CASCADE drops the cycle; a REVOKE abandoning one
/// module of a cycle drops both, as each imports the other.
#[test]
fn a_cycle_is_dropped_whole() {
    let db = database();
    let (a, b) = (db.session("A"), db.session("B"));
    cycle(&a);
    assert_eq!(refused(&a, "DROP XQUERY MODULE EVEN RESTRICT").0, -103);
    a.run("DROP XQUERY MODULE EVEN CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(&a, "DROP XQUERY MODULE ODD CASCADE").0, -103, "gone with it");
    register(&a, "EVEN", EVEN_ALONE);
    a.run("GRANT USAGE ON XQUERY MODULE EVEN TO B").unwrap();
    a.commit().unwrap();
    register(&b, "ODD", ODD);
    b.run("GRANT USAGE ON XQUERY MODULE ODD TO A").unwrap();
    b.commit().unwrap();
    a.run(&alter("EVEN", &EVEN.replace("import module \"urn:m\"", "import module \"urn:m\" at \"B.ODD\""))).unwrap();
    a.commit().unwrap();
    assert_eq!(value(&b, "import module namespace m = \"urn:m\" at \"ODD\"; m:odd(3)"), "true");
    assert_eq!(refused(&a, "REVOKE USAGE ON XQUERY MODULE EVEN FROM B RESTRICT"), (-103, "2B000".to_owned()));
    a.run("REVOKE USAGE ON XQUERY MODULE EVEN FROM B CASCADE").unwrap();
    a.commit().unwrap();
    assert_eq!(refused(&a, "DROP XQUERY MODULE EVEN CASCADE").0, -103, "abandoned with ODD, which it imports");
    assert_eq!(refused(&b, "DROP XQUERY MODULE ODD CASCADE").0, -103, "abandoned");
}

/// Verifies: [DESIGN] RULES.md D25, D57, D106: a cycle of modules
/// survives reopening, from the log alone after a failure, the ALTER that
/// closed it replayed in its order, and after a checkpoint, whose image
/// holds modules that depend on each other; a standby replaying the log
/// holds it too.
#[test]
fn a_cycle_survives_reopening_and_replay() {
    for checkpointed in [false, true] {
        let scratch = Scratch::new(if checkpointed { "xquery-cycle-checkpointed" } else { "xquery-cycle-failed" });
        let (fs, dir): (Arc<dyn FileSystem>, _) = (Arc::new(OsFileSystem), scratch.path("db"));
        let first = Database::new();
        first.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        first.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
        cycle(&first.session("A"));
        if checkpointed {
            first.checkpoint().unwrap();
            first.close().unwrap();
        }
        let db = Database::new();
        db.open_files(&if checkpointed { Arc::clone(&fs) } else { after_crash() }, &dir, &OpenOptions::default()).unwrap();
        let s = db.session("A");
        assert_eq!(value(&s, "import module namespace m = \"urn:m\" at \"ODD\"; m:odd(5)"), "true");
        assert_eq!(refused(&s, "DROP XQUERY MODULE ODD RESTRICT").0, -103);
        db.close().unwrap();
        drop((db, first));
    }
    let log = Arc::new(MemoryLog::new());
    let primary = Database::new();
    primary.set_log(Arc::clone(&log) as Arc<dyn Log>).unwrap();
    primary.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    cycle(&primary.session("A"));
    let standby = Database::new();
    standby.set_log(Arc::new(MemoryLog::new())).unwrap();
    Standby::replay(&standby, &mut log.read_from(Lsn(0))).unwrap();
    assert_eq!(value(&standby.session("A"), "import module namespace m = \"urn:m\" at \"EVEN\"; m:even(8)"), "true");
    assert_eq!(code(&standby.session("A"), "import module \"urn:none\"; 1"), "err:XQST0059");
}
