//! The defaults of the columns of a view an INSTEAD OF trigger is given
//! rows of: each column's counterpart column's default [ISO/IEC
//! 9075-2:2023 §11.5 GR3d], its own or its domain's, none of one whose
//! counterpart is an identity or generated column [GR3a], and none of one
//! with no single counterpart [GR3e]; by an INSERT, an UPDATE's SET
//! DEFAULT, a MERGE, and an INSERT through a view over the trigger's
//! [§15.12 GR2b iii 1] (Feature T213).

use turborel::db::{Database, Session};

/// Schema A: domain D of default 9; T, whose K, N, Q and S have defaults,
/// ID an identity column and G a generated one, with row (1, 10); V, a
/// read-only view of T's columns and of P, an expression; L, a log in
/// the order its rows came, by SEQ.
fn defaulted() -> Database {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE DOMAIN D AS INTEGER DEFAULT 9
           CREATE TABLE T (K INTEGER DEFAULT 5, N INTEGER DEFAULT 7, Q D, S VARCHAR(128) DEFAULT USER,
             ID INTEGER GENERATED ALWAYS AS IDENTITY, G INTEGER GENERATED ALWAYS AS (N * 2))
           CREATE VIEW V AS SELECT DISTINCT K, N, Q, S, N + 1 AS P, ID, G FROM T
           CREATE TABLE L (SEQ INTEGER GENERATED ALWAYS AS IDENTITY, WHAT VARCHAR(10), K INTEGER, N INTEGER,
             Q INTEGER, S VARCHAR(128), P INTEGER, ID INTEGER, G INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("INSERT INTO T (K, N) VALUES (1, 10)").unwrap();
    a.run(
        "CREATE TRIGGER I_V INSTEAD OF INSERT ON V REFERENCING NEW AS NW FOR EACH ROW
           INSERT INTO L (WHAT, K, N, Q, S, P, ID, G) VALUES ('INS', NW.K, NW.N, NW.Q, NW.S, NW.P, NW.ID, NW.G)",
    )
    .unwrap();
    a.run(
        "CREATE TRIGGER U_V INSTEAD OF UPDATE ON V REFERENCING NEW AS NW FOR EACH ROW
           INSERT INTO L (WHAT, K, N, Q, S, P, ID, G) VALUES ('UPD', NW.K, NW.N, NW.Q, NW.S, NW.P, NW.ID, NW.G)",
    )
    .unwrap();
    a.commit().unwrap();
    db
}

/// The rows of `query` in `s`, each its values as text joined by commas.
fn rows(s: &Session, query: &str) -> Vec<String> {
    let mut c = s.open_cursor(query).unwrap();
    let mut out = Vec::new();
    while let Some(row) = c.fetch().unwrap() {
        out.push(row.iter().map(|v| v.to_string().trim_end().to_owned()).collect::<Vec<_>>().join(","));
    }
    out
}

/// L's rows, in the order they came, without SEQ.
fn logged(s: &Session) -> Vec<String> {
    rows(s, "SELECT WHAT, K, N, Q, S, P, ID, G FROM L ORDER BY SEQ")
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.5 GR3a, GR3d, GR3e,
/// §15.13 GR3a] an INSERT into a read-only view its INSTEAD OF trigger
/// takes gives each column it names not, or gives DEFAULT, its
/// counterpart column's default, its own or its domain's; a column of no
/// counterpart, or whose counterpart is an identity or generated column,
/// null; and writes nothing to the base table.
#[test]
fn an_insert_a_trigger_takes_gives_each_column_not_named_its_counterparts_default() {
    let db = defaulted();
    let a = db.session("A");
    a.run("INSERT INTO V (N) VALUES (1)").unwrap();
    a.run("INSERT INTO V (K, N, Q) VALUES (DEFAULT, DEFAULT, DEFAULT)").unwrap();
    a.run("INSERT INTO V DEFAULT VALUES").unwrap();
    assert_eq!(
        logged(&a),
        vec!["INS,5,1,9,A,NULL,NULL,NULL", "INS,5,7,9,A,NULL,NULL,NULL", "INS,5,7,9,A,NULL,NULL,NULL"],
        "K, N and S of T's defaults, Q of its domain's; P of no counterpart, ID and G unassigned"
    );
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM T"), vec!["1"], "nothing is written to T");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.5 GR3d, §15.16 GR3a]
/// SET DEFAULT of an UPDATE of a view its INSTEAD OF trigger takes gives
/// the new row each column's counterpart column's default, null of a
/// column of none, the columns not set as they were.
#[test]
fn set_default_gives_a_column_its_counterparts_default() {
    let db = defaulted();
    let a = db.session("A");
    a.run("UPDATE V SET N = DEFAULT, Q = DEFAULT, P = DEFAULT").unwrap();
    assert_eq!(logged(&a), vec!["UPD,1,7,9,A,NULL,1,20"], "N of T's default, Q of its domain's, P of none; ID and G kept");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.5 GR3d, §14.12 GR6a]
/// a MERGE into a read-only view its INSTEAD OF triggers take gives SET
/// DEFAULT, DEFAULT among its insert values and each column its insert
/// names not their counterpart columns' defaults.
#[test]
fn a_merge_into_a_view_its_triggers_take_gives_its_defaults() {
    let db = defaulted();
    let a = db.session("A");
    let merge = "MERGE INTO V USING (VALUES (1), (3)) AS S (X) ON V.K = S.X
                   WHEN MATCHED THEN UPDATE SET N = DEFAULT
                   WHEN NOT MATCHED THEN INSERT (K, N) VALUES (DEFAULT, S.X)";
    a.run(merge).unwrap();
    assert_eq!(logged(&a), vec!["UPD,1,7,9,A,11,1,20", "INS,5,3,9,A,NULL,NULL,NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §11.5 GR3d, GR3e, §4.26.2]
/// each column of a view of a join takes the default of its counterpart
/// in either table, and a column of a set operation, specified by no
/// column reference, has none.
#[test]
fn a_view_of_a_join_takes_each_columns_default_and_one_of_a_union_none() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER DEFAULT 5)
           CREATE TABLE U (Z INTEGER DEFAULT 6)
           CREATE VIEW J AS SELECT T.K, U.Z FROM T, U
           CREATE VIEW E (K) AS SELECT K FROM T UNION SELECT Z FROM U
           CREATE TABLE L (SEQ INTEGER GENERATED ALWAYS AS IDENTITY, WHAT VARCHAR(10), K INTEGER, Z INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run(
        "CREATE TRIGGER I_J INSTEAD OF INSERT ON J REFERENCING NEW AS NW FOR EACH ROW INSERT INTO L (WHAT, K, Z) VALUES ('J', NW.K, NW.Z)",
    )
    .unwrap();
    a.run("CREATE TRIGGER I_E INSTEAD OF INSERT ON E REFERENCING NEW AS NW FOR EACH ROW INSERT INTO L (WHAT, K) VALUES ('E', NW.K)")
        .unwrap();
    a.run("INSERT INTO J DEFAULT VALUES").unwrap();
    a.run("INSERT INTO E DEFAULT VALUES").unwrap();
    assert_eq!(rows(&a, "SELECT WHAT, K, Z FROM L ORDER BY SEQ"), vec!["J,5,6", "E,NULL,NULL"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §15.12 GR2b iii 1, §11.5
/// GR3d] an INSERT or a MERGE through an updatable view over a view with
/// an INSTEAD OF trigger gives each column of the trigger's view it lacks
/// that column's default.
#[test]
fn an_insert_through_a_view_over_a_trigger_view_gives_its_other_columns_their_defaults() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER DEFAULT 5, N INTEGER)
           CREATE VIEW V AS SELECT K, N FROM T
           CREATE VIEW W AS SELECT N FROM V
           CREATE TABLE L (SEQ INTEGER GENERATED ALWAYS AS IDENTITY, WHAT VARCHAR(10), K INTEGER, N INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    a.run("CREATE TRIGGER I_V INSTEAD OF INSERT ON V REFERENCING NEW AS NW FOR EACH ROW INSERT INTO L (WHAT, K, N) VALUES ('INS', NW.K, NW.N)")
        .unwrap();
    a.run("INSERT INTO W VALUES (1)").unwrap();
    a.run("MERGE INTO W USING (VALUES (2)) AS S (X) ON W.N = S.X WHEN NOT MATCHED THEN INSERT VALUES (S.X)").unwrap();
    assert_eq!(rows(&a, "SELECT WHAT, K, N FROM L ORDER BY SEQ"), vec!["INS,5,1", "INS,5,2"]);
}
