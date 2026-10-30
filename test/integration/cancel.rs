//! Cancelling a running statement (RULES.md D44), from another thread: as
//! it reads, as its cursor is fetched from, and as it waits for a lock;
//! and by STATEMENT_TIMEOUT and LOCK_TIMEOUT.

use turborel::db::Session;
use turborel::error::Code;

use crate::common::{arm, gated, locked, rows};

/// Verifies: [X3.135-1986 §7.3 GR3b i]; [DESIGN] RULES.md D44: a statement
/// cancelled from another thread as it reads fails (SQLCODE -920,
/// SQLSTATE TB920) and is undone, its changes gone; its transaction goes
/// on, and the session's next statement runs. A cancel with nothing
/// running stops nothing.
#[test]
fn a_statement_cancelled_as_it_reads_is_undone() {
    let (db, gate, heard, go) = gated();
    let s = db.session("X");
    s.canceller().cancel();
    s.run("INSERT INTO G VALUES (4)").unwrap();
    arm(&gate);
    let canceller = s.canceller();
    let cancelling = std::thread::spawn(move || {
        heard.recv().unwrap();
        canceller.cancel();
        go.send(()).unwrap();
    });
    let failed = s.run("UPDATE G SET K = K + 10").unwrap_err();
    cancelling.join().unwrap();
    assert_eq!((failed.code, failed.sqlstate().as_str()), (Code::Cancelled, "TB920"));
    assert_eq!(rows(&s, "SELECT K FROM G ORDER BY K"), Ok(vec!["1".into(), "2".into(), "3".into(), "4".into()]), "undone, the insert kept");
}

/// Verifies: [DESIGN] RULES.md D44: a statement whose scan is told its
/// conditions looks for a cancel as it reads each row, as an unfiltered
/// scan does: a query read row by row, cancelled as it reads, fails
/// (SQLCODE -920); and a searched DELETE, cancelled as it reads its object
/// rows, fails before it finds that none match.
#[test]
fn a_filtered_scan_cancelled_as_it_reads_fails() {
    assert_eq!(cancelled_as_it_reads(|s| rows(s, "SELECT K FROM G WHERE K > 0").err()), Some(Code::Cancelled), "a query");
    let deleted = cancelled_as_it_reads(|s| s.run("DELETE FROM G WHERE K > 100").err().map(|e| e.code));
    assert_eq!(deleted, Some(Code::Cancelled), "a DELETE");
}

/// The error of `run`, on a session of `gated()`'s database reading row by
/// row, cancelled once its scan reaches the second row; the table's rows
/// checked to be left as they were.
fn cancelled_as_it_reads(run: impl Fn(&Session) -> Option<Code>) -> Option<Code> {
    let (db, gate, heard, go) = gated();
    let s = db.session("X");
    s.run("SET EXECUTOR.BATCHES = 'OFF'").unwrap();
    arm(&gate);
    let canceller = s.canceller();
    let cancelling = std::thread::spawn(move || {
        heard.recv().unwrap();
        canceller.cancel();
        go.send(()).unwrap();
    });
    let failed = run(&s);
    cancelling.join().unwrap();
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM G WHERE K > 0"), Ok(vec!["3".into()]));
    failed
}

/// Verifies: [DESIGN] RULES.md D44: a fetch of a cursor reading as it is
/// fetched from, cancelled as it reads, fails (SQLCODE -920); its rows were
/// cut short there, so every fetch after fails so too, none seeming the
/// last; a statement after runs.
#[test]
fn a_fetch_cancelled_as_it_reads_fails() {
    let (db, gate, heard, go) = gated();
    let s = db.session("X");
    arm(&gate);
    let mut cursor = s.open_cursor("SELECT K FROM G").unwrap();
    assert!(cursor.fetch().unwrap().is_some());
    let canceller = s.canceller();
    let cancelling = std::thread::spawn(move || {
        heard.recv().unwrap();
        canceller.cancel();
        go.send(()).unwrap();
    });
    assert_eq!(cursor.fetch().err().map(|e| e.code), Some(Code::Cancelled));
    cancelling.join().unwrap();
    assert_eq!(cursor.fetch().err().map(|e| e.code), Some(Code::Cancelled), "cut short");
    drop(cursor);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM G"), Ok(vec!["3".into()]));
}

