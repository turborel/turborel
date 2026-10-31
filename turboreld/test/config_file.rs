//! The `turboreld` program configured by a file, `--config FILE`: the
//! source of the database's settings as it starts, beside `--set`
//! (RULES.md D36, D42).

use std::net::SocketAddr;

use turborel_client::connection::{self, Trust};
use turborel_server::settings::{HOST, PORT, SCRAM_WITHOUT_TLS, SHUTDOWN_TIMEOUT_MS, TRUST_LOCAL, WORKERS};
use turborel_test_support::Scratch;
use turborel_test_support::client::rows;

use crate::program::{config, running};

/// The configuration file of the Operator's Guide (doc/operator/02-turboreld.md,
/// "Configuration files"), as it shows it.
const GUIDE_FILE: &str = r#"# The settings of the main database's server, set as it starts.
[SERVER]
HOST = ["127.0.0.1"]
PORT = 7422
WORKERS = 2
SHUTDOWN_TIMEOUT_MS = 5_000
SCRAM_WITHOUT_TLS = false

[STORAGE]
PAGE_BYTES = 16_384        # fixed as the database is made
POOL_BYTES = 67_108_864
"#;

/// A command line of `words`.
fn line(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| (*w).to_owned()).collect()
}

/// Verifies: [DESIGN] RULES.md D36, D42: the program serves a database
/// configured by the Operator's Guide's file, each setting it gives set
/// as `--set` sets it: the one fixed as the database is made before it is,
/// its headers and keys naming the settings, a boolean a switch's OFF, an
/// array a list; and `--set` wins over the file, as SERVER.PORT shows.
#[test]
fn the_program_takes_its_settings_from_a_file() {
    let scratch = Scratch::new("config-file");
    let file = scratch.path("main.toml");
    std::fs::write(&file, GUIDE_FILE).unwrap();
    let (dir, file) = (scratch.path("db").display().to_string(), file.display().to_string());
    // Port 0, as a test must; and a client of this machine trusted.
    let set = |name: &str, value: &str| format!("{name}={value}");
    let (port, trust) = (set(PORT, "0"), set(TRUST_LOCAL, "ON"));
    let (at, stop, thread) = running(config(&line(&["--dir", &dir, "--config", &file, "--set", &port, "--set", &trust])));
    let address: SocketAddr = at[0].parse().unwrap();
    assert_eq!(address.ip().to_string(), "127.0.0.1");
    let mut c = connection::connect(address, "X", &mut Trust).unwrap();
    let value = |c: &mut connection::Connection<_>, name: &str| {
        let query = format!("SELECT TRIM(SETTING_VALUE) FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = '{name}'");
        rows(&c.query(&query).unwrap()).join("")
    };
    let expected = [(HOST, "127.0.0.1"), (PORT, "0"), (WORKERS, "2"), (SHUTDOWN_TIMEOUT_MS, "5000"), (SCRAM_WITHOUT_TLS, "OFF")];
    for (name, shown) in expected {
        assert_eq!(value(&mut c, name), shown, "{name}");
    }
    assert_eq!(value(&mut c, "STORAGE.PAGE_BYTES"), "16384");
    assert_eq!(value(&mut c, "STORAGE.POOL_BYTES"), "67108864");
    c.terminate().unwrap();
    drop(stop);
    thread.join().unwrap().unwrap();
}

/// Verifies: [DESIGN] RULES.md D36, D42: a file that cannot be read, is
/// not of the subset, names no setting, or gives a value its setting does
/// not take is refused, its path and the line said, and the server does
/// not start; a file wrong before any setting is set makes no database.
#[test]
fn a_wrong_file_is_refused_with_its_line() {
    let scratch = Scratch::new("config-file-wrong");
    let dir = scratch.path("db").display().to_string();
    let refused = |text: &str| {
        let file = scratch.path("wrong.toml");
        std::fs::write(&file, text).unwrap();
        let path = file.display().to_string();
        let ready = |_: &[String]| panic!("not ready");
        let failed = turboreld::run(&config(&line(&["--dir", &dir, "--config", &path])), ready, (async {}, std::future::pending()));
        failed.unwrap_err().replace(&path, "FILE")
    };
    assert_eq!(refused("[STORAGE]\nPAGE_BYTES = 4096\nNO_SUCH = 1\n"), "FILE:3:1: no setting is named STORAGE.NO_SUCH");
    assert_eq!(refused("[SERVER]\nPORT = 0x1F\n"), "FILE:2:8: an integer is decimal: 0x, 0o and 0b are not taken");
    assert!(!scratch.path("db").exists(), "nothing was set, nor a database made");
    let workers = refused(&format!("{WORKERS} = 'many'\n"));
    assert!(workers.starts_with(&format!("FILE:1:1: cannot set {WORKERS}: SQLCODE -910")), "{workers}");
    let missing = scratch.path("missing.toml").display().to_string();
    let ready = |_: &[String]| panic!("not ready");
    let failed =
        turboreld::run(&config(&line(&["--memory", "--config", &missing])), ready, (async {}, std::future::pending())).unwrap_err();
    assert!(failed.starts_with(&format!("cannot read {missing}")), "{failed}");
}
