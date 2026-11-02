//! State transfer (RULES.md D57): an export crossing in frames, a request
//! at a time, through the built-in whole-files transfer; the driver
//! fetching one as its consensus asks and installing it; the simulator's
//! replicas fetching state once behind what the others retain.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Standby, StateSource};
use turborel::disk::io::FileSystem;
use turborel::disk::memory::MemoryFileSystem;
use turborel::error::Code;
use turborel_replication::{ANSWER, Fetching, REQUEST, ReplicaId, StateTransfer, WholeFiles};
use turborel_test_support::database::rows;
use turborel_test_support::replication::Make;
use turborel_test_support::replication::sim::{Faults, Plan, Weather, simulate, simulate_databases};

use crate::common::make_vr;

const PRIMARY: ReplicaId = ReplicaId(1);
const BACKUP: ReplicaId = ReplicaId(2);

/// A database in `fs` at `dir`, with `rows` rows of T committed and a
/// checkpoint taken.
fn filled(fs: &Arc<dyn FileSystem>, dir: &str, rows: i32) -> Database {
    let db = Database::new();
    db.open_files(fs, Path::new(dir), &OpenOptions::default()).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, S CHAR(200))").unwrap();
    let s = db.session("APP");
    for k in 0_i32..rows {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'row {k}')")).unwrap();
    }
    s.commit().unwrap();
    drop(s);
    db.checkpoint().unwrap();
    db
}

/// Drives `fetch` to its end against `serving`, which answers out of
/// `export` numbered 7: the second answer is lost, the third request is
/// answered once as of export 8 first, and each answer taken is tried as
/// another fetch's and after a malformed one; how many requests crossed.
fn crossed(fetch: &mut dyn Fetching, serving: &WholeFiles, export: &mut dyn StateSource) -> u32 {
    let (mut requests, mut lost, mut changed, mut done) = (0_u32, false, false, false);
    while !done {
        let Some(request) = fetch.request() else {
            fetch.waited();
            continue;
        };
        assert_eq!(request[0], REQUEST);
        requests += 1;
        let answer = serving.serve(export, (BACKUP, 7_u64), &request).unwrap().expect("a request is answered");
        assert_eq!(answer[0], ANSWER);
        if requests == 2_u32 && !lost {
            lost = true;
            continue;
        }
        if requests == 3_u32 && !changed {
            changed = true;
            let of_another = serving.serve(export, (BACKUP, 8_u64), &request).unwrap().unwrap();
            assert!(!fetch.take(&of_another).unwrap(), "another export's piece");
            assert_eq!(fetch.request().map(|r| r[9]), Some(1_u8), "the manifest asked again");
            continue;
        }
        let mut other = answer.clone();
        other[1] ^= 1;
        assert!(!fetch.take(&other).unwrap(), "another fetch's");
        assert_eq!(fetch.take(&[ANSWER, 0, 0]).err().map(|e| e.code), Some(Code::Connection), "malformed");
        done = fetch.take(&answer).unwrap();
    }
    requests
}

/// Verifies: [EXTENSION] RULES.md D57: the whole-files transfer: a fetch
/// asks for the manifest, then each file piece by piece, each request
/// answered out of the export with one frame, both beginning with their
/// tags; an answer lost is asked for again after a few ticks, and one of
/// another fetch, or malformed, changes nothing; a piece of another
/// export restarts the fetch from the manifest; what is fetched has the
/// export's image, files and bytes, and a following database installs
/// it; a fetch not done gives no export (SQLCODE -910).
#[test]
fn whole_files_cross_a_request_at_a_time() {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    let source = filled(&fs, "/source", 30000_i32);
    let mut export = source.export().unwrap();
    let (serving, fetching) =
        (WholeFiles::new(Arc::clone(&fs), PathBuf::from("/serving")), WholeFiles::new(Arc::clone(&fs), PathBuf::from("/transfer")));
    assert_eq!(serving.name(), "whole files");
    let mut fetch = fetching.begin(PRIMARY).unwrap();
    assert_eq!(Box::new(fetching.begin(PRIMARY).unwrap()).fetched().err().map(|e| e.code), Some(Code::Api), "not done");
    let requests = crossed(&mut *fetch, &serving, &mut *export);
    assert!(requests >= 7_u32, "{requests} requests: the manifest twice, three pieces at least, and those asked again");
    assert_eq!(serving.serve(&mut *export, (BACKUP, 7_u64), b"not a request").unwrap(), None);
    let fetched = fetch.fetched().unwrap();
    assert_eq!(fetched.image(), export.image());
    assert_eq!(fetched.files().len(), export.files().len());
    for (theirs, mine) in fetched.files().iter().zip(export.files()) {
        assert_eq!((theirs.within.clone(), theirs.length, &theirs.owner), (mine.within.clone(), mine.length, &mine.owner));
        assert!(theirs.path.starts_with("/transfer"), "{:?}", theirs.path);
    }
    drop(export);
    let follower = Database::new();
    follower.open_following(&fs, Path::new("/follower"), &OpenOptions::default()).unwrap();
    Standby::import(&follower, fetched).unwrap();
    assert_eq!(rows(&follower.session("APP"), "SELECT COUNT(*) FROM T"), Ok(vec!["30000".into()]));
    assert_eq!(rows(&follower.session("APP"), "SELECT S FROM T WHERE K = 29999"), Ok(vec![format!("{:200}", "row 29999")]));
}

/// A plan of three replicas whose logs keep little: a replica down a
/// while is behind what the others retain, and fetches the state.
fn retaining_little(seed_crashes: bool) -> Plan {
    let weather = Weather { drop: 20, duplicate: 10, delay: 5, late: 5, late_delay: 100 };
    let crash = if seed_crashes { 6 } else { 0 };
    let faults = Faults { weather, partition: 2, partition_moments: 300, crash, down_moments: 400, disk_error: 0, write_moments: 3 };
    Plan { replicas: 3, faulty: 3000, calm: 3000, append: 300, tick_moments: 5, retain_bytes: 200, faults }
}

/// Verifies: [EXTENSION] RULES.md D57: replicas keeping little of the
/// log, through crashes keeping each down a while, fetch state transfers
/// of the others and converge all the same, holding what the primary
/// holds: modelled databases, and real ones kept in files.
#[test]
fn replicas_behind_the_log_fetch_state_and_converge() {
    let make: Make = Arc::new(make_vr);
    let mut transfers = 0_u64;
    let mut failures = Vec::new();
    for seed in 0..20_u64 {
        match simulate(seed, &retaining_little(true), &make) {
            Ok(report) => transfers += report.transfers,
            Err(why) => failures.push(why),
        }
    }
    assert!(failures.is_empty(), "{} seeds failed:\n{}", failures.len(), failures.join("\n"));
    assert!(transfers > 0_u64, "no state transfer in 20 seeds");
    let mut with_files = 0_u64;
    for seed in 0..4_u64 {
        let report = simulate_databases(seed, &retaining_little(true), &make).unwrap();
        with_files += report.replication.transfers;
        assert_eq!(report.compared, 2_usize);
    }
    assert!(with_files > 0_u64, "no state transfer of a real database in 4 seeds");
}
