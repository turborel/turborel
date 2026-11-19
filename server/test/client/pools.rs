//! Pools of connections (RULES.md D206): bounded under many threads,
//! waiting at most their limit, each connection given back reset to its
//! login, one that no longer answers or has lived its time closed.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use std::net::TcpStream;
use turborel::db::Database;

use turborel_client::connection::{Connection, Trust};
use turborel_client::pool::{Pool, PoolConfig, Pooled, RESET_MOST_STATEMENTS, Source};
use turborel_client::settings::{Options, Settings};
use turborel_client::target::{Duplex, Target, connect_to};
use turborel_client::told::Offer;
use turborel_protocol::capability::Capabilities;

use crate::common::{Running, done, rows, start};

/// A server whose user X has the table T.
fn served() -> (Database, Running) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER)").unwrap();
    let server = start(&db, &[]);
    (db, server)
}

/// A pool of at most `max` connections to `server` as X, waiting `wait`.
fn pool(server: &Running, max: usize, wait: Duration) -> Pool<Settings> {
    let uri = format!("turborel://X@{}/?tls_mode=disable", server.address);
    let settings = Settings::resolve_in(&Options::from_uri(&uri).unwrap(), |_| None).unwrap();
    Pool::new(settings, PoolConfig { max, max_idle: max, wait, ..PoolConfig::default() })
}

