//! \[EXTENSION\] Settings an extension's manifest declares and reads
//! (RULES.md D280 to D282), with a real process of the example plug-in
//! (`plugin/examples/plugin_example`): declared by CREATE EXTENSION, set
//! and shown as built-in settings are, told the program with its calls,
//! a session's own over the database's, changed by ALTER EXTENSION ...
//! UPDATE and retired by DROP EXTENSION.

use turborel::db::{Database, Session};
use turborel::error::Code;

use crate::common::{Scratch, exec, rows};
use crate::plugins::fixture::{allow, install, manifest};

/// The settings of version 1.0.0.
const SETTINGS_1: &str = "\
SETTING DEPTH TYPE INTEGER MIN 1 MAX 10 DEFAULT 3 SCOPE SESSION DESCRIPTION 'How deep a search goes.';
SETTING MODE TYPE CHOICE CHOICES (FAST, SAFE) DEFAULT 'SAFE';
READS SETTINGS ('SESSION.TIME_ZONE');
";
/// The settings of version 2.0.0: MODE gone, VERBOSE new, DEPTH kept.
const SETTINGS_2: &str = "\
SETTING DEPTH TYPE INTEGER MIN 1 MAX 20 DEFAULT 3 SCOPE SESSION;
SETTING VERBOSE TYPE SWITCH;
";
/// The objects that read and write them.
const OBJECTS: &str = "\
FUNCTION SETTING_OF(VARCHAR(200)) RETURNS VARCHAR(200);
PROCEDURE RUN(VARCHAR(400)) MODIFIES SQL DATA;
";

/// A database owned by U, allowed the example at 1.0.0 with `settings`,
/// and its scratch directory.
fn declared(test: &str, settings: &str) -> (Scratch, Database) {
    let scratch = Scratch::new(test);
    let root = scratch.path("plugins");
    install(&root, ("EXAMPLE", "1.0.0"), &manifest("EXAMPLE", "1.0.0", settings, OBJECTS));
    let db = Database::new();
    allow(&db, &root);
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    db.create_user("U", b"x").unwrap();
    db.set_owner("U").unwrap();
    (scratch, db)
}

/// What the program is told of the setting `name` in `s`'s calls.
fn told(s: &Session, name: &str) -> String {
    rows(s, &format!("VALUES (SETTING_OF('{name}'))")).unwrap().join(",")
}

/// Verifies: [EXTENSION] RULES.md D280 to D282: CREATE EXTENSION declares
/// its manifest's settings under its name, shown in
/// INFORMATION_SCHEMA.SETTINGS and, with the built-in one it reads, in
/// EXTENSION_OBJECTS; its program reads each with its calls: a session's
/// own value of a SESSION setting in that session's calls alone, the
/// database's value of a DATABASE one in every session's, a built-in one
/// its READS SETTINGS names, and none it does not.
#[test]
fn an_extensions_settings_are_set_and_told_by_scope() {
    let (_scratch, db) = declared("manifest-settings-scopes", SETTINGS_1);
    let (s, other) = (db.session("U"), db.session("U"));
    s.run("CREATE EXTENSION example").unwrap();
    s.commit().unwrap();
    assert_eq!(told(&s, "EXAMPLE.DEPTH"), "3");
    s.run("SET EXAMPLE.DEPTH = 7").unwrap();
    assert_eq!((told(&s, "example.depth"), told(&other, "EXAMPLE.DEPTH")), ("7".to_owned(), "3".to_owned()));
    s.commit().unwrap();
    s.run("SET DATABASE EXAMPLE.MODE = 'fast'").unwrap();
    assert_eq!((told(&s, "EXAMPLE.MODE"), told(&other, "EXAMPLE.MODE")), ("FAST".to_owned(), "FAST".to_owned()));
    assert_ne!(told(&s, "SESSION.TIME_ZONE"), "NULL");
    assert_eq!(told(&s, "STATEMENT.TIMEOUT_MS"), "NULL");
    let shown = "SELECT SETTING_NAME, SETTING_VALUE, VALUE_SOURCE, IS_SESSION_SETTABLE FROM INFORMATION_SCHEMA.SETTINGS \
                 WHERE SETTING_NAME LIKE 'EXAMPLE.%' ORDER BY 1";
    assert_eq!(rows(&s, shown).unwrap(), ["EXAMPLE.DEPTH,7,SESSION,YES", "EXAMPLE.MODE,FAST,DATABASE,NO"]);
    let objects = "SELECT OBJECT_TYPE, OBJECT_NAME FROM INFORMATION_SCHEMA.EXTENSION_OBJECTS WHERE OBJECT_TYPE LIKE '%SETTING' ORDER BY 2";
    assert_eq!(rows(&s, objects).unwrap(), ["SETTING,EXAMPLE.DEPTH", "SETTING,EXAMPLE.MODE", "READS SETTING,SESSION.TIME_ZONE"]);
}

