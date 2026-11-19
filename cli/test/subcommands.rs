//! `turborel extension` and `turborel admin`, run as processes.

use std::process::{Command, Output};

use turborel::db::Database;

use crate::common::{Scratch, start};

/// `turborel` run with `args`.
fn program(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_turborel")).args(args).env_remove(turborel_cli::PASSWORD_VARIABLE).output().unwrap()
}

/// What `run` wrote to standard output and standard error.
fn said(run: &Output) -> (String, String) {
    (String::from_utf8(run.stdout.clone()).unwrap(), String::from_utf8(run.stderr.clone()).unwrap())
}

/// Verifies: [DESIGN] RULES.md D46, D215: `turborel extension` alone
/// prints its usage and succeeds; a command of it runs, its output
/// written, and one that fails says why, prefixed by the program's
/// command, and fails.
#[test]
fn extension_commands_run_as_a_process() {
    let usage = program(&["extension"]);
    assert!(usage.status.success());
    assert!(said(&usage).0.starts_with("usage: turborel extension COMMAND"), "{:?}", said(&usage));
    let scratch = Scratch::new("cli-extension-process");
    let set = format!("PLUGINS.DIRECTORIES={}", scratch.path("plugins").display());
    let listed = program(&["extension", "list", "--set", &set]);
    assert!(listed.status.success(), "{:?}", said(&listed));
    let unknown = program(&["extension", "info", "NO_SUCH_PACKAGE", "--set", &set]);
    assert!(!unknown.status.success());
    assert!(said(&unknown).1.starts_with("turborel extension: "), "{:?}", said(&unknown));
}

/// Verifies: [DESIGN] RULES.md D215, D218: `turborel admin` alone prints
/// its usage and succeeds; a command answered succeeds, its output
/// written; one refused fails with status 1, saying why; a server not
/// reached fails, saying why.
#[test]
fn admin_commands_run_as_a_process() {
    let usage = program(&["admin"]);
    assert!(usage.status.success());
    assert!(said(&usage).0.starts_with("usage: turborel admin"), "{:?}", said(&usage));
    let db = Database::new();
    db.create_user("X", b"x").unwrap();
    db.set_owner("X").unwrap();
    let server = start(&db, &[]);
    let port = server.address.port().to_string();
    let ping = program(&["admin", "-p", &port, "-U", "X", "ping"]);
    assert!(ping.status.success(), "{:?}", said(&ping));
    assert!(said(&ping).0.contains(" answers, in "), "{:?}", said(&ping));
    let refused = program(&["admin", "-p", &port, "-U", "X", "kill", "999999"]);
    assert_eq!(refused.status.code(), Some(1), "{:?}", said(&refused));
    assert!(said(&refused).1.starts_with("turborel admin: "), "{:?}", said(&refused));
    let unreached = program(&["admin", "-p", "1", "-U", "X", "ping"]);
    assert!(!unreached.status.success());
    assert!(said(&unreached).1.starts_with("turborel admin: "), "{:?}", said(&unreached));
}
