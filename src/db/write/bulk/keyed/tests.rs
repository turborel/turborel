//! Tests of loads by column into tables of keys and indexes (RULES.md
//! D1235): their groups' entries added at once, their duplicates found
//! by the statement's end as a row's are, two loads of one key never both
//! standing, a reader seeing each load whole or not at all, a method of
//! no group path taking each entry in turn, a load refused part way and
//! one failing past each index leaving nothing behind, and a crash at
//! each point leaving every index as its table.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::FAIL_AFTER;
use crate::db::write::lanes::BY_COLUMN;
use crate::db::{Database, OpenOptions, Session};
use crate::disk::io::{FileSystem, OpenMode, walk};
use crate::disk::memory::MemoryFileSystem;
use crate::error::Result;
use crate::params::Load;
use crate::storage::lanes::{Lane, Lanes, set_null};
use crate::threads::JobPolicy;

const STORE: &str = "USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 4096)";
const DIR: &str = "/keyed";

/// The lanes of `columns`, each a column's cells, a null none.
fn lanes(columns: &[Vec<Option<String>>]) -> Lanes {
    let rows = columns.first().map_or(0, Vec::len);
    let lane = |cells: &Vec<Option<String>>| {
        let mut lane = Lane { nulls: vec![0; rows.div_ceil(8)], ..Lane::default() };
        for (n, cell) in cells.iter().enumerate() {
            match cell {
                Some(text) => lane.bytes.extend_from_slice(text.as_bytes()),
                None => set_null(&mut lane.nulls, n),
            }
            lane.ends.push(u32::try_from(lane.bytes.len()).unwrap());
        }
        lane
    };
    Lanes { rows, columns: columns.iter().map(lane).collect() }
}

/// The cells of `f` of each of `keys`.
fn cells(keys: impl Iterator<Item = i64>, f: impl Fn(i64) -> String) -> Vec<Option<String>> {
    keys.map(|k| Some(f(k))).collect()
}

/// The lanes of K and A of `keys`, A of each its key modulo 97.
fn keyed(keys: std::ops::Range<i64>) -> Vec<Vec<Option<String>>> {
    vec![cells(keys.clone(), |k| k.to_string()), cells(keys, |k| (k % 97).to_string())]
}

/// Executes `insert` by session `s` given `columns` by column, as part
/// `load` of a load: whether it went in by column.
fn load(s: &Session, insert: &str, columns: &[Vec<Option<String>>], load: Load) -> Result<bool> {
    let p = s.prepare(insert)?;
    let before = BY_COLUMN.with(std::cell::Cell::get);
    s.execute_prepared_lanes(&p, Arc::new(lanes(columns)), load)?;
    Ok(BY_COLUMN.with(std::cell::Cell::get) > before)
}

const INTO_T: &str = "INSERT INTO T (K, A) VALUES (?, ?)";

/// The first value of the first row of `sql`, as text.
fn one(s: &Session, sql: &str) -> String {
    let row = s.open_cursor(sql).unwrap().fetch().unwrap().unwrap();
    row.first().unwrap().to_string()
}

/// The entries of each index of T, by name.
fn entries(db: &Database) -> Vec<(String, usize)> {
    let st = db.snapshot().unwrap();
    let mut names: Vec<String> =
        st.indexes.keys().filter(|n| st.catalog.index(n).is_some_and(|ix| ix.table.ident == "T")).cloned().collect();
    names.sort();
    names.into_iter().map(|n| (n.clone(), db.index_entries(&n).unwrap())).collect()
}

/// Whether every index of T holds `rows` entries, as T holds `rows` rows.
fn whole(db: &Database, rows: usize) -> bool {
    let s = db.session("S");
    let held = one(&s, "SELECT COUNT(*) FROM T") == rows.to_string();
    s.commit().unwrap();
    held && entries(db).iter().all(|(_, n)| *n == rows)
}

