//! The program's cryptography (RULES.md D26, D48): its files checksummed
//! with HMAC-SHA-256 keyed by `--checksum-key`, and encrypted with
//! ChaCha20-Poly1305; a table's rows sealed again by `turboreld ...
//! reseal`, and a key it named before then retired.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::mpsc;

use turborel_client::connection::{self, Trust};
use turborel_test_support::Scratch;
use turborel_test_support::client::{done, rows};
use turboreld::{Command, Config};

/// The command line `line`, its words split at spaces.
fn parsed(line: &str) -> Result<Command, String> {
    turboreld::parse(&line.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
}

/// The configuration of `line`.
fn config(line: &str) -> Config {
    match parsed(line).unwrap() {
        Command::Serve(config) | Command::User(config, _) | Command::Reseal(config, _) => config,
        Command::Help | Command::Version => panic!("help"),
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

/// Writes the key `name` into `keys`, 32 bytes of `byte`.
fn key(keys: &Path, name: &str, byte: &str) {
    std::fs::write(keys.join(name), byte.repeat(32)).unwrap();
}

/// Verifies: [EXTENSION] RULES.md D26, D48: the program registers
/// ChaCha20-Poly1305, SHA-256, and HMAC-SHA-256 keyed by the key
/// `--checksum-key` names: a database created with its files checksummed
/// by that and encrypted by ChaCha20-Poly1305 under L serves a table
/// encrypted under K; its encodings changed to L, `reseal` seals its rows
/// again, and with K's file gone it is served, its rows read. Without
/// `--checksum-key` it is refused, its files of hash function 12, HMAC-SHA-256, not given;
/// with a key not there, the program says it cannot register it; reseal
/// refuses a database kept nowhere, a table not there, and a command line
/// naming none or two.
#[test]
fn a_key_is_retired_by_resealing_its_table() {
    let scratch = Scratch::new("turboreld-keys");
    let (dir, keys) = (scratch.path("db"), scratch.path("keys"));
    std::fs::create_dir_all(&keys).unwrap();
    key(&keys, "K", "0a");
    key(&keys, "L", "0b");
    let base = format!("--dir {} --keys {} --checksum-key L", dir.display(), keys.display());
    let server = format!("{base} --set SERVER.PORT=0 --set SERVER.TRUST=LOCAL");
    let created = format!("{server} --set STORAGE.CHECKSUM=hmac-sha-256 --set STORAGE.ENCODINGS=chacha20-poly1305:L");
    let made = served(
        config(&created),
        &[
            "CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER, S VARCHAR(20)) WITH (ENCODINGS = 'AES-256-GCM:K')",
            "INSERT INTO T VALUES (1, 'under K')",
            "ALTER TABLE T SET (ENCODINGS = 'ChaCha20-Poly1305:L')",
            "COMMIT WORK",
        ],
    );
    assert_eq!(made, ["0"]);
    let Command::Reseal(config_of, table) = parsed(&format!("{base} reseal X.T")).unwrap() else { panic!("reseal") };
    let mut out = Vec::new();
    turboreld::run_reseal(&config_of, &table, &mut out).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "X.T sealed again\n");
    std::fs::remove_file(keys.join("K")).unwrap();
    assert_eq!(served(config(&server), &["SELECT S FROM T"]), ["under K"]);

    let unkeyed = format!("--dir {} --keys {} --set SERVER.PORT=0", dir.display(), keys.display());
    let refused = turboreld::run(&config(&unkeyed), |_| panic!("not ready"), (async {}, std::future::pending())).unwrap_err();
    assert!(refused.contains("checksummed with hash function 12 (HMAC-SHA-256), which is not given"), "{refused}");
    let missing =
        turboreld::run(&config(&format!("{unkeyed} --checksum-key K")), |_| panic!("not ready"), (async {}, std::future::pending()))
            .unwrap_err();
    assert!(missing.contains("cannot register HMAC-SHA-256") && missing.contains("no key \"K\""), "{missing}");
    let none = turboreld::run_reseal(&config("--memory"), "X.T", &mut Vec::new()).unwrap_err();
    assert!(none.contains("--dir"), "{none}");
    let absent = turboreld::run_reseal(&config_of, "X.NONE", &mut Vec::new()).unwrap_err();
    assert!(absent.contains("cannot reseal X.NONE"), "{absent}");
    for wrong in ["reseal", "reseal X.T X.U", "reseal T"] {
        let said = parsed(&format!("--dir db {wrong}")).unwrap_err();
        assert!(said.contains("reseal takes one table"), "{said}");
    }
}
