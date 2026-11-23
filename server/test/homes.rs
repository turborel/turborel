//! A connection's home: the one thread that runs its statements while they
//! keep coming (RULES.md D767, D768), seen through a function that names
//! the thread calling it.

use std::collections::BTreeSet;
use std::net::TcpStream;
use std::time::Duration;

use turborel::db::Database;
use turborel::error::Result;
use turborel::functions::ScalarFunction;
use turborel::value::{DataType, Decimal, Spelled, Value};
use turborel_client::connection::Connection;
use turborel_server::settings::WORKERS;

use crate::common::{Running, done, rows, start, state};

/// \[DESIGN\] Longer than a worker waits on its client for the next
/// request (`native/lent.rs`'s `WAITED`), so each request after it is read
/// by the connection's task and handed to a thread again.
const PAUSE: Duration = Duration::from_millis(5);

/// `THREAD_NUMBER()`: the number of the thread that calls it. A test-local
/// plug-in (RULES.md D6), not deterministic, so each call is made.
struct ThreadNumber;

impl ScalarFunction for ThreadNumber {
    fn name(&self) -> &'static str {
        "THREAD_NUMBER"
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Exact { precision: 18, scale: 0, spelled: Spelled::Numeric })
    }
    fn call(&self, _: &[Value]) -> Result<Value> {
        let named = format!("{:?}", std::thread::current().id());
        let digits: String = named.chars().filter(char::is_ascii_digit).collect();
        Ok(Value::Exact(Decimal::new(digits.parse().unwrap(), 0)))
    }
    fn deterministic(&self) -> bool {
        false
    }
}

/// A database whose table X.T holds `n` rows, keys 1 to `n` and values 0,
/// that calls THREAD_NUMBER.
fn database(n: i128) -> Database {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL, V INTEGER)").unwrap();
    db.register_scalar_function(Box::new(ThreadNumber)).unwrap();
    db.set_setting("SQL.FUNCTION_CALLS", &Value::Char("ON".into())).unwrap();
    let s = db.session("X");
    for k in 1..=n {
        s.run(&format!("INSERT INTO T VALUES ({k}, 0)")).unwrap();
    }
    s.commit().unwrap();
    db
}

/// The thread that runs `c`'s next statement.
fn thread_of(c: &mut Connection<TcpStream>) -> String {
    rows(&c.query("VALUES THREAD_NUMBER()").unwrap()).remove(0)
}

/// The threads `c` runs `n` statements on, each after a pause.
fn threads_of(c: &mut Connection<TcpStream>, n: usize) -> BTreeSet<String> {
    (0..n)
        .map(|_| {
            std::thread::sleep(PAUSE);
            thread_of(c)
        })
        .collect()
}

/// Verifies: [DESIGN] RULES.md D767: every statement of a connection runs
/// on one thread, its home, though each comes after the worker has given
/// up waiting for it and other connections' statements run between; two
/// connections have two homes. Fails where each request is handed to any
/// free thread of the pool.
#[test]
fn a_connections_statements_run_on_one_thread() {
    let db = database(0);
    let server = start(&db, &[(WORKERS, 2)]);
    let (mut a, mut b) = (server.connect("X"), server.connect("X"));
    let (mut on_a, mut on_b) = (BTreeSet::new(), BTreeSet::new());
    for _ in 0..12 {
        on_a.extend(threads_of(&mut a, 1));
        on_b.extend(threads_of(&mut b, 1));
    }
    assert_eq!((on_a.len(), on_b.len()), (1, 1), "{on_a:?} {on_b:?}");
    assert_ne!(on_a, on_b, "a home each");
    a.terminate().unwrap();
    b.terminate().unwrap();
}

