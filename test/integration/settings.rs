//! Settings (RULES.md D36): the registry of every setting, the database's
//! values and a session's own, and what each changes.

use std::thread;

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;
use turborel::settings::{Definition, Effect, Kind, Source};
use turborel::value::{Decimal, Value};

use crate::common::{Scratch, after_crash, count, locking_setup, os, setup};
use turborel::value::Spelled;

fn word(s: &str) -> Value {
    Value::Char(s.into())
}

fn whole(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The value of setting `name` in `listed`, as a literal, and its source.
fn shown(listed: &[turborel::settings::Listed], name: &str) -> (String, Source) {
    let l = listed.iter().find(|l| l.definition.name == name).unwrap();
    (l.value.to_literal(), l.source)
}

/// Verifies: [DESIGN] RULES.md D36: every setting is listed with its value and
/// where it comes from: its default, then the database's once set, its
/// default again once reset; names are found in any case; a value is taken
/// only of its setting's kind and range, an engine or index method only if
/// registered, and only a declared setting is set.
#[test]
fn database_settings_are_set_listed_and_checked() {
    let db = setup();
    let listed = db.settings().unwrap();
    for name in
        ["TABLES.ENGINE", "TABLES.UNIQUE_INDEX_METHOD", "TRANSACTION.STRICT", "LOCKS.WAIT", "SESSION.COMMIT_AT_END", "OPTIMIZER.MAX_PASSES"]
    {
        assert_eq!(shown(&listed, name).1, Source::Default, "{name}");
    }
    assert_eq!(shown(&listed, "LOCKS.WAIT").0, "'OFF'");
    db.set_setting("locks.wait", &word("on")).unwrap();
    db.set_setting("OPTIMIZER.MAX_PASSES", &Value::Exact(Decimal::new(300, 2))).unwrap();
    let listed = db.settings().unwrap();
    assert_eq!(shown(&listed, "LOCKS.WAIT"), ("'ON'".into(), Source::Database));
    assert_eq!(shown(&listed, "OPTIMIZER.MAX_PASSES"), ("3".into(), Source::Database));
    db.reset_setting("LOCKS.WAIT").unwrap();
    assert_eq!(shown(&db.settings().unwrap(), "LOCKS.WAIT"), ("'OFF'".into(), Source::Default));
    let refused = [
        ("NO.SUCH.SETTING", word("X")),
        ("LOCKS.WAIT", word("SOMETIMES")),
        ("LOCKS.WAIT", whole(1)),
        ("OPTIMIZER.MAX_PASSES", whole(0)),
        ("OPTIMIZER.MAX_PASSES", Value::Exact(Decimal::new(25, 1))),
        ("PLANNER.SORT_COMPARE_COST", Value::Approx(-1.0)),
        ("TABLES.ENGINE", word("nope")),
        ("TABLES.UNIQUE_INDEX_METHOD", word("nope")),
    ];
    for (name, value) in refused {
        assert_eq!(db.set_setting(name, &value).err().map(|e| e.code), Some(Code::Api), "{name} = {value}");
    }
    assert_eq!(db.setting("OPTIMIZER.MAX_PASSES").unwrap().to_literal(), "3", "unchanged by the refusals");
    db.set_setting("TABLES.UNIQUE_INDEX_METHOD", &word("hash")).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION H CREATE TABLE T (K INTEGER NOT NULL UNIQUE)").unwrap();
    db.with_cost_model(|c| c.sort_compare = 0.5).unwrap();
    assert_eq!(shown(&db.settings().unwrap(), "PLANNER.SORT_COMPARE_COST"), (Value::Approx(0.5).to_literal(), Source::Database));
}

/// Verifies: [DESIGN] RULES.md D36: a session sets its own value of a setting
/// a session may set, over the database's, for itself alone, until it
/// resets it; it does not set one that is the database's alone. Its own
/// SESSION.COMMIT_AT_END decides how its open transaction ends [X3.135-1986 §7.1
/// GR2].
#[test]
fn sessions_set_their_own() {
    let db = setup();
    let s = db.session("APP");
    s.set_setting("SESSION.COMMIT_AT_END", &word("off")).unwrap();
    assert_eq!(s.setting("SESSION.COMMIT_AT_END").unwrap().to_literal(), "'OFF'");
    assert_eq!(shown(&s.settings().unwrap(), "SESSION.COMMIT_AT_END").1, Source::Session);
    assert_eq!(db.session("APP").setting("SESSION.COMMIT_AT_END").unwrap().to_literal(), "'ON'", "another session");
    assert_eq!(s.set_setting("OPTIMIZER.MAX_PASSES", &whole(2)).err().map(|e| e.code), Some(Code::Api), "the database's");
    s.run("INSERT INTO T (ID) VALUES (8)").unwrap();
    drop(s);
    assert_eq!(count(&db.session("APP"), "SELECT ID FROM T WHERE ID = 8"), 0, "rolled back as the session ended");
    let kept = db.session("APP");
    kept.set_setting("SESSION.COMMIT_AT_END", &word("OFF")).unwrap();
    kept.reset_setting("SESSION.COMMIT_AT_END").unwrap();
    kept.run("INSERT INTO T (ID) VALUES (9)").unwrap();
    drop(kept);
    assert_eq!(count(&db.session("APP"), "SELECT ID FROM T WHERE ID = 9"), 1, "committed: the database's again");
}

/// Verifies: [EXTENSION] RULES.md D36: `SET name = value` and `SET name TO
/// value` set a session's own value of a setting a session may set, named
/// in any case, a delimited identifier too, its value a literal or a
/// parameter; `SET name TO DEFAULT` takes the database's again. It begins
/// no transaction and ROLLBACK leaves it. An unknown setting or one the
/// database's alone is a Syntax Rule error (SQLCODE -103), a value not of
/// its kind or range a data exception (-301).
#[test]
fn set_statements_set_a_sessions_own() {
    let db = setup();
    let s = db.session("APP");
    let source = "SELECT TRIM(VALUE_SOURCE) FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = 'LOCKS.WAIT'";
    s.run("SET locks.wait = 'on'").unwrap();
    assert_eq!(s.setting("LOCKS.WAIT").unwrap().to_literal(), "'ON'");
    assert_eq!(crate::common::rows(&s, source), Ok(vec!["SESSION".to_owned()]));
    assert_eq!(db.session("APP").setting("LOCKS.WAIT").unwrap().to_literal(), "'OFF'", "another session's");
    s.run("SET LOCKS.WAIT TO DEFAULT").unwrap();
    assert_eq!(s.setting("LOCKS.WAIT").unwrap().to_literal(), "'OFF'");
    s.commit().unwrap();
    assert_eq!(s.transaction(), None);
    s.run("SET \"SESSION.COMMIT_AT_END\" TO 'OFF'").unwrap();
    assert_eq!(s.transaction(), None, "not transaction-initiating");
    count(&s, "SELECT ID FROM T");
    s.rollback().unwrap();
    assert_eq!(s.setting("SESSION.COMMIT_AT_END").unwrap().to_literal(), "'OFF'", "ROLLBACK leaves it");
    let mut p = turborel::params::Params::new().declare("V", turborel::value::DataType::Character { length: 8 }, word("ON"));
    s.execute("SET SESSION.COMMIT_AT_END = V", &mut p).unwrap();
    assert_eq!(s.setting("SESSION.COMMIT_AT_END").unwrap().to_literal(), "'ON'");
    for (sql, refused) in [
        ("SET OPTIMIZER.MAX_PASSES = 3", Code::SyntaxRule),
        ("SET NOT_A.SETTING = 1", Code::SyntaxRule),
        ("SET LOCKS.WAIT = 5", Code::Data),
        ("SET LOCKS.WAIT = 'SOMETIMES'", Code::Data),
        ("SET LOCKS.WAIT 'ON'", Code::Syntax),
        ("SET LOCKS.WAIT = ON", Code::Syntax),
    ] {
        assert_eq!(s.run(sql).err().map(|e| e.code), Some(refused), "{sql}");
    }
    assert_eq!(s.setting("LOCKS.WAIT").unwrap().to_literal(), "'OFF'", "unchanged by the refused ones");
}

/// Verifies: [DESIGN] RULES.md D36: a session's own LOCKS.WAIT decides whether
/// its transactions wait for a lock another holds: one that waits sees the
/// row once the writer commits, while a session of the database's LOCKS.WAIT, OFF,
/// is refused at once (SQLCODE -911).
#[test]
fn a_session_waits_by_its_own_setting() {
    let db = locking_setup();
    let writer = db.session("APP");
    writer.run("INSERT INTO T (ID) VALUES (9)").unwrap();
    assert_eq!(crate::common::rows(&db.session("APP"), "SELECT ID FROM T").err(), Some(Code::Conflict), "OFF");
    let other = db.clone();
    let reader = thread::spawn(move || {
        let s = other.session("APP");
        s.set_setting("LOCKS.WAIT", &word("ON")).unwrap();
        let seen = count(&s, "SELECT ID FROM T");
        s.commit().unwrap();
        seen
    });
    // The reader waits, so it sees the row whenever it reads.
    writer.commit().unwrap();
    assert_eq!(reader.join().unwrap(), 4);
}

/// Verifies: [DESIGN] RULES.md D36, D25: the database's settings
/// are kept with its definitions: they hold when it is opened again, after
/// a clean close or a crash; a plug-in's, replayed before it is declared,
/// is its value once it is.
#[test]
fn database_settings_outlive_the_database() {
    let scratch = Scratch::new("settings-reopen");
    let dir = scratch.path("db");
    let depth = || Definition {
        name: "PLUGIN.DEPTH".into(),
        kind: Kind::Integer { min: 1, max: 9 },
        default: whole(1),
        per_session: false,
        node: false,
        effect: Effect::Statement,
        doc: "How deep.".into(),
    };
    let open = || {
        let db = Database::new();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.declare_setting(depth()).unwrap();
    db.set_setting("PLUGIN.DEPTH", &whole(4)).unwrap();
    db.set_setting("LOCKS.WAIT", &word("ON")).unwrap();
    db.close().unwrap();
    drop(db);
    let db = open();
    assert_eq!(db.setting("LOCKS.WAIT").unwrap().to_literal(), "'ON'", "after a clean close");
    assert_eq!(db.setting("PLUGIN.DEPTH").err().map(|e| e.code), Some(Code::Api), "not declared yet");
    db.declare_setting(depth()).unwrap();
    assert_eq!(db.setting("PLUGIN.DEPTH").unwrap().to_literal(), "4");
    db.set_setting("SESSION.COMMIT_AT_END", &word("OFF")).unwrap();
    let crashed = db;
    let recovered = open();
    drop(crashed);
    assert_eq!(recovered.setting("SESSION.COMMIT_AT_END").unwrap().to_literal(), "'OFF'", "after a crash");
    assert_eq!(recovered.setting("LOCKS.WAIT").unwrap().to_literal(), "'ON'");
}

/// Verifies: [DESIGN] RULES.md D36: what the planner guesses when the
/// statistics do not tell is the database's settings, PLANNER.*: a pattern
/// keeps PLANNER.RANGE_SELECTIVITY of the rows, a third until it is set; a
/// fraction is taken only from 0 to 1, the reordering gain only below 1,
/// whether set or changed in the cost model.
#[test]
fn the_planners_guesses_are_settings() {
    let db = crate::common::big();
    let s = db.session("APP");
    let plan = || s.explain("SELECT ID FROM T WHERE NAME LIKE 'N%'", &turborel::params::Params::new()).unwrap();
    assert!(plan().contains("(est. rows=20)"), "{}", plan());
    db.set_setting("PLANNER.RANGE_SELECTIVITY", &Value::Approx(0.5)).unwrap();
    assert!(plan().contains("(est. rows=30)"), "{}", plan());
    for (name, value) in [("PLANNER.RANGE_SELECTIVITY", 1.5), ("PLANNER.NULL_FRACTION", -0.1), ("PLANNER.REORDER_GAIN", 1.0)] {
        assert_eq!(db.set_setting(name, &Value::Approx(value)).err().map(|e| e.code), Some(Code::Api), "{name} = {value}");
    }
    assert_eq!(db.with_cost_model(|c| c.reorder_gain = 1.0).err().map(|e| e.code), Some(Code::Api));
    db.with_cost_model(|c| c.equal_selectivity = 0.25).unwrap();
    assert_eq!(shown(&db.settings().unwrap(), "PLANNER.EQUAL_SELECTIVITY"), (Value::Approx(0.25).to_literal(), Source::Database));
}

/// Verifies: [DESIGN] RULES.md D36, D25: CATALOG.COMPACT_AFTER_IMAGES is the
/// records a catalog file holds before the next goes alone to the other:
/// at 1, every checkpoint's image goes to the other file.
#[test]
fn the_catalog_compacts_as_its_setting_says() {
    let scratch = Scratch::new("compact-after");
    let dir = scratch.path("db");
    let db = Database::new();
    db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
    let files = || ["catalog.0", "catalog.1"].map(|name| dir.join(name).exists());
    db.checkpoint().unwrap();
    assert_eq!(files(), [true, false], "the default, 64");
    db.set_setting("CATALOG.COMPACT_AFTER_IMAGES", &whole(1)).unwrap();
    db.checkpoint().unwrap();
    assert_eq!(files(), [false, true]);
    db.checkpoint().unwrap();
    assert_eq!(files(), [true, false]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.1, §6.2]; [DESIGN] RULES.md
/// D36, D43: SET DATABASE sets the database's value of any setting, a
/// session's one too, by `=` or TO, a parameter's value among them; DEFAULT
/// gives it its default again; for every session, as `set_setting` does,
/// kept with the definitions, so it holds when the database is opened
/// again. A session's own value stays above it. SET of a setting named
/// DATABASE is a session's still; a name may begin with a key word, as
/// DIAGNOSTICS, a period after it.
#[test]
fn the_owner_sets_the_databases_values_in_sql() {
    let scratch = Scratch::new("set-database");
    let dir = scratch.path("db");
    let db = Database::new();
    db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    db.create_user("ANN", b"").unwrap();
    db.set_owner("ANN").unwrap();
    let ann = db.session("ANN");
    ann.run("SET DATABASE LOCKS.WAIT = 'on'").unwrap();
    ann.run("SET DATABASE optimizer.max_passes TO 3").unwrap();
    let shown = |db: &Database, name: &str| db.setting(name).unwrap().to_string();
    assert_eq!((shown(&db, "LOCKS.WAIT"), shown(&db, "OPTIMIZER.MAX_PASSES")), ("ON".into(), "3".into()));
    let other = db.session("BOB");
    other.run("SET LOCKS.WAIT = 'off'").unwrap();
    let mut params = turborel::params::Params::new().declare(
        "P",
        turborel::value::DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric },
        whole(5),
    );
    ann.execute("SET DATABASE OPTIMIZER.MAX_PASSES = P", &mut params).unwrap();
    assert_eq!(shown(&db, "OPTIMIZER.MAX_PASSES"), "5");
    ann.run("SET DATABASE OPTIMIZER.MAX_PASSES TO DEFAULT").unwrap();
    assert_eq!(shown(&db, "OPTIMIZER.MAX_PASSES"), "64", "its default");
    ann.run("SET DATABASE diagnostics.slow_statement_ms = 50").unwrap();
    assert_eq!(shown(&db, "DIAGNOSTICS.SLOW_STATEMENT_MS"), "50", "a name of a key word, a period after it");
    assert_eq!(
        count(&other, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = 'LOCKS.WAIT' AND VALUE_SOURCE = 'SESSION'"),
        1
    );
    assert_eq!(other.run("SET DATABASE = 1").err().map(|e| e.code), Some(Code::SyntaxRule), "a session's, of a setting DATABASE");
    drop((ann, other));
    db.close().unwrap();
    let again = Database::new();
    again.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
    assert_eq!(shown(&again, "LOCKS.WAIT"), "ON", "kept");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.1, §6.2]; [DESIGN] RULES.md
/// D36, D43, R-ERR-2: SET DATABASE is refused, nothing set: by any but the
/// owner (SQLCODE -104), or with none; in a transaction (-917); of a
/// setting not there (-103); of a value not of the setting's kind or
/// range (-301); and as a Format broken (-102) without `=` or TO, or
/// without a value.
#[test]
fn set_database_is_refused_saying_why() {
    let db = Database::new();
    let ann = db.session("ANN");
    let refused = |sql: &str| ann.run(sql).err().map(|e| e.code);
    assert_eq!(refused("SET DATABASE LOCKS.WAIT = 'ON'"), Some(Code::Privilege), "no owner");
    db.create_user("ANN", b"").unwrap();
    db.set_owner("ANN").unwrap();
    assert_eq!(db.session("BOB").run("SET DATABASE LOCKS.WAIT = 'ON'").err().map(|e| e.code), Some(Code::Privilege));
    assert_eq!(refused("SET DATABASE NO_SUCH = 1"), Some(Code::SyntaxRule));
    assert_eq!(refused("SET DATABASE LOCKS.WAIT = 'SOMETIMES'"), Some(Code::Data));
    assert_eq!(refused("SET DATABASE OPTIMIZER.MAX_PASSES = 'many'"), Some(Code::Data));
    assert_eq!(refused("SET DATABASE LOCKS.WAIT 'ON'"), Some(Code::Syntax));
    assert_eq!(refused("SET DATABASE LOCKS.WAIT ="), Some(Code::Syntax));
    db.schema("CREATE SCHEMA AUTHORIZATION ANN CREATE TABLE T (K INTEGER)").unwrap();
    ann.run("INSERT INTO T VALUES (1)").unwrap();
    assert_eq!(refused("SET DATABASE LOCKS.WAIT = 'ON'"), Some(Code::TransactionState));
    ann.rollback().unwrap();
    assert_eq!(db.setting("LOCKS.WAIT").unwrap().to_string(), "OFF", "nothing set");
}

/// A node's setting of the tests, as a plug-in declares one: text, taken
/// as what reads it starts; `session` and `effect` as given.
fn node_setting(session: bool, effect: Effect) -> Definition {
    Definition {
        name: "PLUGIN.HOST".into(),
        kind: Kind::Text { max: 20 },
        default: word(""),
        per_session: session,
        node: true,
        effect,
        doc: "Where this node listens.".into(),
    }
}

/// Verifies: [DESIGN] RULES.md D36, D57, a node's setting: one a session
/// sets, or fixed at creation, is refused (SQLCODE -910); given before the
/// database opens, as STORAGE.POOL_BYTES is, it holds while it runs, shown
/// with VALUE_SOURCE NODE; SET DATABASE changes it for the running
/// database alone; it is in neither the log nor a checkpoint's image,
/// which a database's own setting is in, so it survives no reopening, after
/// a clean close or a crash: it is its default again, while the database's
/// own setting holds.
#[test]
fn a_node_setting_lasts_while_the_node_runs() {
    let db = Database::new();
    for (session, effect) in [(true, Effect::Statement), (false, Effect::Creation)] {
        assert_eq!(db.declare_setting(node_setting(session, effect)).err().map(|e| e.code), Some(Code::Api), "{effect:?}");
    }
    let scratch = Scratch::new("settings-node");
    let dir = scratch.path("db");
    let open = |given: Option<&str>| {
        let db = Database::new();
        db.declare_setting(node_setting(false, Effect::Start)).unwrap();
        if let Some(host) = given {
            db.set_setting("PLUGIN.HOST", &word(host)).unwrap();
        }
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let source = |db: &Database| {
        let sql = "SELECT TRIM(SETTING_VALUE), TRIM(VALUE_SOURCE) FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = 'PLUGIN.HOST'";
        crate::common::rows(&db.session("OWN"), sql).unwrap()
    };
    let db = open(Some("node-one"));
    assert_eq!(source(&db), ["node-one,NODE"]);
    db.create_user("OWN", b"x").unwrap();
    db.set_owner("OWN").unwrap();
    db.session("OWN").run("SET DATABASE PLUGIN.HOST = 'node-two'").unwrap();
    db.session("OWN").run("SET DATABASE LOCKS.WAIT = 'ON'").unwrap();
    assert_eq!(source(&db), ["node-two,NODE"]);
    db.close().unwrap();
    drop(db);
    for (files, what) in [("log", "the log"), ("catalog", "an image")] {
        let bytes = crate::common::bytes_of(&dir, files);
        assert!(!crate::common::holds(&bytes, "PLUGIN.HOST"), "not in {what}");
        assert!(crate::common::holds(&bytes, "LOCKS.WAIT"), "the database's own is in {what}");
    }
    let db = open(None);
    assert_eq!(source(&db), [",DEFAULT"], "its default again, after a clean close");
    assert_eq!(db.setting("LOCKS.WAIT").unwrap().to_string(), "ON");
    db.set_setting("PLUGIN.HOST", &word("node-three")).unwrap();
    let crashed = db;
    let db = open(None);
    drop(crashed);
    assert_eq!(source(&db), [",DEFAULT"], "and after a crash");
    assert_eq!(db.settings().unwrap().iter().find(|l| l.definition.name == "PLUGIN.HOST").map(|l| l.source), Some(Source::Default));
}
