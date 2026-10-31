//! The `turboreld` program, run from its command line and as a process:
//! it serves a database kept in a directory until told to stop, then
//! closes it.

use std::net::{SocketAddr, TcpListener};
use std::sync::mpsc;

use turborel_client::connection::{self, Trust};
use turboreld::{Command, Config};

use turborel_test_support::Scratch;
#[cfg(unix)]
use turborel_test_support::client::state;
use turborel_test_support::client::{done, rows};

fn config(line: &[String]) -> Config {
    match turboreld::parse(line).unwrap() {
        Command::Serve(config) => config,
        Command::Help | Command::Version | Command::User(..) | Command::Reseal(..) => panic!("not a server"),
    }
}

/// A server running on a thread: where it listens, what stops it once
/// used or dropped, and its outcome once it stops.
type Running = (Vec<String>, tokio::sync::oneshot::Sender<()>, std::thread::JoinHandle<Result<(), String>>);

/// Runs `config` on a thread until the returned sender is used or dropped.
fn running(config: Config) -> Running {
    running_then(config, |_| {})
}

/// Runs `config` as `running` does, doing `then` as it is ready, before
/// the test is told where it listens.
fn running_then(config: Config, then: impl FnOnce(&[String]) + Send + 'static) -> Running {
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (tell, told) = mpsc::channel();
    let ready = move |at: &[String]| {
        then(at);
        tell.send(at.to_vec()).unwrap();
    };
    let thread = std::thread::spawn(move || turboreld::run(&config, ready, (async { drop(stopped.await) }, std::future::pending())));
    match told.recv() {
        Ok(at) => (at, stop, thread),
        Err(mpsc::RecvError) => panic!("it stopped before it listened: {:?}", thread.join().unwrap()),
    }
}

