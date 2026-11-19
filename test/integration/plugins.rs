//! \[EXTENSION\] Extensions of external plug-ins (RULES.md D167 to D173),
//! of the example plug-in (`examples/plugin_example.rs`): CREATE, ALTER
//! ... UPDATE and DROP EXTENSION, each kind of object called, the views
//! of extensions and EXPLAIN. What a program that fails does is
//! `plugin_failures.rs`'s.

pub mod fixture;

use fixture::{install, manifest, plugged, running};
use turborel::error::Code;

use crate::common::{exec, number, rows};

/// Verifies: [EXTENSION] RULES.md D168, D169: CREATE EXTENSION found by
/// name makes its functions; a scalar function's rows are sent BATCH at
/// a time, so 1000 rows are 10 calls; EXTENSIONS shows its version and
/// processes, EXTENSION_OBJECTS each object.
#[test]
fn scalar_functions_are_called_in_batches() {
    let (_scratch, db) = plugged("plugins-scalar");
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    s.run("CREATE TABLE T (N INTEGER)").unwrap();
    s.run("INSERT INTO T SELECT N FROM TABLE(SERIES(1000)) AS S").unwrap();
    let calls = "SELECT CALLS FROM INFORMATION_SCHEMA.EXTENSIONS";
    let before: u64 = rows(&s, calls).unwrap()[0].parse().unwrap();
    let twice = rows(&s, "SELECT TWICE(N), SHOUT('abc') FROM T ORDER BY N").unwrap();
    assert_eq!((twice.len(), twice[999].as_str()), (1000, "2000,ABC"));
    let after: u64 = rows(&s, calls).unwrap()[0].parse().unwrap();
    assert!(after - before <= 25, "batched: {} calls", after - before);
    assert_eq!(rows(&s, "VALUES (TWICE(CAST(NULL AS INTEGER)), TWICE(21))").unwrap(), ["NULL,42"]);
    let shown = "SELECT EXTENSION_NAME, VERSION, SIGNER IS NULL, PROCESSES_RUNNING FROM INFORMATION_SCHEMA.EXTENSIONS";
    assert_eq!(rows(&s, shown).unwrap(), ["EXAMPLE,1.0.0,TRUE,1"]);
    let objects = "SELECT OBJECT_TYPE, OBJECT_NAME, IS_DETERMINISTIC, BATCH_SIZE FROM INFORMATION_SCHEMA.EXTENSION_OBJECTS WHERE OBJECT_NAME IN ('TWICE', 'PID', 'TAXICAB') ORDER BY 2";
    assert_eq!(rows(&s, objects).unwrap(), ["FUNCTION,PID,NO,NULL", "METRIC,TAXICAB,NO,NULL", "FUNCTION,TWICE,YES,100"]);
}

/// Verifies: [EXTENSION] RULES.md D169: an aggregate's state is kept by
/// the database between steps, grouped; a table function makes rows, and
/// one that READS SQL DATA is given the rows of the queries it names.
#[test]
fn aggregates_and_table_functions() {
    let (_scratch, db) = plugged("plugins-aggregate");
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    s.run("CREATE TABLE T (G INTEGER, X DOUBLE PRECISION)").unwrap();
    s.run("INSERT INTO T VALUES (1, 2), (1, 3), (2, 5), (2, NULL), (3, NULL)").unwrap();
    assert_eq!(rows(&s, "SELECT G, PRODUCT(X) FROM T GROUP BY G ORDER BY G").unwrap(), ["1,6E0", "2,5E0", "3,NULL"]);
    assert_eq!(rows(&s, "SELECT N FROM TABLE(SERIES(3)) AS S ORDER BY N").unwrap(), ["1", "2", "3"]);
    assert_eq!(rows(&s, "SELECT TABLE_NAME, N FROM TABLE(ROW_COUNT('U.T')) AS C").unwrap(), ["U.T,5"]);
}

/// Verifies: [EXTENSION] RULES.md D169: a procedure's SQL runs in the
/// call's transaction, so it rolls back with it; one that does not say
/// MODIFIES SQL DATA may not change data, SQLSTATE 2F002.
#[test]
fn procedures_run_sql_in_the_transaction() {
    let (_scratch, db) = plugged("plugins-procedure");
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    s.run("CREATE TABLE NOTES (X VARCHAR(200))").unwrap();
    s.commit().unwrap();
    s.run("CALL NOTE('kept')").unwrap();
    s.commit().unwrap();
    s.run("CALL NOTE('undone')").unwrap();
    s.rollback().unwrap();
    assert_eq!(rows(&s, "SELECT X FROM NOTES").unwrap(), ["kept"]);
    assert_eq!(s.run("CALL PEEK('no')").unwrap_err().sqlstate().as_str(), "2F002");
}