/// A database in memory of T, keyed by K and indexed by A too.
fn in_memory() -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER PRIMARY KEY, A INTEGER) {STORE}")).unwrap();
    let s = db.session("S");
    s.run("CREATE INDEX T_A ON T (A)").unwrap();
    s.commit().unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D1235: a load by column into a table of a
/// primary key and an index goes in by column, each index holding an entry
/// of each row; one of a key twice, or of a key there, goes in by column
/// and fails at the statement's end, as a row's duplicate does, leaving
/// its indexes as they were.
#[test]
fn a_keyed_load_goes_by_column_and_its_duplicates_fail() {
    let db = in_memory();
    let s = db.session("S");
    assert!(load(&s, INTO_T, &keyed(0..4000), Load::Alone).unwrap());
    s.commit().unwrap();
    assert!(whole(&db, 4000));
    let mut twice = keyed(4000..8000);
    for column in &mut twice {
        column[3999] = column[0].clone();
    }
    for doubled in [twice, keyed(3999..7999)] {
        let e = load(&s, INTO_T, &doubled, Load::Alone).unwrap_err();
        assert_eq!(e.sqlstate().to_string().get(..2), Some("23"), "{e}");
        s.rollback().unwrap();
        assert!(whole(&db, 4000));
    }
    assert_eq!(one(&s, "SELECT COUNT(*) FROM T WHERE A = 5"), (0_i64..4000).filter(|k| k % 97 == 5).count().to_string());
}

/// The outcome of a load by `b`, waiting for locks, of keys 2048 to 6143
/// while `a` holds keys 0 to 4095 loaded, `a` committing if `commits`,
/// else rolling back, its failure if it fails: T's indexes hold keys from 10,000 on, so neither
/// load holds one back and takes T's lock alone (RULES.md D532).
fn overlapping(db: &Database, commits: bool) -> Option<crate::error::SqlError> {
    let a = db.session("S");
    assert!(load(&a, INTO_T, &keyed(10_000..10_100), Load::Alone).unwrap());
    a.commit().unwrap();
    assert!(load(&a, INTO_T, &keyed(0..4096), Load::Going).unwrap());
    let b = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let b = db.session("S");
                b.run("SET LOCKS.WAIT = 'ON'").unwrap();
                let went = load(&b, INTO_T, &keyed(2048..6144), Load::Alone);
                if went.is_ok() {
                    b.commit().unwrap();
                } else {
                    b.rollback().unwrap();
                }
                went.err()
            })
            .join()
            .unwrap()
    });
    load(&a, INTO_T, &[vec![], vec![]], Load::Ending).unwrap();
    if commits {
        a.commit().unwrap();
    } else {
        a.rollback().unwrap();
    }
    b
}

/// Verifies: [DESIGN] RULES.md D1235, as of rows inserted together: two
/// loads by column of overlapping keys of a primary key at once never
/// both stand: the second, finding the first's keys in a group of a
/// transaction still open, fails as another transaction's row of its key
/// fails it, SQLCODE -911, whether the first then commits or rolls back.
#[test]
fn two_loads_of_overlapping_keys_never_both_stand() {
    for commits in [true, false] {
        let db = in_memory();
        let e = overlapping(&db, commits).unwrap();
        assert_eq!(e.code, crate::error::Code::Conflict, "{e}");
        assert!(whole(&db, if commits { 4196 } else { 100 }));
    }
}