/// Verifies: [DESIGN] RULES.md D25, D36, D42: the program serves the
/// database kept in its directory where its settings say, set from the
/// command line: over TCP at the port the system chose, and over a Unix
/// socket too, on Windows a named pipe; told to stop, it closes the database cleanly; run again
/// with no setting given, it listens where the settings kept say, and
/// serves what was committed, its files as the settings fixed at its
/// creation say: through LZ4 and AES-256-GCM, which it registers, the key
/// from `--keys`, as T's rows are, and refused without it; one given again
/// otherwise is refused, saying why, and the same taken. Its diagnostics are lines appended to
/// the file `--log` names.
#[test]
fn the_program_serves_a_kept_database_until_told_to_stop() {
    let scratch = Scratch::new("program");
    let dir = scratch.path("db").display().to_string();
    let mut line = vec!["--dir".into(), dir.clone(), "--set".into(), "SERVER.PORT=0".into(), "--set".into(), "SERVER.WORKERS=2".into()];
    // A client of this machine trusted, as SERVER.TRUST_LOCAL is not by default.
    line.extend(["--set".into(), "SERVER.TRUST_LOCAL=ON".into()]);
    let keys = scratch.path("keys");
    std::fs::create_dir_all(&keys).unwrap();
    std::fs::write(keys.join("MASTER"), "ab".repeat(32)).unwrap();
    let keys = keys.display().to_string();
    line.extend(["--set".into(), "STORAGE.PAGE_BYTES=16384".into(), "--set".into(), "STORAGE.ENCODINGS=lz4,aes-256-gcm:MASTER".into()]);
    line.extend(["--keys".into(), keys.clone()]);
    let log = scratch.path("log");
    line.extend(["--log".into(), log.display().to_string()]);
    if cfg!(any(unix, windows)) {
        line.extend(["--set".into(), format!("SERVER.SOCKET={}", scratch.socket("s").display())]);
    }
    let (at, stop, thread) = running(config(&line));
    assert_eq!(at.len(), if cfg!(any(unix, windows)) { 2 } else { 1 }, "{at:?}");
    let address: SocketAddr = at[0].parse().unwrap();
    let mut c = connection::connect(address, "X", &mut Trust).unwrap();
    let made = c.query("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER UNIQUE) WITH (ENCODINGS = 'LZ4,AES-256-GCM:MASTER')");
    assert_eq!(done(&made.unwrap()), 0, "indexed: the files are encrypted too");
    assert_eq!(done(&c.query("INSERT INTO T VALUES (7)").unwrap()), 0);
    c.terminate().unwrap();
    #[cfg(any(unix, windows))]
    {
        let mut local = connection::connect_local(&scratch.socket("s"), "X", &mut Trust).unwrap();
        assert_eq!(rows(&local.query("SELECT K FROM T").unwrap()), ["7"]);
        local.terminate().unwrap();
    }
    stop.send(()).unwrap();
    thread.join().unwrap().unwrap();
    let told = std::fs::read_to_string(&log).unwrap();
    assert!(told.contains(r#""event":"opened","how":"created""#) && told.contains(r#""event":"closed""#), "{told}");
    let without =
        turboreld::run(&config(&["--dir".into(), dir.clone()]), |_| panic!("not ready"), (async {}, std::future::pending())).unwrap_err();
    assert!(without.contains("cannot open") && without.contains("--keys"), "{without}");
    let changed = config(&["--dir".into(), dir.clone(), "--keys".into(), keys.clone(), "--set".into(), "STORAGE.PAGE_BYTES=4096".into()]);
    let refused = turboreld::run(&changed, |_| panic!("not ready"), (async {}, std::future::pending())).unwrap_err();
    // As the Operator's Guide shows it, of a database made with 16384.
    let fixed = "SQLCODE -910 (SQLSTATE TB910): the database was created with STORAGE.PAGE_BYTES = 16384, fixed then: it cannot be 4096";
    assert_eq!(refused, format!("cannot open {dir}: {fixed}"));
    let same = config(&["--dir".into(), dir.clone(), "--keys".into(), keys.clone(), "--set".into(), "STORAGE.PAGE_BYTES=16384".into()]);
    let (_, stop, thread) = running(same);
    drop(stop);
    thread.join().unwrap().unwrap();
    let (at, stop, thread) = running(config(&["--dir".into(), dir, "--keys".into(), keys]));
    assert_eq!(at.len(), if cfg!(any(unix, windows)) { 2 } else { 1 }, "kept: {at:?}");
    let mut c = connection::connect(at[0].parse::<SocketAddr>().unwrap(), "X", &mut Trust).unwrap();
    assert_eq!(rows(&c.query("SELECT K FROM T").unwrap()), ["7"]);
    let page = "SELECT TRIM(SETTING_VALUE) FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = 'STORAGE.PAGE_BYTES'";
    assert_eq!(rows(&c.query(page).unwrap()), ["16384"]);
    let encodings = "SELECT TRIM(SETTING_VALUE) FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = 'STORAGE.ENCODINGS'";
    assert_eq!(rows(&c.query(encodings).unwrap()), ["LZ4,AES-256-GCM:MASTER"]);
    c.terminate().unwrap();
    drop(stop);
    thread.join().unwrap().unwrap();
}

/// Verifies: [DESIGN] RULES.md D40, D42: on Unix, SIGHUP opens the `--log`
/// file again, as a log rotation asks once it has moved the file away:
/// the lines before go to the file moved, those after to a new one at the
/// path; and the server goes on, then stops cleanly when told. Told where
/// it listens, and that it stops. The rotation is done as soon as the
/// program is ready, as a supervisor may: SIGHUP is heard by then, and
/// `listening` told, in the file moved.
///
/// Regression test (R-TEST-7): SIGHUP ended the server uncleanly, and the
/// log was never opened again. Regression test (R-TEST-7): the program was
/// ready before it told `listening`, so a rotation then put that line in
/// the new file.
#[cfg(unix)]
#[test]
fn sighup_opens_the_log_again() {
    let scratch = Scratch::new("program-sighup");
    let log = scratch.path("log");
    let line = ["--memory", "--set", "SERVER.PORT=0", "--log", &log.display().to_string()];
    let (path, moved) = (log.clone(), scratch.path("log.1"));
    let (at, stop, thread) = running_then(config(&line.map(str::to_owned)), move |_| {
        std::fs::rename(&path, moved).unwrap();
        let hup = std::process::Command::new("kill").args(["-HUP", &std::process::id().to_string()]).status().unwrap();
        assert!(hup.success());
        // Opened again by the sink's thread, as the signal is heard.
        let reopened = (0..10_000_u32).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(1));
            path.exists()
        });
        assert!(reopened, "a new log at the path");
    });
    // Alive: it answers, refusing trust, which it is not set to take.
    let refused = connection::connect(at[0].parse::<SocketAddr>().unwrap(), "X", &mut Trust).err().unwrap();
    assert_eq!(state(&refused).0, "28000");
    stop.send(()).unwrap();
    thread.join().unwrap().unwrap();
    let rotated = std::fs::read_to_string(scratch.path("log.1")).unwrap();
    assert!(rotated.contains(&format!(r#""event":"listening","addresses":["{}"]"#, at[0])), "{rotated}");
    let after = std::fs::read_to_string(&log).unwrap();
    assert!(after.contains(r#""event":"stopping","connections":0"#) && !after.contains(r#""event":"listening""#), "{after}");
}

/// Verifies: [DESIGN] RULES.md D40, D42: on Unix, SIGHUP takes the
/// server's TLS certificate and key again, renewed in place in their
/// files: the connections after are secured by the new ones; files that
/// cannot be taken keep the ones there were, told in the log.
#[cfg(unix)]
#[test]
fn sighup_takes_tls_again() {
    use turborel_test_support::tls::pki;
    let (old, new) = (pki("program-tls"), pki("program-tls-new"));
    let log = old.scratch.path("log");
    let (chain, key) = (format!("SERVER.TLS_CERTIFICATE={}", old.chain.display()), format!("SERVER.TLS_KEY={}", old.key.display()));
    let log_at = log.display().to_string();
    let line = [
        "--memory",
        "--set",
        "SERVER.PORT=0",
        "--set",
        "SERVER.LOGIN_DELAY_MS=0",
        "--set",
        "SERVER.TLS=ON",
        "--set",
        &chain,
        "--set",
        &key,
        "--log",
        &log_at,
    ];
    let (at, stop, thread) = running(config(&line.map(str::to_owned)));
    let address: SocketAddr = at[0].parse().unwrap();
    // Trust is not taken: a refusal of credentials (28000) is a handshake
    // done, a certificate not trusted a connection failed (08006).
    let reached = |authority: &std::path::Path| {
        let config = turborel_client::tls::trusting(authority).unwrap();
        turborel_client::tls::connect(address, ("localhost", &config), "x", &mut Trust).err().unwrap().sqlstate
    };
    assert_eq!(reached(&old.authority), "28000");
    std::fs::copy(&new.chain, &old.chain).unwrap();
    std::fs::copy(&new.key, &old.key).unwrap();
    let hup = || assert!(std::process::Command::new("kill").args(["-HUP", &std::process::id().to_string()]).status().unwrap().success());
    hup();
    let renewed = (0..10_000_u32).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(1));
        reached(&new.authority) == "28000"
    });
    assert!(renewed, "secured by the new certificate");
    std::fs::write(&old.chain, b"").unwrap();
    hup();
    let told = (0..10_000_u32).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(1));
        std::fs::read_to_string(&log).unwrap_or_default().contains(r#""event":"tls_reload_failed""#)
    });
    assert!(told, "a failed reload told");
    assert_eq!(reached(&new.authority), "28000", "the one there was kept");
    stop.send(()).unwrap();
    thread.join().unwrap().unwrap();
    assert!(std::fs::read_to_string(&log).unwrap().contains(r#""event":"tls_reloaded""#));
}

/// Verifies: [DESIGN] RULES.md D36, D42: the program stops, saying why,
/// when it cannot open its database, set a setting, listen at its address
/// or at its socket, or is set to listen nowhere; it serves nothing then.
#[test]
fn the_program_says_why_it_cannot_serve() {
    let scratch = Scratch::new("program-fails");
    let file = scratch.path("file");
    std::fs::write(&file, b"not a database").unwrap();
    let line = |more: &[&str]| more.iter().map(|s| (*s).to_owned()).collect::<Vec<String>>();
    let ready = |_: &[String]| panic!("not ready");
    let failed =
        turboreld::run(&config(&line(&["--dir", &file.display().to_string()])), ready, (async {}, std::future::pending())).unwrap_err();
    assert!(failed.contains("cannot open"), "{failed}");
    let failed =
        turboreld::run(&config(&line(&["--memory", "--set", "SERVER.PORT=70000"])), ready, (async {}, std::future::pending())).unwrap_err();
    assert!(failed.contains("cannot set SERVER.PORT"), "{failed}");
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = format!("SERVER.PORT={}", taken.local_addr().unwrap().port());
    let failed = turboreld::run(&config(&line(&["--memory", "--set", &port])), ready, (async {}, std::future::pending())).unwrap_err();
    assert!(failed.contains("cannot listen"), "{failed}");
    let failed =
        turboreld::run(&config(&line(&["--memory", "--set", "SERVER.HOST="])), ready, (async {}, std::future::pending())).unwrap_err();
    assert!(failed.contains("nowhere"), "{failed}");
    let socket = format!("SERVER.SOCKET={}", file.display());
    let failed = turboreld::run(&config(&line(&["--memory", "--set", &socket])), ready, (async {}, std::future::pending())).unwrap_err();
    assert!(failed.contains(if cfg!(any(unix, windows)) { "cannot listen" } else { "not on this platform" }), "{failed}");
    let nowhere = scratch.path("no/such/dir/log").display().to_string();
    let failed = turboreld::run(&config(&line(&["--memory", "--log", &nowhere])), ready, (async {}, std::future::pending())).unwrap_err();
    assert!(failed.contains("cannot open the log"), "{failed}");
}

/// Verifies: [DESIGN] RULES.md D42: `turboreld --help` prints its usage
/// and succeeds; a wrong command line fails, saying why.
#[test]
fn the_program_prints_its_usage_or_what_is_wrong() {
    let program = env!("CARGO_BIN_EXE_turboreld");
    let help = std::process::Command::new(program).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout).unwrap().contains("SERVER.PORT"));
    let wrong = std::process::Command::new(program).arg("--verbose").output().unwrap();
    assert!(!wrong.status.success());
    assert!(String::from_utf8(wrong.stderr).unwrap().contains("turboreld: --verbose is not an option"));
}

