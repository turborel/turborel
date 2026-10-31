//! System tables [ISO/IEC 9075:1992 §21]: the tables of INFORMATION_SCHEMA
//! whose rows the database makes as a statement reads them, the settings
//! first (RULES.md D37).

use turborel::catalog::ColumnDesc;
use turborel::db::Database;
use turborel::error::{Code, Result};
use turborel::params::Params;
use turborel::storage::Row;
use turborel::system::{Seen, SystemTable};
use turborel::value::{DataType, Value};

use crate::common::{code, exec, expected, rows, setup};

fn word(s: &str) -> Value {
    Value::Char(s.into())
}

const LOCKS_WAIT: &str = "SELECT SETTING_VALUE, VALUE_SOURCE FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = 'LOCKS.WAIT'";

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.1]; [DESIGN] RULES.md D36:
/// INFORMATION_SCHEMA.SETTINGS shows a text setting's value whole, up to
/// 1024 characters, as long as a path the server's settings take; its
/// default too. Regression test (R-TEST-7): a value was cut to 256.
#[test]
fn a_settings_value_is_shown_whole() {
    use turborel::settings::{Definition, Effect, Kind};
    let db = setup();
    let path = "/a".repeat(500);
    let declared = Definition {
        name: "PLUGIN.PATH".into(),
        kind: Kind::Text { max: 1024 },
        default: word(&path),
        per_session: false,
        effect: Effect::Statement,
        doc: "A path.".into(),
    };
    db.declare_setting(declared).unwrap();
    db.set_setting("PLUGIN.PATH", &word(&"/b".repeat(512))).unwrap();
    let lengths = "SELECT CHAR_LENGTH(SETTING_VALUE), CHAR_LENGTH(DEFAULT_VALUE) FROM INFORMATION_SCHEMA.SETTINGS \
                   WHERE SETTING_NAME = 'PLUGIN.PATH'";
    assert_eq!(rows(&db.session("APP"), lengths), expected(&["1024,1000"]));
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.1, §21.2.1]:
/// INFORMATION_SCHEMA.SETTINGS has a row for every setting, with the value
/// the reading session sees and where it comes from, its default, kind,
/// whether a session may set it and when a change takes effect; any user
/// reads it; a cursor over it keeps the rows of its OPEN.
#[test]
fn settings_are_a_system_table_21_1() {
    let db = setup();
    let s = db.session("APP");
    assert_eq!(rows(&s, LOCKS_WAIT), expected(&["OFF,DEFAULT"]));
    db.set_setting("LOCKS.WAIT", &word("ON")).unwrap();
    assert_eq!(rows(&s, LOCKS_WAIT), expected(&["ON,DATABASE"]));
    s.set_setting("LOCKS.WAIT", &word("OFF")).unwrap();
    assert_eq!(rows(&s, LOCKS_WAIT), expected(&["OFF,SESSION"]), "its own");
    assert_eq!(rows(&db.session("NOBODY"), LOCKS_WAIT), expected(&["ON,DATABASE"]), "any user's, the database's");
    let all = rows(&s, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.SETTINGS").unwrap();
    assert_eq!(all, vec![db.settings().unwrap().len().to_string()]);
    let described = "SELECT TRIM(DEFAULT_VALUE), TRIM(VALUE_KIND), IS_SESSION_SETTABLE, TRIM(TAKES_EFFECT) \
                     FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = 'OPTIMIZER.MAX_PASSES'";
    assert_eq!(rows(&s, described), expected(&["64,INTEGER FROM 1 TO 1000000,NO,STATEMENT"]));
    s.declare_cursor(
        "DECLARE C CURSOR FOR SELECT TRIM(SETTING_VALUE) FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = 'SESSION.COMMIT_AT_END'",
    )
    .unwrap();
    let mut p = Params::new().declare("V", DataType::Character { length: 8 }, Value::Null);
    s.execute("OPEN C", &mut p).unwrap();
    s.set_setting("SESSION.COMMIT_AT_END", &word("OFF")).unwrap();
    s.execute("FETCH C INTO V", &mut p).unwrap();
    assert_eq!(p.value("V").to_string(), "ON      ", "as at OPEN");
    let plan = s.explain("SELECT * FROM INFORMATION_SCHEMA.SETTINGS", &Params::new()).unwrap();
    assert!(plan.contains("System INFORMATION_SCHEMA.SETTINGS"), "{plan}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §21.1, §21.2.1, §11.9 AR1]: a
/// system table cannot be updated, no privilege but SELECT being granted on
/// it; INFORMATION_SCHEMA is the database's, so no `<schema>` makes it; a
/// CHECK, which needs REFERENCES, does not read one; a view may, its rows
/// those of the session reading it; a name INFORMATION_SCHEMA has not is
/// no table.
#[test]
fn system_tables_are_read_only_21_1() {
    let db = setup();
    let s = db.session("APP");
    for sql in [
        "INSERT INTO INFORMATION_SCHEMA.SETTINGS (SETTING_NAME) VALUES ('X')",
        "UPDATE INFORMATION_SCHEMA.SETTINGS SET SETTING_VALUE = 'X'",
        "DELETE FROM INFORMATION_SCHEMA.SETTINGS",
    ] {
        assert_eq!(exec(&s, sql), Err(Code::Privilege), "{sql}");
    }
    let made = |sql: &str| db.schema(sql).err().map(|e| e.code);
    assert_eq!(made("CREATE SCHEMA AUTHORIZATION INFORMATION_SCHEMA CREATE TABLE T (K INTEGER)"), Some(Code::SyntaxRule));
    let check =
        "CREATE SCHEMA AUTHORIZATION K CREATE TABLE T (N CHAR(128), CHECK (N IN (SELECT SETTING_NAME FROM INFORMATION_SCHEMA.SETTINGS)))";
    assert_eq!(made(check), Some(Code::Privilege), "§11.9 AR1");
    db.schema(
        "CREATE SCHEMA AUTHORIZATION V
           CREATE VIEW MINE (N) AS SELECT SETTING_NAME FROM INFORMATION_SCHEMA.SETTINGS WHERE VALUE_SOURCE = 'SESSION'",
    )
    .unwrap();
    let viewer = db.session("V");
    assert_eq!(rows(&viewer, "SELECT N FROM MINE"), expected(&[]));
    viewer.set_setting("LOCKS.WAIT", &word("ON")).unwrap();
    assert_eq!(rows(&viewer, "SELECT TRIM(N) FROM MINE"), expected(&["LOCKS.WAIT"]), "the reader's own");
    assert_eq!(code(&s, "SELECT * FROM INFORMATION_SCHEMA.NO_SUCH"), Code::SyntaxRule);
}

/// A plug-in's system table: one row, the name of the user reading it, or
/// a row given that may break the contract.
struct Readers(Option<Row>);

impl SystemTable for Readers {
    fn name(&self) -> &'static str {
        "READERS"
    }
    fn columns(&self) -> Vec<ColumnDesc> {
        vec![ColumnDesc {
            name: "READER".into(),
            data_type: DataType::Character { length: 4 },
            not_null: true,
            default: None,
            domain: None,
        }]
    }
    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        Ok(vec![self.0.clone().unwrap_or_else(|| vec![Value::Char(seen.auth.into())])])
    }
}

/// Verifies: [DESIGN] RULES.md D37, `SystemTable`: a plug-in adds a system
/// table, which a statement reads as its user sees it, its values padded to
/// its columns; one that gives a row of the wrong degree, a null for a NOT
/// NULL column, or a value too long, is refused with SQLCODE -910.
#[test]
fn plug_ins_add_system_tables() {
    let db = Database::new();
    db.register_system_table(Box::new(Readers(None))).unwrap();
    assert_eq!(rows(&db.session("AL"), "SELECT * FROM INFORMATION_SCHEMA.READERS"), expected(&["AL  "]));
    for broken in [vec![], vec![Value::Null], vec![word("TOO LONG")], vec![word("A"), word("B")]] {
        db.register_system_table(Box::new(Readers(Some(broken.clone())))).unwrap();
        assert_eq!(code(&db.session("AL"), "SELECT * FROM INFORMATION_SCHEMA.READERS"), Code::Api, "{broken:?}");
    }
    db.register_system_table(Box::new(Readers(Some(vec![word("OK      ")])))).unwrap();
    assert_eq!(rows(&db.session("AL"), "SELECT * FROM INFORMATION_SCHEMA.READERS"), expected(&["OK  "]), "spaces beyond it cut");
}