/// Verifies: [EXTENSION] RULES.md D169: an extension's metric names
/// VECTOR_DISTANCE, one call a batch of rows, and a DiskANN index over
/// it finds the nearest rows; its embedding model embeds.
#[test]
fn metrics_and_models() {
    let (_scratch, db) = plugged("plugins-vectors");
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    s.run("CREATE TABLE V (K INTEGER PRIMARY KEY, E VECTOR(2))").unwrap();
    for k in 0..200 {
        s.run(&format!("INSERT INTO V VALUES ({k}, '[{k}, {}]')", k % 7)).unwrap();
    }
    let near = "SELECT K FROM V ORDER BY VECTOR_DISTANCE(E, '[50, 1]', TAXICAB), K FETCH FIRST 2 ROWS ONLY";
    assert_eq!(rows(&s, near).unwrap(), ["50", "49"]);
    assert_eq!(rows(&s, "SELECT VECTOR_DISTANCE(E, '[4, 6]', TAXICAB) FROM V WHERE K = 1").unwrap(), ["8E0"]);
    s.commit().unwrap();
    s.run("CREATE INDEX V_E ON V (E) USING DISKANN WITH (DISKANN.METRIC = TAXICAB)").unwrap();
    let approx = "SELECT K FROM V ORDER BY VECTOR_DISTANCE(E, '[120, 1]', TAXICAB) FETCH APPROX FIRST 3 ROWS ONLY";
    let found = rows(&s, approx).unwrap();
    assert!(found.contains(&"120".to_owned()), "the index finds the nearest: {found:?}");
    s.run("CREATE EMBEDDING MODEL W WITH (TYPE = 'VOWELS')").unwrap();
    assert_eq!(rows(&s, "VALUES (EMBED('W', 'Queueing'))").unwrap(), ["[0,2,1,0,2]"]);
}

/// Verifies: [EXTENSION] RULES.md D169: an extension's authenticator,
/// backup target and partitioning strategy are found by name, each a
/// conversation with its program.
#[test]
fn authenticators_targets_and_strategies() {
    use turborel::backup::TargetProvider;
    use turborel::plugin::bridge::auth::{AuthStep, Connecting};
    use turborel::plugin::bridge::pending::{PartitionBound, PartitionStrategy};
    use turborel::value::{DataType, Spelled};
    let (scratch, db) = plugged("plugins-points");
    db.session("U").run("CREATE EXTENSION example").unwrap();
    let auth = db.extension_authenticator("EXAMPLE").unwrap().unwrap();
    let mut exchange = auth.begin(&Connecting { user: "U", peer: "here", local: true, secure: true }).unwrap();
    let AuthStep::Challenge(challenge) = exchange.step(b"").unwrap() else { panic!("a challenge first") };
    assert_eq!(challenge, b"say the word");
    assert!(matches!(exchange.step(b"open sesame").unwrap(), AuthStep::Accept));
    let target = db.extension_target("EXAMPLE").unwrap().unwrap().open(&scratch.path("backups").display().to_string()).unwrap();
    let upload = target.begin("base.1").unwrap();
    target.write(upload, b"pages").unwrap();
    target.finish(upload).unwrap();
    assert_eq!(target.read("base.1", 1, 3).unwrap(), b"age");
    assert_eq!(target.list("base").unwrap(), ["base.1"]);
    let strategy = db.extension_strategy("modulus").unwrap().unwrap();
    let with = |r: i128| PartitionBound::With(vec![("MODULUS".into(), number(4)), ("REMAINDER".into(), number(r))]);
    let int = [DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer }];
    strategy.check_key(&int).unwrap();
    let bounds = [strategy.bound(&int, &with(1)).unwrap(), strategy.bound(&int, &with(3)).unwrap()];
    assert_eq!(strategy.route(&[&bounds[0], &bounds[1]], &[number(7)]).unwrap(), Some(1));
    assert!(!strategy.overlaps(&bounds[0], &bounds[1]).unwrap());
    assert!(strategy.bound(&int, &with(9)).is_err(), "a remainder of 9 of 4");
}

/// Verifies: [EXTENSION] RULES.md D168, D172: DROP EXTENSION RESTRICT is
/// refused while a view calls its function, CASCADE drops the view; its
/// processes stop and its functions are gone; EXPLAIN names a call.
#[test]
fn drop_restrict_and_cascade() {
    let (_scratch, db) = plugged("plugins-drop");
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    s.run("CREATE VIEW W AS SELECT TWICE(2) AS D FROM (VALUES (1)) AS O (X)").unwrap();
    let plan = s.explain("SELECT SHOUT('x'), D FROM W", &turborel::params::Params::new()).unwrap();
    assert!(plan.contains("plug-in calls: EXAMPLE.TWICE, EXAMPLE.SHOUT"), "{plan}");
    assert_eq!(rows(&s, "SELECT D FROM W").unwrap(), ["4"]);
    assert_eq!(running(&s, "EXAMPLE"), "1");
    assert_eq!(exec(&s, "DROP EXTENSION example RESTRICT"), Err(Code::SyntaxRule));
    s.run("DROP EXTENSION example CASCADE").unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_NAME = 'W'").unwrap(), ["0"]);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.EXTENSIONS").unwrap(), ["0"]);
    assert!(exec(&s, "VALUES (TWICE(1))").is_err(), "gone with it");
}