/// Verifies: [EXTENSION] RULES.md D280, D282: a value not of a setting's
/// type, out of its range or no choice of it is refused as a built-in
/// setting's is (SQLCODE -301); a session sets no DATABASE setting for
/// itself; and the program sets no setting but its own, another
/// extension's or a built-in one refused.
#[test]
fn values_and_writers_an_extensions_settings_refuse() {
    let (_scratch, db) = declared("manifest-settings-refusals", SETTINGS_1);
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    s.commit().unwrap();
    assert_eq!(exec(&s, "SET EXAMPLE.DEPTH = 11"), Err(Code::Data));
    assert_eq!(exec(&s, "SET EXAMPLE.DEPTH = 'deep'"), Err(Code::Data));
    assert_eq!(exec(&s, "SET DATABASE EXAMPLE.MODE = 'SLOW'"), Err(Code::Data));
    assert_eq!(exec(&s, "SET EXAMPLE.MODE = 'FAST'"), Err(Code::SyntaxRule));
    for foreign in ["CALL RUN('SET STATEMENT.TIMEOUT_MS = 5')", "CALL RUN('SET DATABASE OTHER.MODE = 1')"] {
        let refused = s.run(foreign).unwrap_err();
        assert!(refused.msg.contains("sets its own settings alone"), "{refused:?}");
        s.rollback().unwrap();
    }
    assert_eq!(told(&s, "EXAMPLE.DEPTH"), "3");
}

/// Verifies: [EXTENSION] RULES.md D280: ALTER EXTENSION ... UPDATE
/// declares its new manifest's settings: one kept keeps the database's
/// value if it is still of its type, one dropped is gone, one new is at
/// its default; DROP EXTENSION retires them all, and one created again
/// starts from its default.
#[test]
fn update_and_drop_change_an_extensions_settings() {
    let (scratch, db) = declared("manifest-settings-update", SETTINGS_1);
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    s.commit().unwrap();
    s.run("SET DATABASE EXAMPLE.DEPTH = 9").unwrap();
    install(&scratch.path("plugins"), ("EXAMPLE", "2.0.0"), &manifest("EXAMPLE", "2.0.0", SETTINGS_2, OBJECTS));
    s.run("ALTER EXTENSION example UPDATE").unwrap();
    s.commit().unwrap();
    let names = "SELECT SETTING_NAME, SETTING_VALUE FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME LIKE 'EXAMPLE.%' ORDER BY 1";
    assert_eq!(rows(&s, names).unwrap(), ["EXAMPLE.DEPTH,9", "EXAMPLE.VERBOSE,OFF"]);
    assert_eq!(exec(&s, "SET EXAMPLE.MODE = 'FAST'"), Err(Code::SyntaxRule));
    assert_eq!((told(&s, "EXAMPLE.DEPTH"), told(&s, "EXAMPLE.MODE")), ("9".to_owned(), "NULL".to_owned()));
    s.run("DROP EXTENSION example").unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, names).unwrap().len(), 0);
    assert_eq!(exec(&s, "SET EXAMPLE.DEPTH = 4"), Err(Code::SyntaxRule));
    s.run("CREATE EXTENSION example").unwrap();
    s.commit().unwrap();
    assert_eq!(rows(&s, names).unwrap(), ["EXAMPLE.DEPTH,3", "EXAMPLE.VERBOSE,OFF"]);
}

/// Verifies: [EXTENSION] RULES.md D280, D281: a manifest whose setting
/// takes a default not of its values, MIN past MAX, or reads a setting
/// not built in is refused by CREATE EXTENSION (SQLSTATE 38T06).
#[test]
fn a_manifest_of_wrong_settings_is_refused() {
    for (n, wrong) in [
        "SETTING MODE TYPE CHOICE CHOICES (FAST, SAFE) DEFAULT 'SLOW';",
        "SETTING DEPTH TYPE INTEGER MIN 5 MAX 1;",
        "READS SETTINGS ('NO.SUCH');",
    ]
    .into_iter()
    .enumerate()
    {
        let (_scratch, db) = declared(&format!("manifest-settings-wrong-{n}"), wrong);
        let refused = db.session("U").run("CREATE EXTENSION example").unwrap_err();
        assert_eq!(refused.sqlstate().as_str(), "38T06", "{wrong}");
    }
}

/// Verifies: [EXTENSION] RULES.md D280: the database's value of an
/// extension's setting is kept with the definitions, as a built-in
/// setting's is: the database opened again from its files declares the
/// setting as it makes the extension again, with the value set.
#[test]
fn an_extensions_setting_is_kept_with_the_database() {
    let scratch = Scratch::new("manifest-settings-kept");
    let root = scratch.path("plugins");
    install(&root, ("EXAMPLE", "1.0.0"), &manifest("EXAMPLE", "1.0.0", SETTINGS_1, OBJECTS));
    let open = || {
        let db = Database::new();
        allow(&db, &root);
        db.open_files(&crate::common::os(), &scratch.path("db"), &turborel::db::OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    db.create_user("U", b"x").unwrap();
    db.set_owner("U").unwrap();
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    s.commit().unwrap();
    s.run("SET DATABASE EXAMPLE.MODE = 'FAST'").unwrap();
    drop(s);
    db.close().unwrap();
    let again = open();
    assert_eq!(told(&again.session("U"), "EXAMPLE.MODE"), "FAST");
}