/// Verifies: [DESIGN] RULES.md D42, D36: the usage names each setting of
/// the server's own, as the server declares them.
/// Regression test (R-TEST-7): SERVER.MAX_STATEMENTS was not named.
#[test]
fn the_usage_names_every_setting_of_the_server() {
    let db = turborel::db::Database::new();
    turborel_server::server::Server::new(db.clone()).unwrap();
    let names: Vec<String> =
        db.settings().unwrap().into_iter().map(|l| l.definition.name.clone()).filter(|n| n.starts_with("SERVER.")).collect();
    assert!(names.len() > 10, "{names:?}");
    for name in names {
        assert!(turboreld::USAGE.contains(&format!("  {name} ")), "{name} is not in the usage");
    }
}

/// Verifies: [DESIGN] RULES.md D42: the program is ready only once what
/// stops it has been polled, as `turboreld::signalled` hears SIGTERM from
/// its first poll: a signal sent as soon as it says where it listens
/// stops it cleanly.
/// Regression test (R-TEST-7): it was ready first, and a SIGTERM sent at
/// once, before its handler was set, ended the process uncleanly, or, set
/// but not yet listened for, was lost, and the program served on.
#[test]
fn the_program_hears_what_stops_it_before_it_is_ready() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let polled = Arc::new(AtomicBool::new(false));
    let heard = Arc::clone(&polled);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (tell, told) = mpsc::channel();
    let served = config(&["--memory", "--set", "SERVER.PORT=0"].map(str::to_owned));
    let thread = std::thread::spawn(move || {
        let hearing = async move {
            heard.store(true, Ordering::SeqCst);
            drop(stopped.await);
        };
        turboreld::run(&served, |_| tell.send(polled.load(Ordering::SeqCst)).unwrap(), (hearing, std::future::pending()))
    });
    assert!(told.recv().unwrap(), "what stops it polled before it is ready");
    stop.send(()).unwrap();
    thread.join().unwrap().unwrap();
}