/// Verifies: [DESIGN] RULES.md D28, D44: a statement waiting for a lock
/// another transaction holds, cancelled, stops waiting and fails (SQLCODE
/// -920); the lock's holder is unaffected.
#[test]
fn a_statement_cancelled_as_it_waits_for_a_lock_stops_waiting() {
    let (db, holder) = locked();
    let waiter = db.session("X");
    let canceller = waiter.canceller();
    let waiting = std::thread::spawn(move || waiter.run("INSERT INTO L VALUES (2)").err().map(|e| e.code));
    // Cancelled once it runs: a cancel before is of nothing.
    while !waiting.is_finished() {
        canceller.cancel();
        std::thread::yield_now();
    }
    assert_eq!(waiting.join().unwrap(), Some(Code::Cancelled));
    holder.commit().unwrap();
}

/// Verifies: [DESIGN] RULES.md D44: a statement that runs longer than its
/// session's STATEMENT_TIMEOUT, here waiting for a lock, is cancelled
/// (SQLCODE -920) once the time is past, saying so; the session's next
/// statement runs, and 0 sets no limit.
#[test]
fn a_statement_past_its_timeout_is_cancelled() {
    let (db, holder) = locked();
    let waiter = db.session("X");
    waiter.run("SET STATEMENT_TIMEOUT = 30").unwrap();
    let began = std::time::Instant::now();
    let failed = waiter.run("INSERT INTO L VALUES (2)").unwrap_err();
    assert!(began.elapsed() >= std::time::Duration::from_millis(30), "{:?}", began.elapsed());
    assert_eq!(failed.code, Code::Cancelled);
    assert!(failed.msg.contains("STATEMENT_TIMEOUT"), "{}", failed.msg);
    holder.commit().unwrap();
    waiter.run("SET STATEMENT_TIMEOUT = 0").unwrap();
    waiter.run("INSERT INTO L VALUES (2)").unwrap();
}

/// Verifies: [DESIGN] RULES.md D44: a statement reading past its
/// STATEMENT_TIMEOUT is cancelled as it reads on (SQLCODE -920), and
/// undone.
#[test]
fn a_statement_reading_past_its_timeout_is_cancelled() {
    let (db, gate, heard, go) = gated();
    let s = db.session("X");
    for k in 4..300 {
        s.run(&format!("INSERT INTO G VALUES ({k})")).unwrap();
    }
    s.commit().unwrap();
    s.run("SET STATEMENT_TIMEOUT = 20").unwrap();
    arm(&gate);
    let stalling = std::thread::spawn(move || {
        heard.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(40));
        go.send(()).unwrap();
    });
    let failed = s.run("UPDATE G SET K = K + 1000").unwrap_err();
    stalling.join().unwrap();
    assert_eq!(failed.code, Code::Cancelled);
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM G WHERE K > 1000"), Ok(vec!["0".into()]), "undone");
}

/// Verifies: [DESIGN] RULES.md D28: a statement that waits for a lock
/// longer than LOCK_TIMEOUT is refused (SQLCODE -911), saying so; its
/// transaction goes on, and once the holder commits the statement runs.
#[test]
fn a_wait_for_a_lock_past_its_timeout_is_refused() {
    let (db, holder) = locked();
    let waiter = db.session("X");
    waiter.run("SET LOCK_TIMEOUT = 30").unwrap();
    let began = std::time::Instant::now();
    let failed = waiter.run("INSERT INTO L VALUES (2)").unwrap_err();
    assert!(began.elapsed() >= std::time::Duration::from_millis(30), "{:?}", began.elapsed());
    assert_eq!(failed.code, Code::Conflict);
    assert!(failed.msg.contains("LOCK_TIMEOUT"), "{}", failed.msg);
    holder.commit().unwrap();
    waiter.run("INSERT INTO L VALUES (2)").unwrap();
}
