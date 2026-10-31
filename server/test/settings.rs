//! The server's settings: the database's own.

/// Verifies: [DESIGN] RULES.md D36, D42: the server's settings are the
/// database's: listed with the others, where it listens taking effect as
/// it starts and its limits for each statement; a value out of range is
/// refused (SQLCODE -910); a database served again keeps them declared
/// once. Its built-in protocol is named "native".
#[test]
fn the_servers_settings_are_the_databases() {
    use turborel::settings::Effect;
    use turborel_server::server::Server;
    use turborel_server::settings::{HOST, PORT, WORKERS};
    let db = turborel::db::Database::new();
    drop(Server::new(db.clone()).unwrap());
    drop(Server::new(db.clone()).unwrap());
    let listed = db.settings().unwrap();
    let effect = |name: &str| listed.iter().find(|l| l.definition.name == name).map(|l| l.definition.effect);
    assert_eq!((effect(HOST), effect(PORT), effect(WORKERS)), (Some(Effect::Start), Some(Effect::Start), Some(Effect::Statement)));
    assert_eq!(listed.iter().filter(|l| l.definition.name.starts_with("SERVER.")).count(), 25);
    assert_eq!(db.setting(PORT).unwrap().to_string(), "7422");
    assert_eq!(turborel_server::server::Protocol::name(&turborel_server::native::Native), "native");
    let refused = db.set_setting(WORKERS, &turborel::value::Value::Exact(turborel::value::Decimal::new(0, 0))).unwrap_err();
    assert_eq!(refused.code, turborel::error::Code::Api);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §5.1, §6.2]; [DESIGN] RULES.md
/// D36, D42, D43: the owner, connected, configures the running server in
/// SQL: SET DATABASE of a server's limit takes effect at once, a
/// connection past it refused (SQLSTATE 08004); TO DEFAULT lifts it again.
/// Another user may not (SQLCODE -104).
#[test]
fn the_owner_configures_a_running_server() {
    use turborel_client::connection::{self, Trust};
    use turborel_server::settings::MAX_CONNECTIONS;

    use crate::common::{done, start, state};
    let db = turborel::db::Database::new();
    db.create_user("ANN", b"").unwrap();
    db.set_owner("ANN").unwrap();
    let server = start(&db, &[]);
    let mut ann = server.connect("ann");
    let mut bob = server.connect("bob");
    assert_eq!(state(&bob.query("SET DATABASE SERVER.MAX_CONNECTIONS = 1").unwrap_err()), ("2A000", -104));
    assert_eq!(done(&ann.query("SET DATABASE SERVER.MAX_CONNECTIONS = 2").unwrap()), 0);
    assert_eq!(db.setting(MAX_CONNECTIONS).unwrap().to_string(), "2");
    let refused = connection::connect(server.address, "carol", &mut Trust).err().unwrap();
    assert_eq!(state(&refused).0, "08004", "two open: no third");
    assert_eq!(done(&ann.query("SET DATABASE SERVER.MAX_CONNECTIONS TO DEFAULT").unwrap()), 0);
    connection::connect(server.address, "carol", &mut Trust).unwrap().terminate().unwrap();
    bob.terminate().unwrap();
}

