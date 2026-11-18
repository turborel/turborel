//! VECTOR columns and distance functions kept by a checkpoint and the log.

use turborel::db::{Database, OpenOptions};

use crate::common::{Scratch, after_crash, rows};

/// Verifies: [EXTENSION] vector_standard §4.3 R4.6, RULES.md D25, D53:
/// vectors of each format and layout, fixed and flexible columns, and a
/// distance function of SQL survive a checkpoint and log recovery; an
/// uncommitted change does not.
#[test]
fn vectors_survive_checkpoint_and_recovery() {
    let scratch = Scratch::new("vector-recovery");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (ID INTEGER, F VECTOR(2, FLOAT16), O VECTOR, S VECTOR(9, INT8, SPARSE))")
        .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T VALUES (1, '[1.5, -2]', '[0.25, 0.5, 1]', '[9, [8], [-3]]')").unwrap();
    a.run(
        "CREATE VECTOR DISTANCE FUNCTION A.TWICE (X VECTOR, Y VECTOR) RETURNS FLOAT64 DETERMINISTIC LANGUAGE SQL \
         AS 'L2_DISTANCE(X, Y) * 2'",
    )
    .unwrap();
    a.commit().unwrap();
    db.checkpoint().unwrap();
    a.run("INSERT INTO T VALUES (2, '[3, 4]', CAST('[7]' AS VECTOR(1, FLOAT64)), NULL)").unwrap();
    a.commit().unwrap();
    a.run("UPDATE T SET F = '[0, 0]' WHERE ID = 1").unwrap();
    let crashed = a;
    drop(db);
    let db = open();
    let a = db.session("A");
    assert_eq!(
        rows(&a, "SELECT ID, FROM_VECTOR(F), FROM_VECTOR(O), VECTOR_DIMENSION_FORMAT(O), FROM_VECTOR(S) FROM T ORDER BY ID").unwrap(),
        ["1,[1.5,-2],[0.25,0.5,1],FLOAT32,[9,[8],[-3]]", "2,[3,4],[7],FLOAT64,NULL"]
    );
    let twice = crate::vectors::number(&a, "SELECT VECTOR_DISTANCE(CAST(F AS VECTOR(2)), '[0, 0]', A.TWICE) FROM T WHERE ID = 2");
    assert!(crate::vectors::near(twice, 10.0));
    drop(crashed);
}
