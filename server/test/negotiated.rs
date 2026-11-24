//! The native protocol's capabilities over a real connection (RULES.md
//! D185 to D196): what is granted of what is asked, and each behaviour a
//! capability adds to the session: status, notices, ping, reset, change
//! of user, attributes, several statements in one request, compression.

use std::net::TcpStream;
use std::sync::{Arc, Mutex};

use turborel::db::Database;
use turborel_client::connection::{Connection, Outcome, Trust};
use turborel_client::told::Offer;
use turborel_protocol::Capabilities;
use turborel_protocol::capability::{ALL, COMPRESSION, DIAGNOSTICS, NOTICES, PING, STATUS};
use turborel_server::settings::COMPRESSION_MIN_BYTES;

use crate::common::{Running, rows, start, state};

/// A database whose schema X holds T, of an INTEGER K, and whose user Y
/// owns schema Y.
fn database() -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION Y").unwrap();
    db
}

/// A connection to `server` as X, asking what `offer` asks.
pub fn offering(server: &Running, offer: Offer) -> Connection<TcpStream> {
    let stream = TcpStream::connect(server.address).unwrap();
    Connection::open_offering(stream, ("X", &mut Trust), offer).unwrap()
}

/// Verifies: [DESIGN] RULES.md D185: a server grants what it serves of
/// what a client asks, and nothing it does not; every capability but
/// compression by default, compression too if asked, and not if
/// SERVER.COMPRESSION_MIN_BYTES is 0; a client that asks none is served
/// the protocol's first form, and a request of a capability not granted
/// is refused by the client before it is sent.
#[test]
fn a_server_grants_what_is_asked_of_what_it_serves() {
    let db = database();
    let server = start(&db, &[]);
    let c = server.connect("X");
    assert_eq!(c.granted(), Capabilities::all().without(COMPRESSION));
    let compressed = offering(&server, Offer::default().compressed());
    assert_eq!(compressed.granted(), Capabilities::all());
    let mut none = offering(&server, Offer::none());
    assert_eq!(none.granted(), Capabilities::NONE);
    assert_eq!(state(&none.ping().unwrap_err()).0, "08006", "not granted, not sent");
    assert_eq!(rows(&none.query("SELECT K FROM T").unwrap()), Vec::<String>::new(), "the first form still serves");
    let odd = Offer { capabilities: vec!["no-such".into(), PING.into()], ..Offer::none() };
    assert_eq!(offering(&server, odd).granted(), Capabilities::of(&[PING]));
    assert_eq!(ALL.len(), 19);
    drop(server);
    let off = start(&db, &[(COMPRESSION_MIN_BYTES, 0)]);
    assert_eq!(offering(&off, Offer::default().compressed()).granted(), Capabilities::all().without(COMPRESSION));
}

/// Verifies: [DESIGN] RULES.md D190: the session's settings are told as it
/// begins, and again as a statement changes one, so `setting` reads what
/// the session runs with at no round trip; a client not granted `status`
/// is told none.
#[test]
fn the_sessions_settings_are_told_as_they_change() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    assert_eq!(c.setting("AUTHORIZATION"), Some("X"));
    assert_eq!(c.setting("SCHEMA"), Some("X"));
    assert_eq!(c.setting("READ_ONLY"), Some("NO"));
    assert_eq!(c.setting("SERVER_READ_ONLY"), Some("NO"), "a database alone takes changes");
    assert_eq!((c.setting("CATALOG"), c.setting("OWNER")), (Some("TURBOREL"), Some("NO")));
    assert_eq!(c.setting("SERVER_VERSION"), Some(turborel::product().as_str()));
    assert!(c.setting("SESSION.TIME_ZONE").is_some());
    c.query("SET TIME ZONE INTERVAL '+05:30' HOUR TO MINUTE").unwrap();
    assert_eq!(c.setting("SESSION.TIME_ZONE"), Some("+05:30"));
    c.query("SET SCHEMA 'Y'").unwrap();
    assert_eq!(c.setting("SCHEMA"), Some("Y"));
    c.query("SET TRANSACTION READ ONLY").unwrap();
    c.query("SELECT 1 FROM X.T").unwrap();
    assert_eq!(c.setting("READ_ONLY"), Some("YES"));
    let none = offering(&server, Offer::none());
    assert_eq!(none.settings().count(), 0);
}

