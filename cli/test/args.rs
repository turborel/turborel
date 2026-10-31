//! The command line: where the server is, who connects, what is run.

use std::path::PathBuf;

use std::time::Duration;

use turborel_cli::{Command, Credentials, DEFAULT_PORT, Importing, Input, Options, PASSWORD_VARIABLE, Server, Tls, parse};
use turborel_client::import::{Dialect, Import};
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

/// Verifies: [DESIGN] RULES.md D59: `--import` names a file whose rows are
/// imported, in turn with the statements named, into the table `--table`
/// names, as CSV unless `--format` says tab-separated, any case, with the
/// header, delimiter, batch and skipping named; null written as `--null`
/// says. By default: CSV, no header, 1000 rows a batch, stopping at a bad
/// row.
#[test]
fn imports_are_named() {
    let line = ["-c", "SELECT 1", "--import", "a.tsv", "--table", "X.T", "--format", "TSV", "--header", "--delimiter", ";"];
    let more = ["--batch", "5", "--skip-bad-rows", "--null", "NULL", "--import", "b.tsv"];
    let options = parsed(&[&line[..], &more[..]].concat(), &[("USER", "u")]).unwrap();
    let inputs = [Input::Command("SELECT 1".into()), Input::Import(PathBuf::from("a.tsv")), Input::Import(PathBuf::from("b.tsv"))];
    assert_eq!(options.inputs, inputs);
    let dialect = Dialect::tsv().with_delimiter(';').unwrap().with_header().with_null("NULL");
    let into = Import { table: "X.T".into(), batch: 5, skip: true };
    assert_eq!(options.import, Some(Importing { into, dialect }));
    let plain = parsed(&["--import", "a.csv", "--table", "T", "--format", "csv"], &[("USER", "u")]).unwrap();
    assert_eq!(plain.import, Some(Importing { into: Import::into("T"), dialect: Dialect::csv().with_null("") }));
    assert_eq!(Import::into("T").batch, 1000);
    assert_eq!(parsed(&[], &[("USER", "u")]).unwrap().import, None);
}

/// Verifies: [DESIGN] RULES.md D59: an import's command line that is wrong
/// is refused, saying why: no table; an import's options without one; a
/// format not known; a delimiter not one character, or a quote; a batch
/// not a number.
#[test]
fn a_wrong_import_is_refused() {
    let wrong = |line: &[&str]| parsed(line, &[("USER", "u")]).unwrap_err();
    assert_eq!(wrong(&["--import", "a.csv"]), "--import needs the table its rows go into: --table NAME");
    let without = "--table, --format, --header, --delimiter, --batch and --skip-bad-rows are of an import: name --import FILE too";
    for alone in
        [&["--table", "T"][..], &["--format", "csv"], &["--header"], &["--delimiter", ";"], &["--batch", "1"], &["--skip-bad-rows"]]
    {
        assert_eq!(wrong(alone), without, "{alone:?}");
    }
    let import = ["--import", "a.csv", "--table", "T"];
    let with = |more: &[&str]| wrong(&[&import[..], more].concat());
    assert_eq!(with(&["--format", "json"]), "json is no format: csv or tsv");
    assert_eq!(with(&["--delimiter", ";;"]), "--delimiter is one character, not \";;\"");
    assert_eq!(with(&["--delimiter", ""]), "--delimiter is one character, not \"\"");
    assert!(with(&["--delimiter", "\""]).starts_with("[DESIGN] a delimiter is one ASCII character"));
    assert_eq!(with(&["--batch", "many"]), "many is no number of rows");
}
