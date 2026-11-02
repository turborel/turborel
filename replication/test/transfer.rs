//! State transfer (RULES.md D57): an export crossing in frames, a request
//! at a time, through the built-in whole-files transfer, and through the
//! page-difference transfer, which takes only what differs from the
//! fetcher's own files; the driver fetching one as its consensus asks and
//! installing it; the simulator's replicas fetching state once behind
//! what the others retain.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use turborel::db::{Database, OpenOptions, Standby, StateSource};
use turborel::disk::io::{FileSystem, OpenMode};
use turborel::disk::memory::MemoryFileSystem;
use turborel::error::Code;
use turborel_replication::{ANSWER, BLOCK, Fetching, PIECE, PageDifferences, REQUEST, ReplicaId, StateTransfer, WholeFiles};
use turborel_test_support::database::rows;
use turborel_test_support::replication::Make;
use turborel_test_support::replication::sim::plans;
use turborel_test_support::replication::sim::{simulate, simulate_databases};

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
fn crossed(fetch: &mut dyn Fetching, serving: &dyn StateTransfer, export: &mut dyn StateSource) -> u32 {
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

/// Every file under `from` of `fs` copied under `to`, directories made.
fn copy_tree(fs: &Arc<dyn FileSystem>, from: &Path, to: &Path) {
    fs.create_dir_all(to).unwrap();
    for path in fs.read_dir(from).unwrap() {
        let target = to.join(path.strip_prefix(from).unwrap());
        if fs.open(&path, OpenMode::ReadOnly).is_err() {
            copy_tree(fs, &path, &target);
            continue;
        }
        let source = fs.open(&path, OpenMode::ReadOnly).unwrap();
        let mut bytes = vec![0; usize::try_from(source.size().unwrap()).unwrap()];
        source.read_exact_at(&mut bytes, 0).unwrap();
        fs.open(&target, OpenMode::CreateNew).unwrap().write_all_at(&bytes, 0).unwrap();
    }
}

/// The bytes of file `file` of `source`, whole.
fn whole(source: &mut dyn StateSource, file: usize) -> Vec<u8> {
    let length = usize::try_from(source.files()[file].length).unwrap();
    let (mut bytes, mut at) = (vec![0; length], 0_usize);
    while at < length {
        let read = source.read(file, u64::try_from(at).unwrap(), &mut bytes[at..]).unwrap();
        assert!(read > 0_usize);
        at += read;
    }
    bytes
}

/// Verifies: [EXTENSION] RULES.md D57: the page-difference transfer: a
/// database behind the export, a copy of the same database at an earlier
/// checkpoint, fetches the manifest, then each file's summary of block
/// hashes, and asks only for the runs of blocks that differ from its
/// own, far fewer than the file's blocks, the rest copied from its own
/// file; through the same losses as whole files bear; what is fetched is
/// the export byte for byte, and the database behind installs it, holding
/// every row; a database with none of the files fetches them whole.
#[test]
fn page_differences_fetch_only_what_differs() {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    filled(&fs, "/source", 20000_i32).close().unwrap();
    copy_tree(&fs, Path::new("/source"), Path::new("/behind"));
    let source = Database::new();
    source.open_files(&fs, Path::new("/source"), &OpenOptions::default()).unwrap();
    let s = source.session("APP");
    for k in 20000_i32..30000_i32 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'row {k}')")).unwrap();
    }
    s.commit().unwrap();
    drop(s);
    source.checkpoint().unwrap();
    let mut export = source.export().unwrap();
    let (serving, fetching) =
        (PageDifferences::new(Arc::clone(&fs), Path::new("/serving")), PageDifferences::new(Arc::clone(&fs), Path::new("/behind")));
    assert_eq!(serving.name(), "page differences");
    let mut fetch = fetching.begin(PRIMARY).unwrap();
    let requests = crossed(&mut *fetch, &serving, &mut *export);
    let data = export.files().iter().position(|f| f.within == Path::new("data")).unwrap();
    let blocks = u32::try_from(export.files()[data].length.div_ceil(u64::try_from(BLOCK).unwrap())).unwrap();
    assert!(blocks > 8_u32 && requests < blocks, "{requests} requests for {blocks} blocks of the data file");
    let mut fetched = fetch.fetched().unwrap();
    assert_eq!(fetched.image(), export.image());
    for file in 0..export.files().len() {
        assert_eq!(whole(&mut *fetched, file), whole(&mut *export, file), "file {file}");
    }
    drop(export);
    let behind = Database::new();
    behind.open_following(&fs, Path::new("/behind"), &OpenOptions::default()).unwrap();
    assert_eq!(rows(&behind.session("APP"), "SELECT COUNT(*) FROM T"), Ok(vec!["20000".into()]), "behind, before the install");
    Standby::import(&behind, fetched).unwrap();
    assert_eq!(rows(&behind.session("APP"), "SELECT COUNT(*) FROM T"), Ok(vec!["30000".into()]));
    assert_eq!(rows(&behind.session("APP"), "SELECT S FROM T WHERE K = 29999"), Ok(vec![format!("{:200}", "row 29999")]));

    let mut again = source.export().unwrap();
    let empty = PageDifferences::new(Arc::clone(&fs), Path::new("/nothing"));
    let mut whole_fetch = empty.begin(PRIMARY).unwrap();
    let asked = crossed(&mut *whole_fetch, &serving, &mut *again);
    let pieces = u32::try_from(again.files()[data].length.div_ceil(u64::try_from(PIECE).unwrap())).unwrap();
    assert!(asked > pieces, "{asked} requests: every block fetched, in runs of a piece, {pieces} of them for the data file");
    let mut whole_fetched = whole_fetch.fetched().unwrap();
    assert_eq!(whole(&mut *whole_fetched, data), whole(&mut *again, data));
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
        match simulate(seed, &plans::retaining(), &make) {
            Ok(report) => transfers += report.transfers,
            Err(why) => failures.push(why),
        }
    }
    assert!(failures.is_empty(), "{} seeds failed:\n{}", failures.len(), failures.join("\n"));
    assert!(transfers > 0_u64, "no state transfer in 20 seeds");
    let mut with_files = 0_u64;
    for seed in 0..4_u64 {
        let report = simulate_databases(seed, &plans::retaining(), &make).unwrap();
        with_files += report.replication.transfers;
        assert_eq!(report.compared, 2_usize);
    }
    assert!(with_files > 0_u64, "no state transfer of a real database in 4 seeds");
}
