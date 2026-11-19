//! A backup's archive served by a host plug-in (RULES.md D274): the
//! example's `vault`, declared by PLUGINS.HOST, reached by a backup and
//! by a restore into a database that has no extension and no target
//! registered.

use std::path::Path;

use turborel::backup::seed::RecoveryTarget;
use turborel::db::backup::{BackupOptions, RestoreOptions};
use turborel::db::{Database, OpenOptions};
use turborel::value::Value;

use crate::common::{Scratch, rows};
use crate::plugins::fixture::{install, manifest};

/// A database declaring the example, installed in `root`, as the host
/// plug-in VAULTED serving the archive `vault`.
fn declaring(root: &Path) -> Database {
    let db = Database::new();
    db.set_setting("PLUGINS.DIRECTORIES", &Value::Char(root.display().to_string())).unwrap();
    db.set_setting("PLUGINS.HOST", &Value::Char(format!("VAULTED={}", root.join("VAULTED").join("1.0.0").display()))).unwrap();
    db
}

/// Verifies: [EXTENSION] RULES.md D274: a backup written through a host
/// archive is restored through it before any database opens, with no
/// extension installed nor target registered; and the program's ERROR, a
/// location it cannot open, fails only the restore, the node
/// going on.
#[test]
fn a_host_archive_serves_a_backup_and_its_restore() {
    let scratch = Scratch::new("plugin-archive");
    let root = scratch.path("plugins");
    install(&root, ("VAULTED", "1.0.0"), &manifest("VAULTED", "1.0.0", "", "HOST ARCHIVE vault;"));
    let db = declaring(&root);
    db.start_host_plugins().unwrap();
    db.open_files(&crate::common::os(), &scratch.path("db"), &OpenOptions::default()).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION U CREATE TABLE T (K INTEGER)").unwrap();
    let s = db.session("U");
    s.run("INSERT INTO T VALUES (7)").unwrap();
    s.commit().unwrap();
    let location = format!("vault:{}", scratch.path("store").display());
    db.backup(&BackupOptions { location: location.clone(), ..BackupOptions::default() }).unwrap();
    assert!(scratch.path("store").join("backups").is_dir(), "written through the host plug-in");
    drop((s, db));
    let restore = |location: String, dir: &Path| {
        let options = RestoreOptions {
            dir: dir.to_owned(),
            location,
            backup: String::new(),
            archive: None,
            target: RecoveryTarget::End,
            timeline: None,
        };
        let restorer = declaring(&root);
        (restorer.restore(&options), restorer)
    };
    // The program cannot make a directory under a file: its ERROR.
    std::fs::write(scratch.path("plain"), b"a file").unwrap();
    let (missing, restorer) = restore(format!("vault:{}", scratch.path("plain").join("under").display()), &scratch.path("nothing"));
    assert_eq!(missing.unwrap_err().state.map(|s| s.to_string()), Some("38000".to_owned()));
    assert!(restorer.host_stopped().unwrap().is_none(), "the archive's ERROR is the call's alone");
    let dir = scratch.path("back");
    let (restored, restorer) = restore(location, &dir);
    restored.unwrap();
    drop(restorer);
    let back = Database::new();
    back.open_files(&crate::common::os(), &dir, &OpenOptions::default()).unwrap();
    assert_eq!(rows(&back.session("U"), "SELECT K FROM T").unwrap(), ["7"]);
}