/// Verifies: [DESIGN] RULES.md D36, D42: a setting the server reads as it
/// starts shows the value it started with, and, once set apart from it,
/// that it waits for a restart; one read at once shows neither; the
/// longest message is read as each connection is taken. Regression test
/// (R-TEST-7): the value set showed as if in effect, and the longest
/// message as taken for each statement.
#[test]
fn a_setting_read_as_the_server_starts_says_a_change_waits() {
    use crate::common::{rows, start};
    use turborel_server::settings::{MAX_MESSAGE_BYTES, PORT, WORKERS};
    let db = turborel::db::Database::new();
    let server = start(&db, &[]);
    let mut c = server.connect("ann");
    let shown = |c: &mut turborel_client::connection::Connection<std::net::TcpStream>, name: &str| {
        let sql = format!(
            "SELECT TRIM(SETTING_VALUE) || ',' || COALESCE(TRIM(STARTED_VALUE), 'NULL') || ',' || IS_PENDING || ',' || TRIM(TAKES_EFFECT) \
             FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME = '{name}'"
        );
        rows(&c.query(&sql).unwrap())
    };
    assert_eq!(shown(&mut c, PORT), ["0,0,NO,START"]);
    db.set_setting(PORT, &turborel::value::Value::Exact(turborel::value::Decimal::new(7000, 0))).unwrap();
    assert_eq!(shown(&mut c, PORT), ["7000,0,YES,START"], "waits for a restart");
    assert_eq!(shown(&mut c, WORKERS).first().map(|r| r.ends_with(",NULL,NO,STATEMENT")), Some(true));
    assert_eq!(shown(&mut c, MAX_MESSAGE_BYTES), ["16777216,NULL,NO,CONNECTION"]);
    c.terminate().unwrap();
}

/// The words Appendix C gives `effect` in.
fn effect_words(effect: turborel::settings::Effect) -> &'static str {
    use turborel::settings::Effect;
    match effect {
        Effect::Statement => "at once",
        Effect::Transaction => "from the next transaction",
        Effect::Connection => "from the next connection",
        Effect::Start => "at the next start",
        Effect::Creation => "when the database is made",
    }
}

/// Appendix C's table and sentences of `listed`, as the page prints them:
/// the default of SERVER.WORKERS, the machine's processors, in words.
fn appendix_section(listed: &[&turborel::settings::Listed]) -> String {
    use std::fmt::Write as _;
    let mut table = String::from("| Setting | Default | Values | Takes effect | |\n|---|---|---|---|---|\n");
    let mut sentences = String::new();
    for l in listed {
        let d = &l.definition;
        let mut default = d.default.to_string();
        if d.name == turborel_server::settings::WORKERS {
            "the processors".clone_into(&mut default);
        }
        if default.is_empty() {
            "(empty)".clone_into(&mut default);
        }
        let session = if d.per_session { "*session*" } else { "" };
        writeln!(table, "| {} | {default} | {} | {} | {session} |", d.name, d.kind.described(), effect_words(d.effect)).unwrap();
        writeln!(sentences, "- **{}**: {}", d.name, d.doc).unwrap();
    }
    format!("{table}\nEach in a sentence:\n\n{sentences}")
}

/// Verifies: [TOOL] R-DOC-8; [DESIGN] RULES.md D36: Appendix C lists every
/// setting the database and its server declare, in the order of their
/// names, the database's then the server's, each with its default, the
/// values it takes, when a change takes effect, whether a session sets
/// it, and its sentence, as INFORMATION_SCHEMA.SETTINGS has them; and no
/// setting that is not declared.
#[test]
fn appendix_c_lists_every_setting_as_declared() {
    let db = turborel::db::Database::new();
    turborel_server::settings::declare(&db).unwrap();
    let mut listed = db.settings().unwrap();
    listed.sort_by(|a, b| a.definition.name.cmp(&b.definition.name));
    let (server, own): (Vec<_>, Vec<_>) = listed.iter().partition(|l| l.definition.name.starts_with("SERVER."));
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../doc/appendix/c-settings.md");
    let page = std::fs::read_to_string(path).unwrap();
    for (heading, section) in
        [("## The database's settings", appendix_section(&own)), ("## The server's settings", appendix_section(&server))]
    {
        let expected = format!("{heading}\n\n{section}");
        assert!(page.contains(&expected), "Appendix C should have, as declared:\n{expected}");
    }
    let rows = page.lines().filter(|l| l.starts_with("| ") && !l.starts_with("| Setting ")).count();
    let sentences = page.lines().filter(|l| l.starts_with("- **")).count();
    assert_eq!((rows, sentences), (listed.len(), listed.len()), "no setting listed that is not declared");
}
