//! The command line: where the server is, who connects, what is run.

use std::path::PathBuf;

use std::time::Duration;

use turborel_cli::{Command, Credentials, DEFAULT_PORT, Input, Options, PASSWORD_VARIABLE, Server, Tls, parse};
use turborel_client::timeouts::{CONNECT_WITHIN, Timeouts};

/// The options of `line`, in an environment of `vars`.
fn parsed(line: &[&str], vars: &[(&str, &str)]) -> Result<Options, String> {
    let args: Vec<String> = line.iter().map(|&a| a.to_owned()).collect();
    let env = |name: &str| vars.iter().find(|(n, _)| *n == name).map(|(_, v)| (*v).to_owned());
    parse(&args, &env).map(|command| match command {
        Command::Run(options) => *options,
        Command::Help | Command::Version => panic!("help"),
    })
}

/// Verifies: [DESIGN] RULES.md D46: with nothing named, the client reaches
/// 127.0.0.1 at port 7422 over TCP, as the user of the environment,
/// trusted, and reads standard input; a null is shown as nothing; it
/// waits 30 seconds to connect and log in, and for replies without limit.
#[test]
fn defaults_reach_the_local_server() {
    let options = parsed(&[], &[("USER", "ann")]).unwrap();
    assert_eq!(options.server, Server::Tcp { host: "127.0.0.1".into(), port: DEFAULT_PORT, tls: None });
    assert_eq!(DEFAULT_PORT, 7422);
    assert_eq!((options.user.as_str(), options.inputs.as_slice(), options.null.as_str()), ("ann", [Input::Standard].as_slice(), ""));
    assert!(matches!(options.credentials, Credentials::Trust));
    assert!(!options.stop_on_error);
    assert_eq!(options.timeouts, Timeouts { connect: Some(CONNECT_WITHIN), read: None });
    assert_eq!(CONNECT_WITHIN, Duration::from_secs(30));
    assert_eq!(parsed(&[], &[("USERNAME", "bob")]).unwrap().user, "bob", "as Windows names it");
    assert_eq!(parsed(&[], &[]).unwrap_err(), "name the user: --user NAME");
}

/// Verifies: [DESIGN] RULES.md D46: every option is taken, its short form
/// too; statements run in the order named; a password is typed if asked
/// for, else taken from TURBOREL_PASSWORD if it is set.
#[test]
fn options_are_taken() {
    let line = [
        "-h",
        "db.example",
        "-p",
        "7000",
        "-U",
        "carol",
        "--tls",
        "ca.pem",
        "--server-name",
        "db",
        "-c",
        "SELECT 1",
        "-f",
        "a.sql",
        "--command",
        "SELECT 2",
        "--stop-on-error",
        "--null",
        "<null>",
        "--connect-timeout",
        "0",
        "--read-timeout",
        "5",
    ];
    let options = parsed(&line, &[(PASSWORD_VARIABLE, "secret")]).unwrap();
    let tls = Some(Tls { roots: PathBuf::from("ca.pem"), name: Some("db".into()), identity: None });
    assert_eq!(options.server, Server::Tcp { host: "db.example".into(), port: 7000, tls });
    let inputs = [Input::Command("SELECT 1".into()), Input::File(PathBuf::from("a.sql")), Input::Command("SELECT 2".into())];
    assert_eq!((options.user.as_str(), options.inputs.as_slice(), options.null.as_str()), ("carol", inputs.as_slice(), "<null>"));
    assert!(options.stop_on_error);
    assert_eq!(options.timeouts, Timeouts { connect: None, read: Some(Duration::from_secs(5)) }, "0 for no limit");
    let Credentials::Given(password) = &options.credentials else { panic!("given") };
    assert_eq!(password.0, "secret");
    assert_eq!(format!("{:?}", options.credentials), "Given(Password(..))", "never shown");
    let long =
        parsed(&["--host", "h", "--port", "1", "--user", "u", "--password", "--file", "b.sql"], &[(PASSWORD_VARIABLE, "x")]).unwrap();
    assert!(matches!(long.credentials, Credentials::Ask), "typed, if asked for");
    assert_eq!(long.inputs, [Input::File(PathBuf::from("b.sql"))]);
}

/// Verifies: [DESIGN] RULES.md D43, D46: `--certificate` and `--key` name
/// the certificate the client presents over TLS, and its key; without a
/// password, the certificate proves the user; with one, the password does,
/// the certificate presented still.
#[test]
fn a_certificate_is_presented() {
    let line = ["--tls", "ca.pem", "--certificate", "c.pem", "--key", "k.pem"];
    let options = parsed(&line, &[("USER", "u")]).unwrap();
    let identity = Some((PathBuf::from("c.pem"), PathBuf::from("k.pem")));
    let tls = Some(Tls { roots: PathBuf::from("ca.pem"), name: None, identity: identity.clone() });
    assert_eq!(options.server, Server::Tcp { host: "127.0.0.1".into(), port: DEFAULT_PORT, tls });
    assert!(matches!(options.credentials, Credentials::Certificate));
    let with_password = parsed(&line, &[("USER", "u"), (PASSWORD_VARIABLE, "x")]).unwrap();
    assert!(matches!(with_password.credentials, Credentials::Given(_)));
}

/// Verifies: [DESIGN] RULES.md D46: `--help` asks for the usage; a command
/// line that is wrong is refused, saying why.
#[test]
fn a_wrong_command_line_is_refused() {
    let help = parse(&["-c".into(), "x".into(), "--help".into()], &|_| None).unwrap();
    assert!(matches!(help, Command::Help));
    let wrong = |line: &[&str]| parsed(line, &[("USER", "u")]).unwrap_err();
    assert!(wrong(&["--verbose"]).starts_with("--verbose is not an option\n\nusage: turborel"));
    assert_eq!(wrong(&["-c"]), "-c needs a value");
    assert_eq!(wrong(&["-p", "70000"]), "70000 is no port");
    assert_eq!(wrong(&["--read-timeout", "soon"]), "soon is no number of seconds");
    assert_eq!(wrong(&["--connect-timeout", "-1"]), "-1 is no number of seconds");
    let of_tls = "--server-name, --certificate and --key are of TLS: name --tls too";
    assert_eq!(wrong(&["--server-name", "db"]), of_tls);
    assert_eq!(wrong(&["--certificate", "c.pem", "--key", "k.pem"]), of_tls);
    let both = "--certificate and --key name a certificate and its key: both";
    assert_eq!(
        (wrong(&["--tls", "ca.pem", "--certificate", "c.pem"]), wrong(&["--tls", "ca.pem", "--key", "k.pem"])),
        (both.into(), both.into())
    );
    let elsewhere = "--socket names where the server is: not with --host, --port or --tls";
    for other in [["--host", "h"], ["--port", "1"], ["--tls", "ca.pem"]] {
        let line = [&["--socket", "s"][..], &other[..]].concat();
        let said = wrong(&line);
        assert!(said == elsewhere || said.ends_with("Unix only"), "{said}");
    }
}

/// Verifies: [DESIGN] RULES.md D46: `--socket` reaches the server at a
/// Unix socket, on Unix; elsewhere it is refused, saying why.
#[test]
fn a_socket_is_reached_on_unix() {
    let reached = parsed(&["--socket", "/run/turborel.sock"], &[("USER", "u")]);
    #[cfg(unix)]
    assert_eq!(reached.unwrap().server, Server::Socket(PathBuf::from("/run/turborel.sock")));
    #[cfg(not(unix))]
    assert_eq!(reached.unwrap_err(), "--socket: a Unix socket is reached on Unix only");
}