/// Verifies: [DESIGN] RULES.md D206: eight threads borrowing from a pool
/// of three, again and again, each wait for a connection to come back;
/// never more than three are made, each borrower's statements run, and
/// as the threads end every connection made is idle.
#[test]
fn a_pool_bounds_its_connections_under_threads() {
    let (_db, server) = served();
    let pool = pool(&server, 3, Duration::from_secs(30));
    let peak = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let (pool, peak) = (pool.clone(), Arc::clone(&peak));
            std::thread::spawn(move || {
                for _ in 0..5 {
                    let mut c = pool.get().unwrap();
                    peak.fetch_max(pool.counts().unwrap().0, Ordering::SeqCst);
                    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["0"]);
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert!(peak.load(Ordering::SeqCst) <= 3, "{peak:?}");
    let (made, idle) = pool.counts().unwrap();
    assert!(made <= 3 && made == idle, "{made} {idle}");
}

/// Verifies: [DESIGN] RULES.md D206: a pool whose every connection is
/// handed out waits its limit, then fails (SQLSTATE 08001); the
/// connection given back is handed out again, none more made.
#[test]
fn an_exhausted_pool_waits_its_limit() {
    let (_db, server) = served();
    let pool = pool(&server, 1, Duration::from_millis(50));
    let held = pool.get().unwrap();
    let waited = pool.get().err().unwrap();
    assert_eq!(waited.sqlstate, "08001");
    assert!(waited.message.contains("came back"), "{}", waited.message);
    drop(held);
    let again = pool.get().unwrap();
    assert_eq!(pool.counts().unwrap(), (1, 0));
    drop(again);
}

/// Verifies: [DESIGN] RULES.md D206, D189: a connection given back is
/// reset to its login, by Reset of a server that grants it: its
/// transaction rolled back, the rows it wrote gone; its prepared
/// statements destroyed, so the next borrower prepares afresh, more of
/// them than statements would destroy kept the connection; its schema its
/// user's again, autocommit off.
#[test]
fn a_connection_given_back_is_reset() {
    let (_db, server) = served();
    let pool = pool(&server, 1, Duration::from_secs(30));
    let mut c = pool.get().unwrap();
    for _ in 0..=RESET_MOST_STATEMENTS {
        c.prepare("VALUES 1").unwrap();
    }
    given_back_reset(&pool, c);
}

/// Verifies: [DESIGN] RULES.md D206: of a server that grants neither Ping
/// nor Reset, a connection is checked by ROLLBACK and reset by statements,
/// to the same login; with too many statements left prepared, it is
/// closed rather than kept.
#[test]
fn a_connection_granted_nothing_is_reset_by_statements() {
    let (_db, server) = served();
    let address = server.address;
    let connect = move || {
        let stream: Box<dyn Duplex> = Box::new(TcpStream::connect(address).unwrap());
        Connection::open_offering(stream, ("X", &mut Trust), Offer::none())
    };
    let pool = Pool::new(connect, PoolConfig { max: 1, max_idle: 1, ..PoolConfig::default() });
    let c = pool.get().unwrap();
    assert_eq!(c.granted(), Capabilities::NONE);
    given_back_reset(&pool, c);
    let mut many = pool.get().unwrap();
    for _ in 0..=RESET_MOST_STATEMENTS {
        many.prepare("VALUES 1").unwrap();
    }
    drop(many);
    assert_eq!(pool.counts().unwrap(), (0, 0), "closed rather than reset");
}

/// Asserts `c`, of `pool`, given back is reset.
fn given_back_reset<S: Source>(pool: &Pool<S>, mut c: Pooled<S>) {
    c.query("INSERT INTO T VALUES (1)").unwrap();
    let statement = c.prepare("SELECT K FROM T").unwrap();
    c.open_cursor(&statement, &[], 1).unwrap();
    c.query("SET SCHEMA 'ELSEWHERE'").unwrap_or_else(|f| panic!("{f:?}"));
    c.set_autocommit(true);
    assert!(c.in_transaction());
    drop(c);
    let mut again = pool.get().unwrap();
    assert!(!again.in_transaction() && !again.autocommit());
    assert_eq!(rows(&again.query("SELECT COUNT(*) FROM T").unwrap()), ["0"], "rolled back, and T found in X");
    let fresh = again.prepare("SELECT K FROM T").unwrap();
    assert_eq!(rows(&again.execute(&fresh, &[]).unwrap()), Vec::<String>::new());
    assert_eq!(pool.counts().unwrap(), (1, 0), "the same connection");
}

/// Verifies: [DESIGN] RULES.md D206: an idle connection its server ended
/// fails its check as it is handed out, and is closed; a new one is
/// made in its place.
#[test]
fn a_connection_that_no_longer_answers_is_replaced() {
    let (_db, server) = served();
    let pool = pool(&server, 2, Duration::from_secs(30));
    drop(pool.get().unwrap());
    let ended: Vec<u64> = server.operator.connections().iter().map(|(id, _)| *id).collect();
    assert_eq!(ended.len(), 1);
    assert!(server.operator.end(ended[0]));
    let mut c = pool.get().unwrap();
    assert_eq!(done(&c.query("ROLLBACK").unwrap()), 0);
    assert_eq!(pool.counts().unwrap(), (1, 0), "the ended one closed, one made");
}

/// Verifies: [DESIGN] RULES.md D206: a connection that has lived its time
/// is closed as it comes back; one idle past its limit is closed rather
/// than handed out; a pool made of a closure connects by it.
#[test]
fn connections_live_and_idle_within_their_limits() {
    let (_db, server) = served();
    let address = server.address;
    let connect = move || connect_to(&Target::Tcp(address), "X", &mut Trust);
    let lived = Pool::new(connect, PoolConfig { max_lifetime: Some(Duration::ZERO), ..PoolConfig::default() });
    drop(lived.get().unwrap());
    assert_eq!(lived.counts().unwrap(), (0, 0), "closed as it came back");
    let resting = Pool::new(connect, PoolConfig { idle_timeout: Some(Duration::ZERO), check: false, ..PoolConfig::default() });
    drop(resting.get().unwrap());
    assert_eq!(resting.counts().unwrap(), (1, 1));
    let before: Vec<u64> = server.operator.connections().iter().map(|(id, _)| *id).collect();
    drop(resting.get().unwrap());
    assert_eq!(resting.counts().unwrap(), (1, 1), "the idle one closed, another made and given back");
    let after = server.operator.connections();
    assert!(after.iter().any(|(id, _)| !before.contains(id)), "{before:?} {after:?}");
}
