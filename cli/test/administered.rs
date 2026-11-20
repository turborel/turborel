//! `turborel admin`'s commands that the first admin test does not reach:
//! the host plug-ins, TLS's files, a schema dropped only when the
//! question is answered, and a password typed wrong (RULES.md D218).

use turborel::db::Database;
use turborel_cli::Command;
use turborel_cli::admin::{Confirm, Console};
use turborel_cli::{Dialer, Secret};
use turborel_client::agent::Agent;

use crate::common::{Running, pki, start};

/// A server of a database X owns, where X has schema S2.
fn served() -> (Database, Running) {
    let db = Database::new();
    db.create_user("X", b"x").unwrap();
    db.set_owner("X").unwrap();
    db.schema("CREATE SCHEMA S2 AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let server = start(&db, &[]);
    (db, server)
}

/// What answers the questions and types the passwords asked for, if
/// anything does.
struct Asked<'a> {
    confirm: Option<Confirm<'a>>,
    secret: Option<Secret<'a>>,
}

/// Runs `turborel admin` with `words` against `server`, as `asked`
/// answers: what it wrote, or why it failed.
fn admin(server: &Running, words: &[&str], mut asked: Asked<'_>) -> Result<String, String> {
    let port = server.address.port().to_string();
    let mut line: Vec<String> = ["admin", "-U", "X", "-p", port.as_str()].iter().map(|a| (*a).to_owned()).collect();
    line.extend(words.iter().map(|w| (*w).to_owned()));
    let Ok(Command::Admin(invocation)) = turborel_cli::parse(&line, &|_| None) else { panic!("{line:?}") };
    let connection = turborel_cli::connect(&invocation.options, || Ok(String::new())).unwrap();
    let mut agent = Agent::connected(Dialer::new(invocation.options.clone(), Box::new(|| Ok(String::new()))), connection);
    let mut out = Vec::new();
    let confirm = asked.confirm.as_deref_mut().map(|c| c as &mut dyn FnMut(&str) -> std::io::Result<bool>);
    let secret = asked.secret.as_deref_mut().map(|s| s as &mut dyn FnMut(&str) -> std::io::Result<String>);
    let mut console = Console { out: &mut out, confirm, secret };
    let done = turborel_cli::admin::run(&mut agent, &invocation, &mut console);
    agent.terminate().unwrap();
    done.map(|()| String::from_utf8(out).unwrap())
}

/// Nothing to answer with.
const UNASKED: Asked<'static> = Asked { confirm: None, secret: None };

/// Verifies: [DESIGN] RULES.md D218: `host-plugins` lists the host
/// plug-ins; `tls` sets the server's certificate chain and key files,
/// saying they are taken for the connections after; `tls` without both
/// files is refused.
#[test]
fn host_plugins_are_listed_and_tls_files_set() {
    let (db, server) = served();
    assert!(admin(&server, &["host-plugins"], UNASKED).is_ok());
    let pki = pki("cli-admin-tls");
    let (chain, key) = (pki.chain.display().to_string(), pki.key.display().to_string());
    assert_eq!(admin(&server, &["tls", &chain, &key], UNASKED).unwrap(), "The files are taken for the connections after.\n");
    let set = db.setting(turborel_server::settings::TLS_CERTIFICATE).unwrap();
    let turborel::value::Value::Char(set) = set else { panic!("{set:?}") };
    assert_eq!(set, chain);
    let port = server.address.port().to_string();
    let line: Vec<String> = ["admin", "-U", "X", "-p", &port, "tls", &chain].iter().map(|a| (*a).to_owned()).collect();
    assert_eq!(turborel_cli::parse(&line, &|_| None).err().unwrap(), "tls needs the files of the certificate chain and of the key");
}

/// Verifies: [DESIGN] RULES.md D218: `drop-schema` asks first: with no
/// terminal to ask at it is refused, saying to give --yes; answered no,
/// the schema is not dropped; with --cascade the question says all it
/// holds goes too, and answered yes it is dropped, its tables with it.
#[test]
fn a_schema_is_dropped_only_when_asked_and_answered() {
    let (_db, server) = served();
    let unasked = admin(&server, &["drop-schema", "S2", "--cascade"], UNASKED).unwrap_err();
    assert_eq!(unasked, "drop-schema asks before it drops S2; give --yes to drop it unasked");
    let mut no = |_: &str| Ok(false);
    let kept = admin(&server, &["drop-schema", "S2", "--cascade"], Asked { confirm: Some(&mut no), secret: None });
    assert_eq!(kept.unwrap_err(), "S2 is not dropped");
    let mut questions = Vec::new();
    let mut yes = |q: &str| {
        questions.push(q.to_owned());
        Ok(true)
    };
    let dropped = admin(&server, &["drop-schema", "S2", "--cascade"], Asked { confirm: Some(&mut yes), secret: None });
    assert_eq!(dropped.unwrap(), "Schema S2 is dropped.\n");
    assert_eq!(questions, ["Drop the schema S2 and all it holds? Type y to drop it: "]);
}

/// Verifies: [DESIGN] RULES.md D218: `password` typed twice differently
/// is not changed, saying so; one that cannot be read says why.
#[test]
fn a_password_typed_wrong_is_not_changed() {
    let (_db, server) = served();
    let mut typed = vec!["two", "one"];
    let mut differ = |_: &str| Ok(typed.pop().unwrap().to_owned());
    let differs = admin(&server, &["password"], Asked { confirm: None, secret: Some(&mut differ) });
    assert_eq!(differs.unwrap_err(), "the two do not match; the password is not changed");
    let mut broken = |_: &str| Err(std::io::Error::other("no tty"));
    let unread = admin(&server, &["password", "X"], Asked { confirm: None, secret: Some(&mut broken) });
    assert_eq!(unread.unwrap_err(), "no tty");
}
