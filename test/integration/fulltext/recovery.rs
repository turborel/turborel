//! FULLTEXT and FT_PATTERN columns kept by a checkpoint and the log.

use turborel::db::{Database, OpenOptions};

use crate::common::{Scratch, after_crash, rows};

/// Verifies: [EXTENSION] RULES.md D25, D53: FULLTEXT values, their
/// languages, a text of FT_MaxTextLength and FT_PATTERN values survive a
/// checkpoint and log recovery, an uncommitted change does not.
#[test]
fn fulltext_survives_checkpoint_and_recovery() {
    let scratch = Scratch::new("sqlmm-fulltext-recovery");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE DOCS (ID INTEGER, BODY FULLTEXT, P FT_PATTERN)").unwrap();
    let a = db.session("A");
    a.run("INSERT INTO DOCS VALUES (1, FULLTEXT('Red foxes run', 'NONE'), '\"fox\"')").unwrap();
    a.commit().unwrap();
    db.checkpoint().unwrap();
    let long = "word ".repeat(6_552);
    a.run(&format!("INSERT INTO DOCS VALUES (2, CAST('{long}fox' AS FULLTEXT), NULL)")).unwrap();
    a.commit().unwrap();
    a.run("UPDATE DOCS SET BODY = FULLTEXT('gone') WHERE ID = 1").unwrap();
    db.checkpoint().unwrap();
    let crashed = a;
    drop(db);
    let db = open();
    let a = db.session("A");
    assert_eq!(
        rows(
            &a,
            "SELECT ID, BODY.\"LANGUAGE\", CHARACTER_LENGTH(FULLTEXT_TO_CHARACTER(BODY)), CAST(P AS VARCHAR(10)) FROM DOCS ORDER BY ID"
        )
        .unwrap(),
        ["1,NONE,13,\"fox\"", "2,ENGLISH,32763,NULL"]
    );
    assert_eq!(rows(&a, "SELECT ID FROM DOCS WHERE CONTAINS(BODY, '\"fox\"') = 1 ORDER BY ID").unwrap(), ["2"]);
    a.rollback().unwrap();
    db.close().unwrap();
    drop((a, db));
    let db = open();
    assert_eq!(rows(&db.session("A"), "SELECT COUNT(*) FROM DOCS").unwrap(), ["2"]);
    drop(crashed);
}
