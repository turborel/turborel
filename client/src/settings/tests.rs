//! Unit tests of connection settings: the URI, the service file, the
//! password file, the environment, and the order the layers are laid in
//! (RULES.md D202, D203).

use std::collections::BTreeMap;

use super::*;

/// An environment of `pairs`, as `Settings::resolve_in` reads one.
fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let map: BTreeMap<String, String> = pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    move |name| map.get(name).cloned()
}

/// A scratch file of `text`, named for `test`, in the system's temporary
/// directory.
fn scratch(test: &str, contents: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("turborel-client-{test}-{}", std::process::id()));
    std::fs::write(&path, contents).unwrap();
    path
}

/// Verifies: [DESIGN] RULES.md D202: a URI gives its user, password,
/// hosts each of its port, an IPv6 address in brackets, a local socket
/// percent-encoded, and its query's settings; percent-encoding is decoded
/// in each part.
#[test]
fn a_uri_gives_its_user_hosts_and_settings() {
    let uri = "turborel://ann:p%40ss@db1:7000,[::1]:7001,db3,%2Frun%2Fs.sock/?tls_mode=verify-full&target=read-write&connect_timeout=5";
    let options = Options::from_uri(uri).unwrap();
    assert_eq!(options.get(USER), Some("ann"));
    assert_eq!(options.get(PASSWORD), Some("p@ss"));
    let settings = Settings::resolve_in(&options, environment(&[])).unwrap();
    assert_eq!(
        settings.hosts,
        [
            Host::Tcp("db1".to_owned(), 7000),
            Host::Tcp("::1".to_owned(), 7001),
            Host::Tcp("db3".to_owned(), DEFAULT_PORT),
            Host::Local(PathBuf::from("/run/s.sock"))
        ]
    );
    assert_eq!((settings.tls_mode, settings.target), (TlsMode::VerifyFull, TargetRole::ReadWrite));
    assert_eq!(settings.connect_timeout, Some(Duration::from_secs(5)));
    assert!(!format!("{options:?} {settings:?}").contains("p@ss"), "the password is never shown");
    let bare = Settings::resolve_in(&Options::from_uri("turborel://ann@/").unwrap(), environment(&[])).unwrap();
    assert_eq!(bare.hosts, [Host::Tcp(DEFAULT_HOST.to_owned(), DEFAULT_PORT)]);
}

/// Verifies: [DESIGN] RULES.md D202: a URI of another scheme, of a path,
/// of a key that names no setting, of a pair without `=`, or of a `%` not
/// followed by two hexadecimal digits is refused (SQLSTATE 08001); so is
/// a word no setting takes, a port that is no number, and no user.
#[test]
fn a_malformed_uri_or_setting_is_refused() {
    for uri in ["postgres://a@b", "turborel://a@b/db", "turborel://a@b/?colour=red", "turborel://a@b/?tls_mode", "turborel://a%4@b"] {
        assert_eq!(Options::from_uri(uri).unwrap_err().sqlstate, "08001", "{uri}");
    }
    for uri in ["turborel://a@b/?tls_mode=always", "turborel://a@b:port", "turborel://a@b/?connect_timeout=-1", "turborel://b"] {
        assert_eq!(Settings::resolve_in(&Options::from_uri(uri).unwrap(), environment(&[])).unwrap_err().sqlstate, "08001", "{uri}");
    }
}

/// Verifies: [DESIGN] RULES.md D202: a setting the program gives prevails
/// over the service's, which prevails over the environment's, which
/// prevails over the default; the environment names the service and its
/// file when the program does not; an empty variable gives nothing.
#[test]
fn settings_are_laid_program_service_environment_default() {
    let file = scratch("precedence", "# servers\n[prod]\nhost = prod-db\nport = 7500\ntls_mode = require\n\n[test]\nhost = test-db\n");
    let env = environment(&[
        ("TURBOREL_SERVICE", "prod"),
        ("TURBOREL_SERVICE_FILE", file.to_str().unwrap()),
        ("TURBOREL_USER", "env-user"),
        ("TURBOREL_PORT", "7600"),
        ("TURBOREL_TLS_MODE", "disable"),
        ("TURBOREL_APPLICATION_NAME", ""),
    ]);
    let mut explicit = Options::new();
    explicit.set(TLS_MODE, "verify-ca").unwrap();
    let settings = Settings::resolve_in(&explicit, &env).unwrap();
    assert_eq!(settings.hosts, [Host::Tcp("prod-db".to_owned(), 7500)], "the service over the environment");
    assert_eq!(settings.tls_mode, TlsMode::VerifyCa, "the program over the service");
    assert_eq!(settings.user, "env-user", "the environment where the service says nothing");
    assert_eq!(settings.application_name, None);
    assert_eq!(settings.channel_binding, ChannelBinding::Prefer, "the default");
    explicit.set(SERVICE, "test").unwrap();
    let chosen = Settings::resolve_in(&explicit, &env).unwrap();
    assert_eq!(chosen.hosts, [Host::Tcp("test-db".to_owned(), 7600)], "the program names the service");
    std::fs::remove_file(file).unwrap();
}

