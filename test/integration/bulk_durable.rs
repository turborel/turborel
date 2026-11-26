//! \[DESIGN\] A load's committed rows outlive the database's end (RULES.md
//! D25, D36, D533, D1143, D1146): of a table of the row store, and of the
//! column store loading through its tree or straight to groups, on pages
//! or, of a table of no index, as extents of segments, by one session and
//! by several at once, each load's last batch short of a group. A
//! clean close keeps every committed row, whatever LOG.DURABILITY is; so
//! does a crash after a checkpoint; a crash with no checkpoint keeps
//! every commit synced, under DATA and FULL, and under NONE either all of
//! a load's rows or none.

use std::path::Path;
use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Session};
use turborel::disk::io::FileSystem;
use turborel::params::Load;
use turborel::threads::JobPolicy;
use turborel::value::{Decimal, Value};

use crate::common::{Scratch, count};
use crate::faults::{Faults, FaultyFs};
use crate::log_sync::last_segment;

/// The rows each session loads: two batches of a group each, then one
/// short of a group.
const ROWS: i64 = 9_192;

/// The tables loaded: of the row store, and of the column store in groups
/// of 4096 rows through its tree or straight to groups, the last of no
/// index, so its groups go to segments.
const TABLES: [&str; 4] = [
    "CREATE TABLE T (K INTEGER PRIMARY KEY, A INTEGER NOT NULL)",
    "CREATE TABLE T (K INTEGER PRIMARY KEY, A INTEGER NOT NULL) USING columnstore \
     WITH (COLUMNSTORE.GROUP_ROWS = 4096, COLUMNSTORE.LOAD = 'TREE')",
    "CREATE TABLE T (K INTEGER PRIMARY KEY, A INTEGER NOT NULL) USING columnstore \
     WITH (COLUMNSTORE.GROUP_ROWS = 4096, COLUMNSTORE.LOAD = 'GROUPS')",
    "CREATE TABLE T (K INTEGER NOT NULL, A INTEGER NOT NULL) USING columnstore \
     WITH (COLUMNSTORE.GROUP_ROWS = 4096, COLUMNSTORE.LOAD = 'GROUPS')",
];

/// How a database ends before it is opened again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// Closed cleanly.
    Closed,
    /// A checkpoint taken, then closed cleanly.
    CheckpointClosed,
    /// A checkpoint taken, then a failure of the system.
    CheckpointCrashed,
    /// A failure of the system, the log cut to its last sync.
    Crashed,
    /// As `Crashed`, each segment's unsynced extents torn: half of what
    /// was written past its last sync kept.
    CrashedTorn,
}

/// The database in `dir` of `fs`, opened, its background work done only
/// when asked.
fn opened(fs: &Arc<dyn FileSystem>, dir: &Path) -> Database {
    let db = Database::new();
    db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db
}

/// T's rows `from..to`, as a copy gives an INSERT's parameters.
fn given(from: i64, to: i64) -> Vec<Vec<Value>> {
    (from..to).map(|k| vec![Value::Exact(Decimal::new(i128::from(k), 0)), Value::Exact(Decimal::new(i128::from(k % 97), 0))]).collect()
}

/// Loads `ROWS` rows in each of `sessions` at once, the n-th's from n times `ROWS`, a batch
/// of 4096 each in turn, then ends and commits each load.
fn loaded(sessions: &[Session]) {
    let insert = "INSERT INTO T (K, A) VALUES (?, ?)";
    let prepared: Vec<_> = sessions.iter().map(|s| s.prepare(insert).unwrap()).collect();
    for start in (0..ROWS).step_by(4096) {
        for (n, (s, p)) in sessions.iter().zip(&prepared).enumerate() {
            let first = i64::try_from(n).unwrap() * ROWS;
            s.execute_prepared_rows(p, &given(first + start, first + (start + 4096).min(ROWS)), Load::Going).unwrap();
        }
    }
    for (s, p) in sessions.iter().zip(&prepared) {
        s.execute_prepared_rows(p, &[], Load::Ending).unwrap();
        s.commit().unwrap();
    }
}

