//! Index options replayed from the log (RULES.md D41, D24): ALTER INDEX of
//! an option changed in place and of one fixed when the index is made,
//! committed and then recovered by a database not closed, the index made
//! anew of its table's rows as it is replayed.

use turborel::db::{Database, OpenOptions};
use turborel::index::BTreeMethod;

use crate::common::{Scratch, exec, os, rows};
use crate::table_settings::{Calls, Noted};

/// Verifies: [EXTENSION] RULES.md D41, D24: a database not closed replays
/// its log; an ALTER INDEX of an option fixed when the index is made gives
/// the index made anew of its table's rows, one of an option changed in
/// place gives it the option told; INDEX_OPTIONS lists both, and the index
/// finds the rows it held.
#[test]
fn index_options_are_replayed_from_the_log() {
    let scratch = Scratch::new("index-replay");
    let dir = scratch.path("db");
    let calls = Calls::default();
    let open = || {
        let db = Database::new();
        db.register_index_method(Box::new(Noted(BTreeMethod::in_memory(), calls.clone()))).unwrap();
        db.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER, V INTEGER)").unwrap();
    let s = db.session("S");
    for k in 0..30 {
        s.run(&format!("INSERT INTO T VALUES ({k}, {})", k % 3)).unwrap();
    }
    s.commit().unwrap();
    exec(&s, "CREATE INDEX T_K ON T (K) USING NOTED WITH (FILL = 60)").unwrap();
    exec(&s, "ALTER INDEX T_K SET (SHAPE = TALL)").unwrap();
    exec(&s, "ALTER INDEX T_K SET (FILL = 40)").unwrap();
    s.run("INSERT INTO T VALUES (100, 1)").unwrap();
    s.commit().unwrap();
    drop(s);
    drop(db);
    calls.taken();
    let db = open();
    let replayed = calls.taken();
    assert!(replayed.iter().any(|c| c.contains("SHAPE=TALL")), "{replayed:?}");
    let s = db.session("S");
    let options = "SELECT OPTION_NAME, OPTION_VALUE FROM INFORMATION_SCHEMA.INDEX_OPTIONS WHERE INDEX_NAME = 'T_K' ORDER BY 1";
    assert_eq!(rows(&s, options).unwrap(), ["FILL,40", "SHAPE,TALL"]);
    assert_eq!(rows(&s, "SELECT V FROM T WHERE K = 100").unwrap(), ["1"]);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM T WHERE K BETWEEN 10 AND 19").unwrap(), ["10"]);
}