/// Verifies: [DESIGN] RULES.md D203: a service file's section is its
/// lines' settings; a section that is not there, a setting before any
/// section, a line without `=`, a key that names no setting, and a
/// service or service file set by a service are refused, naming the line.
#[test]
fn a_service_file_is_read_by_section() {
    let file = Path::new("services.conf");
    let options = files::service("[a]\nuser=ann\n[b]\nuser = bob\n  host =  h  \n", "b", file).unwrap();
    assert_eq!((options.get(USER), options.get(HOST)), (Some("bob"), Some("h")));
    assert!(files::service("[a]\nuser=ann\n", "b", file).unwrap_err().message.contains("has no service b"));
    for (text, why) in [
        ("user=ann\n", "line 1: a setting comes before any section"),
        ("[a]\nuser\n", "line 2: a setting is <key> = <value>"),
        ("[a]\ncolour=red\n", "line 2: colour is no connection setting"),
        ("[a]\nservice=b\n", "line 2: a service cannot set service"),
        ("[a\n", "line 1: a section's name ends with ]"),
    ] {
        let refused = files::service(text, "a", file).unwrap_err();
        assert!(refused.message.contains(why), "{text:?}: {}", refused.message);
    }
}

/// Verifies: [DESIGN] RULES.md D203: the password file gives the password
/// of its first line matching host, port and user, `*` matching any,
/// `\:` and `\\` taken as they are, an escaped `\*` matching only `*`;
/// comments and short lines are skipped; the settings' own password
/// prevails over it.
#[test]
fn a_password_file_matches_by_host_port_and_user() {
    let text = "# passwords\nshort:line\ndb1:7422:ann:first\n*:*:ann:any\\:host\ndb\\:2:*:bob:c:\\\\d\n\\*:1:carl:star\n";
    assert_eq!(files::password(text, ("db1", "7422", "ann")).as_deref(), Some("first"));
    assert_eq!(files::password(text, ("db9", "1", "ann")).as_deref(), Some("any:host"));
    assert_eq!(files::password(text, ("db:2", "7", "bob")).as_deref(), Some("c:\\d"));
    assert_eq!(files::password(text, ("x", "1", "carl")), None, "an escaped * is a star");
    assert_eq!(files::password(text, ("*", "1", "carl")).as_deref(), Some("star"));
    assert_eq!(files::password(text, ("db1", "7422", "nobody")), None);
    let file = scratch("passfile", text);
    let mut explicit = Options::new();
    explicit.set(USER, "ann").unwrap().set(PASSFILE, file.to_str().unwrap()).unwrap().set(HOST, "db1").unwrap();
    let settings = Settings::resolve_in(&explicit, environment(&[])).unwrap();
    assert_eq!(settings.password_for(&Host::Tcp("db1".to_owned(), 7422)).as_deref(), Some("first"));
    explicit.set(PASSWORD, "given").unwrap();
    let given = Settings::resolve_in(&explicit, environment(&[])).unwrap();
    assert_eq!(given.password_for(&Host::Tcp("db1".to_owned(), 7422)).as_deref(), Some("given"));
    std::fs::remove_file(&file).unwrap();
    assert_eq!(settings.password_for(&Host::Tcp("db1".to_owned(), 7422)), None, "a file that cannot be read gives none");
}

/// Verifies: [DESIGN] RULES.md D202: every variable of the environment
/// gives its setting, and the words of each kind are read as named.
#[test]
fn the_environment_and_words_are_read() {
    let pairs: Vec<(&str, &str)> = ENVIRONMENT.iter().map(|(name, _)| (*name, "v")).collect();
    let options = Options::environment(environment(&pairs));
    assert!(ENVIRONMENT.iter().all(|(_, key)| options.get(key) == Some("v")));
    assert_eq!(TlsMode::parse(TLS_MODE, "prefer").unwrap(), TlsMode::Prefer);
    assert!(TlsMode::VerifyCa.verifies() && !TlsMode::Require.verifies());
    assert_eq!(ChannelBinding::parse(CHANNEL_BINDING, "require").unwrap(), ChannelBinding::Require);
    assert_eq!(LoadBalance::parse(LOAD_BALANCE, "random").unwrap(), LoadBalance::Random);
    assert!(TargetRole::parse(TARGET, "primary").unwrap_err().message.contains("any, read-write, read-only"));
    let mut both = Options::new();
    both.set(HOST, "a,b").unwrap().set(PORT, "1,2").unwrap().set(USER, "u").unwrap().set(TLS_SYSTEM_ROOTS, "true").unwrap();
    let settings = Settings::resolve_in(&both, environment(&[])).unwrap();
    assert_eq!(settings.hosts, [Host::Tcp("a".to_owned(), 1), Host::Tcp("b".to_owned(), 2)]);
    assert!(settings.tls.system_roots);
}