/// Verifies: [EXTENSION] RULES.md D168: an extension created in a
/// transaction rolled back leaves nothing, its name free again.
#[test]
fn a_rolled_back_extension_leaves_nothing() {
    let (_scratch, db) = plugged("plugins-rollback");
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    assert_eq!(rows(&s, "VALUES (TWICE(4))").unwrap(), ["8"]);
    s.rollback().unwrap();
    assert!(exec(&s, "VALUES (TWICE(4))").is_err());
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM INFORMATION_SCHEMA.EXTENSIONS").unwrap(), ["0"]);
    s.run("CREATE EXTENSION example").unwrap();
    assert_eq!(exec(&s, "CREATE EXTENSION example"), Err(Code::SyntaxRule), "once");
}

/// Verifies: [EXTENSION] RULES.md D171: REQUIRES makes the extensions
/// required first, AS DEPENDENCY, and DROP RESTRICT of one required is
/// refused; ALTER EXTENSION ... UPDATE takes the newest version
/// installed and runs its upgrade script in the transaction.
#[test]
fn requirements_and_updates() {
    let (scratch, db) = plugged("plugins-update");
    let root = scratch.path("plugins");
    let base = "FUNCTION SHOUT2(VARCHAR(200)) RETURNS VARCHAR(200);";
    install(&root, ("NEEDS", "1.0.0"), &manifest("NEEDS", "1.0.0", "REQUIRES EXAMPLE '^1.0';", base));
    let s = db.session("U");
    s.run("CREATE EXTENSION needs").unwrap();
    let lock = "SELECT PACKAGE_NAME, PACKAGE_VERSION, IS_EXPLICIT, REQUIRES FROM INFORMATION_SCHEMA.EXTENSION_PACKAGES ORDER BY 1";
    assert_eq!(rows(&s, lock).unwrap(), ["EXAMPLE,1.0.0,NO,", "NEEDS,1.0.0,YES,EXAMPLE ^1.0"]);
    assert_eq!(exec(&s, "DROP EXTENSION example RESTRICT"), Err(Code::SyntaxRule));
    s.run("CREATE TABLE LOG (X VARCHAR(20))").unwrap();
    let dir = install(
        &root,
        ("NEEDS", "1.1.0"),
        &manifest("NEEDS", "1.1.0", "REQUIRES EXAMPLE '^1.0'; UPGRADE FROM '<1.1.0' SCRIPT 'up.sql';", base),
    );
    std::fs::write(dir.join("up.sql"), "INSERT INTO U.LOG VALUES ('up; one');\nINSERT INTO U.LOG VALUES ('two')").unwrap();
    s.run("ALTER EXTENSION needs UPDATE").unwrap();
    assert_eq!(rows(&s, "SELECT X FROM LOG ORDER BY X").unwrap(), ["two", "up; one"]);
    assert_eq!(rows(&s, "SELECT VERSION FROM INFORMATION_SCHEMA.EXTENSIONS WHERE EXTENSION_NAME = 'NEEDS'").unwrap(), ["1.1.0"]);
}

/// Verifies: [EXTENSION] RULES.md D166, D169: an extension's scheme of
/// targets is a provider of the database's own, so BACKUP DATABASE writes
/// to a location of it; registered with a fresh database, the same
/// provider restores the backup, which opens with its rows.
#[test]
fn a_backup_and_restore_through_an_extensions_target() {
    use turborel::backup::TargetProvider;
    use turborel::backup::seed::RecoveryTarget;
    use turborel::db::OpenOptions;
    use turborel::db::backup::{BackupOptions, RestoreOptions};
    let scratch = crate::common::Scratch::new("plugins-backup");
    let root = scratch.path("plugins");
    install(&root, ("EXAMPLE", "1.0.0"), &manifest("EXAMPLE", "1.0.0", "", fixture::OBJECTS));
    let db = turborel::db::Database::new();
    fixture::allow(&db, &root);
    db.open_files(&crate::common::os(), &scratch.path("db"), &OpenOptions::default()).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION U CREATE TABLE T (K INTEGER)").unwrap();
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    s.run("INSERT INTO T VALUES (7)").unwrap();
    s.commit().unwrap();
    let location = format!("example:{}", scratch.path("store").display());
    db.backup(&BackupOptions { location: location.clone(), ..BackupOptions::default() }).unwrap();
    assert!(scratch.path("store").join("backups").is_dir(), "written through the plug-in");
    let provider: std::sync::Arc<dyn TargetProvider> = db.extension_target("example").unwrap().unwrap();
    let restorer = turborel::db::Database::new();
    restorer.register_target_provider(provider).unwrap();
    let dir = scratch.path("back");
    let options =
        RestoreOptions { dir: dir.clone(), location, backup: String::new(), archive: None, target: RecoveryTarget::End, timeline: None };
    restorer.restore(&options).unwrap();
    drop((s, db, restorer));
    let back = turborel::db::Database::new();
    fixture::allow(&back, &root);
    back.open_files(&crate::common::os(), &dir, &OpenOptions::default()).unwrap();
    assert_eq!(rows(&back.session("U"), "SELECT K FROM T").unwrap(), ["7"]);
}