/// Verifies: [DESIGN] RULES.md D42: `turboreld` serves until SIGTERM, then
/// stops cleanly: its client's open transaction rolled back, it exits
/// successfully. A database kept nowhere has nothing to close.
#[cfg(unix)]
#[test]
fn the_program_stops_cleanly_when_terminated() {
    use std::io::{BufRead, BufReader};
    let program = env!("CARGO_BIN_EXE_turboreld");
    let mut child = std::process::Command::new(program)
        .args(["--memory", "--set", "SERVER.PORT=0", "--set", "SERVER.TRUST_LOCAL=ON"])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // The diagnostics, lines of JSON, come first on standard error.
    let said = BufReader::new(child.stderr.take().unwrap()).lines().map(Result::unwrap).find(|l| l.starts_with("turboreld: listening at "));
    let said = said.unwrap();
    let at = said.trim().strip_prefix("turboreld: listening at ").unwrap();
    let mut c = connection::connect(at.parse::<SocketAddr>().unwrap(), "X", &mut Trust).unwrap();
    assert_eq!(done(&c.query("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap()), 0);
    assert_eq!(done(&c.query("INSERT INTO T VALUES (1)").unwrap()), 0);
    let killed = std::process::Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap();
    assert!(killed.success());
    assert!(child.wait().unwrap().success());
    assert_eq!(state(&c.query("SELECT K FROM T").unwrap_err()).0, "08006", "cut off as it stopped");
}
