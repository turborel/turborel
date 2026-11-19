//! Pipelines (RULES.md D45, D200): requests written at once, answered in
//! order, a failure its request's own; or, skipping after an error, the
//! requests after it undone and those before kept; batches longer than a
//! window written a window at a time.

use turborel::db::Database;
use turborel::value::{Decimal, Value};
use turborel_client::pipeline::{OnError, PIPELINE_WINDOW, Piped};

use crate::common::{Running, done, rows, start};

/// A server whose user X has the table T of one key.
fn served() -> (Database, Running) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER PRIMARY KEY, V VARCHAR(100))").unwrap();
    let server = start(&db, &[]);
    (db, server)
}

/// The keys of T, as a new connection of `server` reads them.
fn keys(server: &Running) -> Vec<String> {
    rows(&server.connect("X").query("SELECT K FROM T ORDER BY K").unwrap())
}

/// Verifies: [DESIGN] RULES.md D45, D200: a pipeline's requests, queries
/// and executions of a prepared statement, are answered one outcome each,
/// in the order queued; a failure, a key met twice (class 23), is its
/// request's alone, and the requests after it run; the connection goes on
/// after, its transaction active as the last request left it.
#[test]
fn a_pipeline_answers_each_request_in_order() {
    let (_db, server) = served();
    let mut c = server.connect("X");
    let insert = c.prepare("INSERT INTO T VALUES (?, 'v')").unwrap();
    let mut pipeline = c.pipeline();
    pipeline
        .query("INSERT INTO T VALUES (1, 'a')")
        .query("INSERT INTO T VALUES (1, 'again')")
        .execute(&insert, &[Value::Exact(Decimal::new(2, 0))])
        .query("SELECT K, V FROM T ORDER BY K");
    assert_eq!(pipeline.len(), 4);
    let outcomes = pipeline.run().unwrap();
    assert!(matches!(&outcomes[0], Piped::Done(o) if done(o) == 0), "{outcomes:?}");
    assert!(matches!(&outcomes[1], Piped::Failed(f) if f.sqlstate.starts_with("23")), "{outcomes:?}");
    assert!(matches!(&outcomes[2], Piped::Done(o) if done(o) == 0), "{outcomes:?}");
    let Piped::Done(read) = &outcomes[3] else { panic!("{outcomes:?}") };
    assert_eq!(rows(read), ["1,a", "2,v"]);
    assert!(c.in_transaction());
    assert_eq!(done(&c.query("COMMIT").unwrap()), 0);
    assert_eq!(keys(&server), ["1", "2"]);
    assert!(c.pipeline().run().unwrap().is_empty(), "an empty pipeline sends nothing");
}

/// Verifies: [DESIGN] RULES.md D200: under autocommit each request of a
/// pipeline is its own transaction, so a failed one undoes none other;
/// under `OnError::Skip` the requests after the first failure are undone
/// and `Skipped`, those before it kept, and under autocommit the batch's
/// work committed as it ends.
#[test]
fn a_pipeline_skips_after_an_error_when_asked() {
    let (_db, server) = served();
    let mut c = server.connect("X");
    c.set_autocommit(true);
    let mut each = c.pipeline();
    each.query("INSERT INTO T VALUES (1, 'a')").query("INSERT INTO T VALUES (1, 'b')").query("INSERT INTO T VALUES (2, 'c')");
    let outcomes = each.run().unwrap();
    assert!(matches!(outcomes[1], Piped::Failed(_)) && matches!(outcomes[2], Piped::Done(_)), "{outcomes:?}");
    assert!(!c.in_transaction());
    assert_eq!(keys(&server), ["1", "2"]);
    let mut skipping = c.pipeline();
    skipping
        .on_error(OnError::Skip)
        .query("INSERT INTO T VALUES (3, 'kept')")
        .query("INSERT INTO T VALUES (1, 'twice')")
        .query("INSERT INTO T VALUES (4, 'undone')")
        .query("INSERT INTO T VALUES (5, 'undone')");
    let outcomes = skipping.run().unwrap();
    assert_eq!(outcomes.len(), 4);
    assert!(matches!(outcomes[0], Piped::Done(_)) && matches!(outcomes[1], Piped::Failed(_)), "{outcomes:?}");
    assert!(matches!(outcomes[2], Piped::Skipped) && matches!(outcomes[3], Piped::Skipped), "{outcomes:?}");
    assert!(!c.in_transaction(), "committed as it ended");
    assert_eq!(keys(&server), ["1", "2", "3"]);
}

/// Verifies: [DESIGN] RULES.md D200: within a transaction a skipping
/// pipeline undoes only what came after its failure, the transaction's
/// earlier work kept and still active; its first request failing skips
/// all the rest.
#[test]
fn a_skipping_pipeline_keeps_the_transaction_before_it() {
    let (_db, server) = served();
    let mut c = server.connect("X");
    c.query("INSERT INTO T VALUES (1, 'before')").unwrap();
    let mut skipping = c.pipeline();
    skipping.on_error(OnError::Skip).query("INSERT INTO T VALUES (1, 'twice')").query("INSERT INTO T VALUES (2, 'undone')");
    let outcomes = skipping.run().unwrap();
    assert!(matches!(outcomes[0], Piped::Failed(_)) && matches!(outcomes[1], Piped::Skipped), "{outcomes:?}");
    assert!(c.in_transaction());
    assert_eq!(rows(&c.query("SELECT K FROM T").unwrap()), ["1"]);
    c.query("COMMIT").unwrap();
    assert_eq!(keys(&server), ["1"]);
}

/// Verifies: [DESIGN] RULES.md D200: a batch of many more bytes than a
/// window is written a window at a time, each reply read in order, and
/// every request answered: none waits on a server waiting on it.
#[test]
fn a_batch_longer_than_a_window_is_answered_whole() {
    let (_db, server) = served();
    let mut c = server.connect("X");
    let value = "w".repeat(90);
    let count = PIPELINE_WINDOW.checked_div(50).unwrap();
    let mut pipeline = c.pipeline();
    for k in 0..count {
        pipeline.query(&format!("INSERT INTO T VALUES ({k}, '{value}')"));
        pipeline.query("SELECT K, V FROM T WHERE K = 0");
    }
    let outcomes = pipeline.run().unwrap();
    assert_eq!(outcomes.len(), count * 2);
    assert!(outcomes.iter().all(|o| matches!(o, Piped::Done(_))), "every request answered");
    c.query("COMMIT").unwrap();
    assert_eq!(keys(&server).len(), count);
}