/// Verifies: [DESIGN] [ISO/IEC 9075:1992 §22.1 Table 23] [ISO/IEC 9075-4:2006
/// draft §17.2] RULES.md D187: each warning a statement raised comes as a
/// notice before its reply, to the handler set, or kept with none: a
/// SIGNAL's with its message, the completion's named as its subclass is; a
/// client granted neither notices nor conditions hears none.
#[test]
fn a_statements_warnings_come_as_notices() {
    let db = database();
    let server = start(&db, &[]);
    let offer = Offer { capabilities: vec![NOTICES.into(), STATUS.into(), DIAGNOSTICS.into()], ..Offer::none() };
    let mut c = offering(&server, offer);
    c.query("BEGIN SIGNAL SQLSTATE '01H01' SET MESSAGE_TEXT = 'careful'; INSERT INTO T VALUES (NULL); END").unwrap();
    let kept = c.take_notices();
    assert_eq!(kept.iter().map(|n| (n.sqlstate.as_str(), n.message.as_str())).collect::<Vec<_>>(), [("01H01", "careful")]);
    let heard = Arc::new(Mutex::new(Vec::new()));
    let into = Arc::clone(&heard);
    c.set_notice_handler(Some(Box::new(move |n| into.lock().unwrap().push(n.clone()))));
    c.query("SELECT SUM(K) FROM T").unwrap();
    let heard = heard.lock().unwrap().clone();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(heard[0].sqlstate, "01003");
    assert_eq!(heard[0].message, "the statement completed with warning: null value eliminated in set function");
    assert_eq!(heard[0].diagnostics.severity.as_deref(), Some("WARNING"));
    let mut deaf = offering(&server, Offer::none());
    deaf.query("SELECT SUM(K) FROM T").unwrap();
    assert_eq!(deaf.take_notices().len(), 0);
}

/// Verifies: [DESIGN] RULES.md D189: Ping is answered; Reset puts the
/// session back as it was at login, its transaction rolled back with a
/// notice that says so, its prepared statements and settings gone, its user
/// kept; ChangeUser authenticates the connection again as another user,
/// whose session it then is.
#[test]
fn a_session_is_pinged_reset_and_changed() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    c.ping().unwrap();
    let statement = c.prepare("SELECT K FROM T").unwrap();
    c.query("SET TIME ZONE INTERVAL '+03:00' HOUR TO MINUTE").unwrap();
    c.query("INSERT INTO T VALUES (1)").unwrap();
    assert!(c.in_transaction());
    c.reset().unwrap();
    assert!(!c.in_transaction());
    let notices = c.take_notices();
    assert_eq!(notices.iter().map(|n| n.sqlstate.as_str()).collect::<Vec<_>>(), ["01T01"], "rolled back, and told");
    assert_ne!(c.setting("SESSION.TIME_ZONE"), Some("+03:00"), "settings as at login");
    assert_eq!(state(&c.execute(&statement, &[]).unwrap_err()).0, "26000", "prepared statements gone");
    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["0"]);
    c.query("INSERT INTO T VALUES (2)").unwrap();
    assert!(c.in_transaction());
    c.change_user("Y", &mut Trust).unwrap();
    assert!(!c.in_transaction(), "renewed as Reset renews it, its transaction rolled back");
    assert_eq!(c.authorization(), "Y");
    assert_eq!(c.setting("AUTHORIZATION"), Some("Y"));
    assert_eq!(state(&c.query("SELECT K FROM X.T").unwrap_err()).0, "42000", "Y holds no privilege on X.T");
    c.terminate().unwrap();
}

