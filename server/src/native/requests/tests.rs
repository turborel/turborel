//! The tests of what a session's worker does with a request.

use turborel::db::Database;
use turborel_protocol::Completed;

use super::*;

/// A session of user X, of a database whose table X.T is empty.
fn held() -> Held {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T (K INTEGER NOT NULL)").unwrap();
    let serves = Serves { db: db.clone(), requests: Arc::default() };
    Held::new(db.session("X"), serves, Capabilities::NONE)
}

/// Runs `work` in `held`, with batches too large for anything to be
/// sent ahead: the replies gathered.
fn run(held: &mut Held, work: Work) -> Vec<Reply> {
    let mut sink = |_: Vec<Reply>| Ok(());
    let mut out = Outgoing::new(&mut sink, 1 << 20);
    held.handle(work, &mut out, 8).unwrap();
    out.gathered
}

/// Whether `replies` is one completion, of a transaction `active` or
/// not.
fn completed(replies: &[Reply], active: bool) -> bool {
    matches!(replies, [Reply::Complete(Completed { sqlcode: 0, active: a, .. })] if *a == active)
}

/// The work of the Query `sql`, the transaction ended with it if `ends`.
fn query(sql: &str, ends: bool) -> Work {
    Work::Query { sql: sql.to_owned(), ends, attributes: None, multi: false }
}

/// The work that executes statement 1, at most `rows` rows sent, the
/// transaction ended with it if `ends`.
fn execute(rows: u32, ends: bool) -> Work {
    Work::Execute { at: (1, 0), values: Vec::new(), rows, ends, attributes: None }
}

/// The work that inserts `k`, the transaction ended with it if `ends`.
fn insert(k: i32, ends: bool) -> Work {
    query(&format!("INSERT INTO T VALUES ({k})"), ends)
}

/// Verifies: [EXTENSION] [DESIGN] RULES.md D45: a request that ends the
/// transaction with its statement commits it once the statement is
/// done, or rolls it back if the statement failed, its one reply saying
/// no transaction is active; one that does not leaves it active.
#[test]
fn a_request_that_ends_its_transaction_commits_or_rolls_back() {
    let mut held = held();
    assert!(completed(&run(&mut held, insert(1_i32, true)), false), "committed");
    assert!(held.session.transaction().is_none());
    assert!(completed(&run(&mut held, insert(2_i32, false)), true), "left active");
    let failed = run(&mut held, query("INSERT INTO T VALUES ('x')", true));
    assert!(matches!(failed.as_slice(), [Reply::Error(Failure { active: false, .. })]), "{failed:?}");
    assert!(held.session.transaction().is_none(), "rolled back");
    let counted = run(&mut held, query("SELECT COUNT(*) FROM T", true));
    assert!(matches!(&counted[1], Reply::Rows(rows) if rows[0][0].to_string() == "1"), "the second insert undone: {counted:?}");
    assert!(matches!(counted.as_slice(), [Reply::Description(_), Reply::Rows(_), Reply::Complete(Completed { active: false, .. })]));
}

/// Verifies: [EXTENSION] [DESIGN] RULES.md D45: a prepared query's
/// execution sends its rows with no Description; its cursor left open,
/// the transaction its request would end stays active, through the
/// fetches after; one whose rows are all sent ends it.
#[test]
fn a_cursor_left_open_keeps_the_transaction_its_request_would_end() {
    let mut held = held();
    run(&mut held, Work::Prepare(1, "SELECT K FROM T".into()));
    run(&mut held, insert(3_i32, false));
    run(&mut held, insert(4_i32, false));
    let opened = run(&mut held, execute(1, true));
    assert!(matches!(opened.as_slice(), [Reply::Rows(_), Reply::Suspended]), "no Description: {opened:?}");
    assert!(held.session.transaction().is_some(), "its cursor open");
    let rest = run(&mut held, Work::Fetch((1, 0), 0));
    assert!(matches!(rest.as_slice(), [Reply::Rows(_), Reply::Complete(Completed { active: true, .. })]), "{rest:?}");
    assert!(completed(&run(&mut held, execute(0, true))[1..], false));
}

/// Verifies: [DESIGN] RULES.md D42: a statement that fails with its
/// Description and rows gathered, none sent ahead, sends neither, while a
/// reply gathered before it, of a request pipelined ahead, and its own
/// notices still go; one that failed after a batch was sent ahead keeps
/// the rows gathered since, as its Description went with that batch.
/// Regression test (R-TEST-7): a statement cancelled at its first fetch
/// sent its Description before its Error, so a client printed the
/// columns of a result it never had.
#[test]
fn a_statement_failed_before_its_rows_sends_no_description() {
    let mut sink = |_: Vec<Reply>| Ok(());
    let mut out = Outgoing::new(&mut sink, 1 << 20);
    out.gathered.push(Reply::Pong);
    let mark = out.mark();
    out.gathered.push(Reply::Description(Vec::new()));
    out.rows(&mut vec![Vec::new()]);
    out.gathered.push(Reply::Suspended);
    out.unanswered(mark);
    assert!(matches!(out.gathered.as_slice(), [Reply::Pong, Reply::Suspended]), "{:?}", out.gathered);
    let sent_ahead = out.mark();
    out.gathered.push(Reply::Description(Vec::new()));
    assert!(out.send().is_ok());
    out.rows(&mut vec![Vec::new()]);
    out.unanswered(sent_ahead);
    assert!(matches!(out.gathered.as_slice(), [Reply::Rows(_)]), "its Description sent ahead, its rows go after");
}