/// Verifies: [DESIGN] RULES.md D44, D767: a statement running on a
/// connection's home is cancelled over another connection, and the session
/// goes on, on the same thread.
#[test]
fn a_statement_at_home_is_cancelled() {
    let db = database(300);
    let server = start(&db, &[(WORKERS, 2)]);
    let mut c = server.connect("X");
    let home = threads_of(&mut c, 3);
    let canceller = c.canceller().unwrap();
    let running = std::thread::spawn(move || {
        let outcome = c.query("SELECT COUNT(*) FROM T A, T B, T C");
        (c, outcome.err())
    });
    while !running.is_finished() {
        canceller.cancel().unwrap();
        std::thread::yield_now();
    }
    let (mut c, failure) = running.join().unwrap();
    assert_eq!(state(&failure.unwrap()), ("TB920", -920));
    assert_eq!(threads_of(&mut c, 3), home, "the same home");
    assert_eq!(rows(&c.query("SELECT COUNT(*) FROM T").unwrap()), ["300"], "the session goes on");
}

/// Runs `client`'s statements on `server`: `n` inserts of keys from
/// `client * 100`, each after a pause, then a commit; the threads they ran
/// on.
fn inserted(server: &Running, client: usize, n: usize) -> BTreeSet<String> {
    let mut c = server.connect("X");
    let mut threads = BTreeSet::new();
    for k in 0..n {
        std::thread::sleep(PAUSE);
        c.query(&format!("INSERT INTO T VALUES ({}, THREAD_NUMBER())", client * 100 + k)).unwrap();
        threads.extend(threads_of(&mut c, 1));
    }
    assert_eq!(done(&c.query("COMMIT").unwrap()), 0);
    c.terminate().unwrap();
    threads
}

/// Verifies: [DESIGN] RULES.md D42, D767: more connections than
/// SERVER.WORKERS each wait their turn for the one worker allowed and all
/// finish, each on a home of its own, what each commits all there.
#[test]
fn connections_past_the_workers_wait_their_turn_at_home() {
    let db = database(0);
    let server = start(&db, &[(WORKERS, 1)]);
    let homes: Vec<BTreeSet<String>> = std::thread::scope(|scope| {
        let clients: Vec<_> = (0..6)
            .map(|client| {
                scope.spawn({
                    let server = &server;
                    move || inserted(server, client, 10)
                })
            })
            .collect();
        clients.into_iter().map(|c| c.join().unwrap()).collect()
    });
    assert!(homes.iter().all(|h| h.len() == 1), "{homes:?}");
    let s = db.session("X");
    let count = s.open_cursor("SELECT COUNT(*) FROM T").unwrap().fetch().unwrap().unwrap()[0].to_string();
    assert_eq!(count, "60");
}

/// Verifies: [DESIGN] RULES.md D767: a session waiting on a row another
/// session's transaction holds does not keep that session from running
/// the commit that frees it, each connection on its own home, the workers
/// as few as the two sessions; the waiter then goes on.
#[test]
fn a_session_waiting_on_a_row_does_not_keep_its_holder_from_committing() {
    let db = database(1);
    let server = start(&db, &[(WORKERS, 2)]);
    let (mut holder, mut waiter) = (server.connect("X"), server.connect("X"));
    assert_eq!(done(&holder.query("UPDATE T SET V = 1 WHERE K = 1").unwrap()), 0);
    let waiting = std::thread::spawn(move || {
        let updated = waiter.query("UPDATE T SET V = 2 WHERE K = 1").map(|o| done(&o));
        (waiter, updated)
    });
    while db.waiting_statements().unwrap() == 0 {
        std::thread::yield_now();
    }
    std::thread::sleep(PAUSE);
    assert_eq!(done(&holder.query("COMMIT").unwrap()), 0, "committed while the other waits");
    let (mut waiter, updated) = waiting.join().unwrap();
    assert_eq!(updated.unwrap(), 0);
    assert_eq!(done(&waiter.query("COMMIT").unwrap()), 0);
    assert_eq!(rows(&holder.query("SELECT V FROM T").unwrap()), ["2"]);
}
