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
    assert_eq!(listed.iter().filter(|l| l.definition.name.starts_with("SERVER.")).count(), 16);
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
