//! \[EXTENSION\] A host plug-in's log, catalog, spill and file system
//! (RULES.md D248, D250, D251): a database kept by the program, opened
//! again from it, and the program killed mid-commit, mid-recovery and
//! mid-page-write, the node stopped each time and every commit synced
//! given back after.

use turborel::db::{Database, OpenOptions};
use turborel::value::Value;

use super::{crash, hosted, started};
use crate::common::{exec, rows};

/// The host plug-in's stores.
const STORES: &str = "HOST LOG STORAGE FILELOG (BATCH = 16); HOST CATALOG STORAGE FILECAT; HOST SPILL MEMORY; HOST FILE SYSTEM PASS;";

/// `db` opened from `dir` on its file system: the host plug-in's.
fn opened(db: &Database, dir: &std::path::Path) -> turborel::error::Result<()> {
    db.open_files(&db.file_system()?, dir, &OpenOptions::default()).map(drop)
}

/// A table of `n` rows committed one transaction each.
fn filled(db: &Database, n: i32) {
    db.schema("CREATE SCHEMA AUTHORIZATION U CREATE TABLE T (K INTEGER, S VARCHAR(200))").unwrap();
    let s = db.session("U");
    for k in 0..n {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'row {k}')")).unwrap();
        s.commit().unwrap();
    }
}

/// Verifies: [EXTENSION] RULES.md D248, D251, D229: a database whose
/// log, catalog and files a host plug-in keeps is opened again from them;
/// the program killed halfway through writing a commit's records stops
/// the node, the commit in doubt; killed as recovery reads the log, the
/// opening fails; and the next opening gives back every commit synced
/// before, none of the one it died in.
#[test]
fn a_host_plugins_log_survives_kills_mid_commit_and_mid_recovery() {
    let (scratch, plugin, spec) = hosted("host_log", STORES);
    let dir = scratch.path("db");
    let db = started(&spec);
    opened(&db, &dir).unwrap();
    filled(&db, 40);
    let kept: Vec<String> = std::fs::read_dir(&plugin).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(kept.iter().any(|n| n.starts_with("log-")) && kept.iter().any(|n| n.starts_with("catalog-")), "{kept:?}");
    assert!(!dir.join(turborel::db::LOG_DIR).exists(), "the log is the program's, not in the directory");
    let s = db.session("U");
    s.run("INSERT INTO T VALUES (40, 'never')").unwrap();
    crash(&plugin, "append", true);
    let refused = s.commit().unwrap_err();
    assert_eq!(refused.sqlstate().as_str(), "08007", "in doubt until recovery: {refused:?}");
    assert!(refused.msg.contains("HOSTED failed"), "{refused:?}");
    assert!(db.host_stopped().unwrap().is_some());
    drop((s, db));
    crash(&plugin, "append", false);

    crash(&plugin, "read", true);
    let dying = started(&spec);
    let refused = opened(&dying, &dir).unwrap_err();
    assert_eq!(refused.sqlstate().as_str(), "38T11", "killed mid-recovery: {refused:?}");
    drop(dying);
    crash(&plugin, "read", false);

    let again = started(&spec);
    opened(&again, &dir).unwrap();
    let s = again.session("U");
    assert_eq!(rows(&s, "SELECT COUNT(*), MAX(K) FROM T").unwrap(), ["40,39"]);
    s.run("INSERT INTO T VALUES (40, 'after')").unwrap();
    s.commit().unwrap();
    again.checkpoint().unwrap();
}

/// Verifies: [EXTENSION] RULES.md D251, D229: the program serving the
/// file system killed halfway through writing a page stops the node, and
/// the next opening recovers every commit from the log.
#[test]
fn a_host_plugins_file_system_survives_a_kill_mid_page_write() {
    let (scratch, plugin, spec) = hosted("host_files", "HOST FILE SYSTEM PASS;");
    let dir = scratch.path("db");
    let db = started(&spec);
    opened(&db, &dir).unwrap();
    filled(&db, 30);
    crash(&plugin, "write", true);
    let refused = db.checkpoint().unwrap_err();
    assert_eq!(refused.sqlstate().as_str(), "38T11", "{refused:?}");
    drop(db);
    crash(&plugin, "write", false);
    let again = started(&spec);
    opened(&again, &dir).unwrap();
    assert_eq!(rows(&again.session("U"), "SELECT COUNT(*) FROM T").unwrap(), ["30"]);
}

/// Verifies: [EXTENSION] RULES.md D250: a sort past MEMORY.WORK_BYTES
/// spills to the host plug-in, its runs in the process an area keeps,
/// and gives what it gives in memory.
#[test]
fn a_sort_spills_to_a_host_plugin() {
    let (_scratch, plugin, spec) = hosted("host_spill", "HOST SPILL MEMORY;");
    let db = started(&spec);
    db.schema("CREATE SCHEMA AUTHORIZATION U CREATE TABLE T (K INTEGER, S VARCHAR(200))").unwrap();
    let s = db.session("U");
    for k in 0..3000 {
        s.run(&format!("INSERT INTO T VALUES ({}, '{:0>150}')", (k * 7919) % 3000, k)).unwrap();
    }
    s.commit().unwrap();
    let in_memory = rows(&s, "SELECT K FROM T ORDER BY K, S").unwrap();
    exec(&s, "SET MEMORY.WORK_BYTES = 65536").unwrap();
    let spilled = rows(&s, "SELECT K FROM T ORDER BY K, S").unwrap();
    assert_eq!(spilled, in_memory);
    assert!(std::fs::read_to_string(plugin.join("spill.log")).is_ok_and(|l| l.contains("run")), "runs made in the program");
    db.set_setting("PLUGINS.HOST", &Value::Char(spec.into())).unwrap();
}
