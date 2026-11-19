//! The program's backups (RULES.md D160, D161, D164). An encrypted
//! database served is backed up by its owner in SQL, the backup's objects
//! encrypted too; `--verify-backup` checks it; `--restore` restores it
//! into a new directory, which the program then serves.

use std::io::Cursor;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::mpsc;

use turborel_client::connection::{self, Trust};
use turborel_test_support::Scratch;
use turborel_test_support::client::{done, rows};
use turboreld::{BackupCommand, Command, Config, UserCommand};

/// The command line `line`, its words split at spaces.
fn parsed(line: &str) -> Result<Command, String> {
    turboreld::parse(&line.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
}

/// The configuration and command of a backup's command line `line`.
fn backup_command(line: &str) -> (Config, BackupCommand) {
    match parsed(line).unwrap() {
        Command::Backup(config, command) => (config, command),
        Command::Serve(_) | Command::User(..) | Command::Reseal(..) | Command::Rekey(_) | Command::Help | Command::Version => {
            panic!("not a backup's command")
        }
    }
}

/// The configuration of a server's command line `line`.
fn config(line: &str) -> Config {
    match parsed(line).unwrap() {
        Command::Serve(config) | Command::User(config, _) => config,
        Command::Backup(..) | Command::Reseal(..) | Command::Rekey(_) | Command::Help | Command::Version => panic!("not a server"),
    }
}

/// Serves `config`, runs each of `queries` as the user X over TCP, each to
/// be done, then stops: the rows of the last, if it has any.
fn served(config: Config, queries: &[&str]) -> Vec<String> {
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (tell, told) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        turboreld::run(&config, |at| tell.send(at.to_vec()).unwrap(), (async { drop(stopped.await) }, std::future::pending()))
    });
    let at: SocketAddr = told.recv().unwrap()[0].parse().unwrap();
    let mut c = connection::connect(at, "X", &mut Trust).unwrap();
    let mut last = Vec::new();
    for query in queries {
        let outcome = c.query(query).unwrap();
        last = if query.starts_with("SELECT") { rows(&outcome) } else { vec![done(&outcome).to_string()] };
    }
    c.terminate().unwrap();
    stop.send(()).unwrap();
    thread.join().unwrap().unwrap();
    last
}

/// Whether some file under `dir` holds `bytes`.
fn holds(dir: &Path, bytes: &[u8]) -> bool {
    std::fs::read_dir(dir).unwrap().any(|e| {
        let path = e.unwrap().path();
        if path.is_dir() { holds(&path, bytes) } else { std::fs::read(&path).unwrap().windows(bytes.len()).any(|w| w == bytes) }
    })
}

/// Verifies: [EXTENSION] RULES.md D160, D161, D164: BACKUP DATABASE, run by
/// the owner of a database encrypted by ChaCha20-Poly1305 under L, writes
/// a backup none of whose objects holds a row's text in the clear;
/// `--verify-backup` with the keys finds it sound, and without them is
/// refused; `--restore` restores it into a new directory, served then
/// with the rows committed before it; a restore into a directory there
/// already is refused.
#[test]
fn an_encrypted_backup_is_restored_by_the_program() {
    let scratch = Scratch::new("turboreld-backup");
    let (dir, keys, target) = (scratch.path("db"), scratch.path("keys"), scratch.path("target"));
    std::fs::create_dir_all(&keys).unwrap();
    std::fs::write(keys.join("L"), "0b".repeat(32)).unwrap();
    let base = format!("--dir {} --keys {}", dir.display(), keys.display());
    let owner =
        config(&format!("{base} --set STORAGE.ENCODINGS=chacha20-poly1305:L --set SERVER.PASSWORD_ITERATIONS=10000 user add X --owner"));
    turboreld::run_user(&owner, &UserCommand::Add { name: "X".into(), owner: true }, (&mut Cursor::new(&b"pw\n"[..]), &mut Vec::new()))
        .unwrap();
    let server = format!("{base} --set SERVER.PORT=0 --set SERVER.TRUST_LOCAL=ON");
    let backup = format!("BACKUP DATABASE TO '{}' LABEL 'nightly'", target.display());
    let shown = "SELECT TRIM(KIND), TRIM(STATE), TRIM(LABEL) FROM INFORMATION_SCHEMA.BACKUPS";
    let queries = [
        "CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER, S VARCHAR(20))",
        "INSERT INTO T VALUES (1, 'plainly secret')",
        "COMMIT",
        &backup,
        shown,
    ];
    assert_eq!(served(config(&server), &queries), ["FULL,COMPLETED,nightly"]);
    assert!(!holds(&target, b"plainly secret"), "the backup is encrypted");
    let mut out = Vec::new();
    let (verify, command) = backup_command(&format!("--keys {} --verify-backup {}", keys.display(), target.display()));
    turboreld::run_backup(&verify, &command, &mut out).unwrap();
    assert!(String::from_utf8(out).unwrap().starts_with("the backup is sound: 1 backups"));
    let (keyless, command) = backup_command(&format!("--verify-backup {}", target.display()));
    assert!(turboreld::run_backup(&keyless, &command, &mut Vec::new()).unwrap_err().contains("not sound"));
    let back = scratch.path("back");
    let (restore, command) = backup_command(&format!("--dir {} --keys {} --restore {}", back.display(), keys.display(), target.display()));
    let mut out = Vec::new();
    turboreld::run_backup(&restore, &command, &mut out).unwrap();
    assert!(String::from_utf8(out).unwrap().starts_with("restored B"));
    let again = format!("--dir {} --keys {} --set SERVER.PORT=0 --set SERVER.TRUST_LOCAL=ON", back.display(), keys.display());
    assert_eq!(served(config(&again), &["SELECT K, TRIM(S) FROM X.T"]), ["1,plainly secret"]);
    assert!(turboreld::run_backup(&restore, &command, &mut Vec::new()).unwrap_err().contains("there already"));
}

/// Verifies: [EXTENSION] RULES.md D164: the command line of a restore
/// names its target and where recovery stops, a time as a timestamp of
/// UTC; one stopping at a time or a restore point names the archive; an
/// option of a restore is refused after `--verify-backup`.
#[test]
fn a_restore_s_command_line_names_where_it_stops() {
    let words = ["--dir", "d", "--restore", "t", "--archive", "a", "--until-time", "1970-01-01 00:00:01"].map(str::to_owned);
    let Ok(Command::Backup(_, BackupCommand::Restore { target, .. })) = turboreld::parse(&words) else { panic!("a restore") };
    assert_eq!(format!("{target:?}"), "Time(1000000)");
    assert_eq!(turboreld::instant("1970-01-01 01:00:00+01:00"), Ok(0));
    assert!(parsed("--dir d --restore t --until-point p").unwrap_err().contains("--archive"));
    assert!(parsed("--verify-backup t --archive a").unwrap_err().contains("not an option"));
    assert!(parsed("--dir d --restore").unwrap_err().contains("needs a backup target"));
}