/// Verifies: [DESIGN] RULES.md D1235, RULES.md D29: readers counting T's
/// rows by its key while loads by column put keys between the keys there,
/// splitting leaf after leaf, each see every load whole or not at all.
#[test]
fn readers_see_each_load_whole_while_its_entries_split_leaves() {
    let db = in_memory();
    let s = db.session("S");
    let evens: Vec<Vec<Option<String>>> = vec![cells((0..4096).map(|k| k * 2), |k| k.to_string()), cells(0..4096, |_| "1".into())];
    assert!(load(&s, INTO_T, &evens, Load::Alone).unwrap());
    s.commit().unwrap();
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let reader = scope.spawn(|| {
            let r = db.session("S");
            let mut seen = vec![4096];
            while !done.load(Ordering::Acquire) {
                let n: usize = one(&r, "SELECT COUNT(*) FROM T WHERE K BETWEEN 0 AND 100000").parse().unwrap();
                r.commit().unwrap();
                assert!(n.is_multiple_of(1024) && n >= *seen.last().unwrap() && n <= 8192, "whole loads only, in order: {n}");
                seen.push(n);
            }
            seen.len()
        });
        for part in 0..4 {
            let odd = (0..1024).map(|k| (k * 4 + part) * 2 + 1);
            assert!(load(&s, INTO_T, &[cells(odd, |k| k.to_string()), cells(0..1024, |_| "2".into())], Load::Alone).unwrap());
            s.commit().unwrap();
        }
        done.store(true, Ordering::Release);
        assert!(reader.join().unwrap() > 0);
    });
    assert!(whole(&db, 8192));
}

/// Verifies: [DESIGN] RULES.md D1235: a load by column into a table of a
/// full-text index, of no group path, goes in by column, the index taking
/// each entry in turn (`Index::insert_sorted`'s default), and finds each
/// word's rows; a load that fails leaves it as it was.
#[test]
fn a_method_of_no_group_path_takes_each_entry_in_turn() {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER PRIMARY KEY, B VARCHAR(40)) {STORE}")).unwrap();
    let s = db.session("S");
    s.run("CREATE INDEX T_B ON T (B) USING fulltext").unwrap();
    s.commit().unwrap();
    let into = "INSERT INTO T (K, B) VALUES (?, ?)";
    let words = |keys: std::ops::Range<i64>| vec![cells(keys.clone(), |k| k.to_string()), cells(keys, |k| format!("w{} common", k % 7))];
    assert!(load(&s, into, &words(0..4000), Load::Alone).unwrap());
    s.commit().unwrap();
    let matching = |word: &str| one(&s, &format!("SELECT COUNT(*) FROM T WHERE TEXT_MATCHES(B, '{word}')"));
    assert_eq!(matching("w3"), (0_i64..4000).filter(|k| k % 7 == 3).count().to_string());
    assert_eq!(matching("common"), "4000");
    assert!(load(&s, into, &words(3990..4500), Load::Alone).is_err());
    s.rollback().unwrap();
    assert_eq!(matching("common"), "4000");
}

/// T's rows and bytes, as TABLE_STATISTICS says.
fn pages(s: &Session) -> String {
    let sql = "SELECT ROW_COUNT, BYTES FROM INFORMATION_SCHEMA.TABLE_STATISTICS WHERE TABLE_NAME = 'T'";
    let row = s.open_cursor(sql).unwrap().fetch().unwrap().unwrap();
    row.iter().map(ToString::to_string).collect::<Vec<_>>().join(" ")
}

/// T's indexes' entries and its rows and bytes after a load's part by
/// column, then after a part of a row whose foreign key finds no parent,
/// given by column if `by_column`, else as rows, which fails, and after
/// the load is rolled back.
fn refused(by_column: bool) -> [(Vec<(String, usize)>, String); 3] {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    let parent = "CREATE TABLE P (ID INTEGER PRIMARY KEY)";
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION S {parent} CREATE TABLE T (K INTEGER PRIMARY KEY, A INTEGER REFERENCES P) {STORE}"))
        .unwrap();
    let s = db.session("S");
    s.run("CREATE INDEX T_A ON T (A)").unwrap();
    s.run(&format!("INSERT INTO P VALUES {}", (0_i32..97_i32).map(|n| format!("({n})")).collect::<Vec<_>>().join(", "))).unwrap();
    s.commit().unwrap();
    assert!(load(&s, INTO_T, &keyed(0..4096), Load::Going).unwrap());
    let first = (entries(&db), pages(&s));
    let mut orphan = keyed(5000..9096);
    orphan[1][17] = Some("500".into());
    let e = if by_column {
        load(&s, INTO_T, &orphan, Load::Going).unwrap_err()
    } else {
        let rows: Vec<crate::storage::Row> =
            (0..4096).map(|n| orphan.iter().map(|c| crate::value::Value::Char(c[n].clone().unwrap().into())).collect()).collect();
        s.execute_prepared_rows(&s.prepare(INTO_T).unwrap(), &rows, Load::Going).unwrap_err()
    };
    assert_eq!(e.sqlstate().to_string().get(..2), Some("23"), "{e}");
    let failed = (entries(&db), pages(&s));
    s.rollback().unwrap();
    [first, failed, (entries(&db), pages(&s))]
}