/// Verifies: [DESIGN] RULES.md D191: a request's attributes are read in SQL
/// by QUERY_ATTRIBUTE, null for one not given; a connection's attributes
/// are shown to the database's owner in
/// INFORMATION_SCHEMA.CONNECTION_ATTRIBUTES, and its `application` in
/// CONNECTIONS, a connection of settings naming its `application_name`
/// so.
#[test]
fn attributes_are_read_in_sql_and_shown() {
    let db = database();
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    let server = start(&db, &[]);
    let mut owner = server.connect("OWNER");
    let mut c = offering(&server, Offer::default().attribute("application", "billing").attribute("region", "eu"));
    c.set_query_attributes(vec![("traceparent".into(), "00-4bf9-00f0-01".into())]).unwrap();
    let read = c.query("VALUES (QUERY_ATTRIBUTE('traceparent'), QUERY_ATTRIBUTE('other'))").unwrap();
    assert_eq!(rows(&read), ["00-4bf9-00f0-01,NULL"]);
    c.set_query_attributes(Vec::new()).unwrap();
    assert_eq!(rows(&c.query("VALUES (QUERY_ATTRIBUTE('traceparent'))").unwrap()), ["NULL"], "none from now on");
    let shown = owner.query("SELECT ATTRIBUTE_NAME, ATTRIBUTE_VALUE FROM INFORMATION_SCHEMA.CONNECTION_ATTRIBUTES WHERE ATTRIBUTE_NAME <> 'none' ORDER BY 1").unwrap();
    assert_eq!(rows(&shown), ["application,billing", "region,eu"]);
    let application = owner.query("SELECT APPLICATION FROM INFORMATION_SCHEMA.CONNECTIONS ORDER BY 1").unwrap();
    assert_eq!(rows(&application), ["billing", "NULL"], "the owner's own gave none");
    let uri = format!("turborel://X@{}/?tls_mode=disable&application_name=reports", server.address);
    let options = turborel_client::settings::Options::from_uri(&uri).unwrap();
    let _named = turborel_client::settings::Settings::resolve_in(&options, |_| None).unwrap().connect().unwrap();
    let application = owner.query("SELECT APPLICATION FROM INFORMATION_SCHEMA.CONNECTIONS WHERE APPLICATION = 'reports'").unwrap();
    assert_eq!(rows(&application), ["reports"], "the settings' application_name sent as the attribute");
}

/// Verifies: [DESIGN] RULES.md D192: a Query of several statements runs
/// each in order, each reply its own, the last marked; the first that
/// fails stops the rest; empty text completes.
#[test]
fn several_statements_run_in_one_request() {
    let db = database();
    let server = start(&db, &[]);
    let mut c = server.connect("X");
    let ran = c.query_many("INSERT INTO T VALUES (1); SELECT K FROM T; INSERT INTO T VALUES ('x'); INSERT INTO T VALUES (3)").unwrap();
    assert_eq!(ran.outcomes.len(), 2, "{ran:?}");
    assert!(matches!(&ran.outcomes[1], Outcome::Rows { rows, .. } if rows.len() == 1));
    assert_eq!(state(ran.failure.as_ref().unwrap()).0, "42000", "the third stopped the rest");
    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["1"]);
    assert_eq!(c.query_many("").unwrap().outcomes.len(), 1);
}

/// Verifies: [DESIGN] RULES.md D196: granted compression, long requests and
/// replies travel compressed and read back whole.
#[test]
fn long_frames_travel_compressed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE S (V VARCHAR(20000))").unwrap();
    let server = start(&db, &[(COMPRESSION_MIN_BYTES, 64)]);
    let mut c = offering(&server, Offer::default().compressed());
    let long = "abc".repeat(5000);
    c.query(&format!("INSERT INTO S VALUES ('{long}')")).unwrap();
    let back = c.query("SELECT V FROM S").unwrap();
    assert_eq!(rows(&back), [long]);
}
