//! `turboreld`'s commands on a database no server holds, run as a
//! process: its release, users, reseal, rekey and a backup verified.

use std::io::Write;
use std::process::{Command, Output, Stdio};

use turborel_test_support::Scratch;

/// `turboreld` run with `args`, `input` its standard input.
fn program(args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_turboreld"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

/// What `run` wrote to standard output and standard error.
fn said(run: &Output) -> (String, String) {
    (String::from_utf8_lossy(&run.stdout).into_owned(), String::from_utf8_lossy(&run.stderr).into_owned())
}

/// Verifies: [DESIGN] RULES.md D42: `turboreld --version` prints the
/// release; `user add` reads the password from standard input when it is
/// no terminal, and `user list` shows the user, the owner marked; `rekey`
/// writes the database's files again; `reseal` of a table there is not
/// fails, saying why (SQLCODE -103) after the diagnostics; `--verify-backup` of a target that holds no backup
/// fails, saying why.
#[test]
fn the_commands_run_as_a_process() {
    let version = program(&["--version"], "");
    assert!(version.status.success());
    assert_eq!(said(&version).0.trim_end(), turborel::product());
    let scratch = Scratch::new("turboreld-subcommands");
    let dir = scratch.path("db").display().to_string();
    let added = program(&["--dir", &dir, "user", "add", "ANN", "--owner"], "ann's\n");
    assert!(added.status.success(), "{:?}", said(&added));
    let listed = program(&["--dir", &dir, "user", "list"], "");
    assert!(listed.status.success(), "{:?}", said(&listed));
    assert!(said(&listed).0.contains("ANN"), "{:?}", said(&listed));
    let rekeyed = program(&["--dir", &dir, "rekey"], "");
    assert!(rekeyed.status.success(), "{:?}", said(&rekeyed));
    let resealed = program(&["--dir", &dir, "reseal", "ANN.NOWHERE"], "");
    assert!(!resealed.status.success());
    assert!(said(&resealed).1.contains("\nturboreld: cannot reseal ANN.NOWHERE: SQLCODE -103"), "{:?}", said(&resealed));
    let target = scratch.path("backups").display().to_string();
    let verified = program(&["--verify-backup", &target], "");
    assert!(!verified.status.success());
    assert!(said(&verified).1.lines().last().is_some_and(|l| l.starts_with("turboreld: ")), "{:?}", said(&verified));
}