/// Verifies: [DESIGN] RULES.md D1235: a load's part by column whose
/// foreign key finds no parent for one row is refused before anything of
/// it is written and goes in as rows, which fail at the statement's end:
/// T's indexes hold the entries of the part before alone, and its pages
/// are what the same rows given as rows leave, as they are once the load
/// is rolled back, every index then empty.
#[test]
fn a_load_refused_part_way_leaves_nothing_behind() {
    let [first, failed, undone] = refused(true);
    assert!(first.0.iter().all(|(_, n)| *n == 4096), "{first:?}");
    assert_eq!(failed.0, first.0, "the part before's entries alone");
    assert!(undone.0.iter().all(|(_, n)| *n == 0), "{undone:?}");
    assert_eq!([first, failed, undone], refused(false), "as rows given as rows leave it");
}

/// A copy of what `fs` holds under `DIR` now: what a crash leaves.
fn image(fs: &MemoryFileSystem) -> Arc<dyn FileSystem> {
    let copy = MemoryFileSystem::new();
    for path in walk(fs, Path::new(DIR)).unwrap() {
        copy.create_dir_all(path.parent().unwrap()).unwrap();
        let from = fs.open(&path, OpenMode::ReadOnly).unwrap();
        let mut bytes = vec![0; usize::try_from(from.size().unwrap()).unwrap()];
        from.read_exact_at(&mut bytes, 0).unwrap();
        copy.open(&path, OpenMode::CreateNew).unwrap().write_all_at(&bytes, 0).unwrap();
    }
    Arc::new(copy)
}

/// The database `fs` holds, opened.
fn opened(fs: &Arc<dyn FileSystem>) -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.open_files(fs, Path::new(DIR), &OpenOptions::default()).unwrap();
    db
}

/// A database of T in `fs`, keyed and indexed twice, its first 100 rows
/// committed and checkpointed, so it is reopened as its checkpoint left
/// it and its indexes mended.
fn checkpointed(fs: &Arc<MemoryFileSystem>) -> Database {
    let shared: Arc<dyn FileSystem> = Arc::<MemoryFileSystem>::clone(fs);
    let db = opened(&shared);
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION S CREATE TABLE T (K INTEGER PRIMARY KEY, A INTEGER) {STORE}")).unwrap();
    let s = db.session("S");
    s.run("CREATE INDEX T_A ON T (A)").unwrap();
    // A group, so the loads after go to groups too: no row of the tree is past its last.
    assert!(load(&s, INTO_T, &keyed(0..100), Load::Alone).unwrap());
    s.commit().unwrap();
    db.checkpoint().unwrap();
    db
}