/// How many of each of `streams` loads' rows T holds, opened again.
fn kept(db: &Database, streams: i64) -> Vec<usize> {
    let s = db.session("S");
    (0..streams).map(|n| count(&s, &format!("SELECT K FROM T WHERE K >= {} AND K < {}", n * ROWS, (n + 1) * ROWS))).collect()
}

/// Loads T, of `table`, by `streams` sessions under `durability`, ends the
/// database as `end` says and opens it again; how many of each load's
/// rows it then holds.
fn after(table: &str, streams: i64, durability: &str, end: End) -> Vec<usize> {
    let scratch = Scratch::new("bulk-durable");
    let dir = scratch.path("db");
    let (fs, faults): (Arc<dyn FileSystem>, Arc<Faults>) = FaultyFs::with_faults();
    let db = opened(&fs, &dir);
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION S {table}")).unwrap();
    db.set_setting("LOG.DURABILITY", &Value::Char(durability.into())).unwrap();
    let sessions: Vec<Session> = (0..streams).map(|_| db.session("S")).collect();
    loaded(&sessions);
    assert_eq!(kept(&db, streams), vec![9_192; usize::try_from(streams).unwrap()], "seen before the end");
    drop(sessions);
    if matches!(end, End::CheckpointClosed | End::CheckpointCrashed) {
        db.checkpoint().unwrap();
    }
    if matches!(end, End::Closed | End::CheckpointClosed) {
        db.close().unwrap();
    }
    drop(db);
    if matches!(end, End::Crashed | End::CheckpointCrashed | End::CrashedTorn) {
        faults.cut_to_synced(&last_segment(&dir));
        crashed_segments(&faults, &dir, end == End::CrashedTorn);
    }
    kept(&opened(&fs, &dir), streams)
}

/// Cuts each segment of the database in `dir` to its last sync, as a
/// failure of the system leaves it; if `torn`, half of what was written
/// past it kept, a torn extent at its end.
fn crashed_segments(faults: &Faults, dir: &Path, torn: bool) {
    let Ok(listed) = std::fs::read_dir(dir.join("groups")) else { return };
    for path in listed.map(|e| e.unwrap().path()) {
        let written = std::fs::read(&path).unwrap();
        faults.cut_to_synced(&path);
        let synced = usize::try_from(std::fs::metadata(&path).unwrap().len()).unwrap();
        let lost = written.get(synced..).unwrap_or_default();
        if torn && !lost.is_empty() {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(&lost[..lost.len() >> 1]).unwrap();
        }
    }
}

/// Verifies: [DESIGN] RULES.md D25, D36, D533, D1143, D1146: every
/// committed row of a load, of each kind of table and by one session or
/// several, is kept by a clean close, with a checkpoint before or not, and
/// by a crash after a checkpoint, under each LOG.DURABILITY; by a crash
/// with none, its segments cut to their syncs or torn, under DATA and
/// FULL; under NONE, all of a load's rows or none.
///
/// Decision `image.clean && oldest >= log.end()` at reopen: the groups of
/// loads stored as made by a transaction open kept (D1143), else a clean
/// close loses a load's groups written since they last settled.
#[test]
fn a_loads_committed_rows_outlive_the_end_of_the_database() {
    let ends = [End::Closed, End::CheckpointClosed, End::CheckpointCrashed, End::Crashed, End::CrashedTorn];
    let cases = TABLES.iter().flat_map(|&table| {
        [1_i64, 3].into_iter().flat_map(move |streams| {
            ["NONE", "DATA", "FULL"]
                .into_iter()
                .flat_map(move |durability| ends.into_iter().map(move |end| (table, streams, durability, end)))
        })
    });
    for (table, streams, durability, end) in cases {
        let found = after(table, streams, durability, end);
        let case = format!("{table} by {streams} under {durability}, {end:?}");
        if durability == "NONE" && matches!(end, End::Crashed | End::CrashedTorn) {
            assert!(found.iter().all(|&n| n == 0 || n == 9_192), "{case}: {found:?}, a load kept in part");
        } else {
            assert_eq!(found, vec![9_192_usize; usize::try_from(streams).unwrap()], "{case}");
        }
    }
}