/// Verifies: [DESIGN] RULES.md D1235, D25: a load by column into a table
/// of two indexes, a part of which fails past its extent logged, before
/// any entry, between the two indexes' entries, or after both, leaves T
/// and its indexes as its part before left them; a crash before the load
/// commits, or after, leaves them as they were before it, or with each of
/// its rows, the indexes, reopened as their checkpoint left them, mended.
#[test]
fn a_crash_at_each_point_of_a_group_leaves_its_indexes_whole() {
    for fails in 0..3 {
        let fs = Arc::new(MemoryFileSystem::new());
        let db = checkpointed(&fs);
        let s = db.session("S");
        assert!(load(&s, INTO_T, &keyed(1000..5096), Load::Going).unwrap());
        FAIL_AFTER.with(|f| f.set(Some(fails)));
        let failed = load(&s, INTO_T, &keyed(6000..10_096), Load::Going);
        FAIL_AFTER.with(|f| f.set(None));
        assert!(failed.is_err(), "fails after {fails} indexes");
        assert_eq!(one(&s, "SELECT COUNT(*) FROM T"), "4196", "as the part before left it");
        assert!(entries(&db).iter().all(|(_, n)| *n == 4196));
        let uncommitted = image(&fs);
        load(&s, INTO_T, &[vec![], vec![]], Load::Ending).unwrap();
        s.commit().unwrap();
        let committed = image(&fs);
        assert!(whole(&opened(&uncommitted), 100), "crashed before the commit, failed after {fails}");
        let after = opened(&committed);
        assert!(whole(&after, 4196), "crashed after the commit, failed after {fails}");
        let r = after.session("S");
        let found = one(&r, "SELECT COUNT(*) FROM T WHERE K = 6000 OR A = 96 AND K > 1000");
        assert_eq!(found, (1000_i64..5096).filter(|k| k % 97 == 96).count().to_string());
        assert!(r.run("INSERT INTO T VALUES (1500, 1)").is_err(), "still unique");
    }
}

/// Verifies: [DESIGN] RULES.md D1235, D25: a checkpoint taken while a load
/// by column is open holds its group's entries; a crash before it commits
/// leaves no entry of them, each taken back by the rows its extent holds,
/// and one after leaves each once.
#[test]
fn a_checkpoint_within_a_load_leaves_no_entry_of_it_after_a_crash() {
    let fs = Arc::new(MemoryFileSystem::new());
    let db = checkpointed(&fs);
    let s = db.session("S");
    assert!(load(&s, INTO_T, &keyed(1000..5096), Load::Going).unwrap());
    db.checkpoint().unwrap();
    let uncommitted = image(&fs);
    load(&s, INTO_T, &[vec![], vec![]], Load::Ending).unwrap();
    s.commit().unwrap();
    let committed = image(&fs);
    assert!(whole(&opened(&uncommitted), 100), "crashed before the commit");
    assert!(whole(&opened(&committed), 4196), "crashed after the commit");
}

/// Verifies: [DESIGN] RULES.md D1235, D1236: a key another transaction
/// is changing fails a load by column as it fails a multi-row INSERT of
/// the row path, SQLCODE -911, the whole statement undone, whether the
/// other's row is a version in the tree or a row of its open load.
#[test]
fn a_pending_key_fails_a_load_as_it_fails_a_multi_row_insert() {
    let db = in_memory();
    let a = db.session("S");
    assert!(load(&a, INTO_T, &keyed(10_000..10_100), Load::Alone).unwrap());
    a.commit().unwrap();
    a.run("INSERT INTO T VALUES (5, 5)").unwrap();
    let b = db.session("S");
    let rows = |keys: std::ops::Range<i64>| keys.map(|k| format!("({k}, 1)")).collect::<Vec<_>>().join(", ");
    let failed = |e: crate::error::SqlError| (e.code, e.sqlcode());
    let by_rows = b.run(&format!("INSERT INTO T VALUES {}", rows(0..10))).map_err(failed).unwrap_err();
    b.rollback().unwrap();
    let by_column = load(&b, INTO_T, &keyed(0..4096), Load::Alone).map_err(failed).unwrap_err();
    b.rollback().unwrap();
    assert_eq!(by_rows, (crate::error::Code::Conflict, -911_i32));
    assert_eq!(by_column, by_rows, "as the row path fails");
    a.rollback().unwrap();
    assert!(load(&a, INTO_T, &keyed(20_000..24_096), Load::Going).unwrap());
    let of_group = b.run(&format!("INSERT INTO T VALUES {}", rows(20_000..20_010))).map_err(failed).unwrap_err();
    b.rollback().unwrap();
    assert_eq!(of_group, (crate::error::Code::Conflict, -911_i32), "a row of an open load's group");
    load(&a, INTO_T, &[vec![], vec![]], Load::Ending).unwrap();
    a.rollback().unwrap();
    assert!(whole(&db, 100));
}
