//! The log (RULES.md D25): the `Log` contract, checked for both built-in
//! logs by the same tests, and the file log's segments, recovery, truncation
//! and grouped syncs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use turborel::disk::block::HEADER_LEN;
use turborel::disk::io::{Durability, FileSystem, OsFileSystem};
use turborel::error::Code;
use turborel::log::{FileLog, FileLogOptions, Log, LogRecovery, Lsn, MemoryLog};

use crate::common::{Scratch, crc32c, os, small_segments};
use crate::logs::{self, BuiltIn, all, contract, record};

fn fs() -> Arc<dyn FileSystem> {
    os()
}

fn file_log(dir: &Path, options: FileLogOptions) -> FileLog {
    FileLog::create(fs(), dir, crc32c(), options).unwrap()
}

fn reopen(dir: &Path) -> (FileLog, LogRecovery) {
    FileLog::open(fs(), dir, &[crc32c()], &[], small_segments()).unwrap()
}

/// Verifies: [DESIGN] RULES.md D25, `Log` contract, memory log: positions,
/// reading back, syncs and truncation as the contract says; it is not
/// durable.
#[test]
fn the_memory_log_keeps_the_contract() {
    let log = MemoryLog::new();
    contract(&log);
    assert!(!log.capabilities().durable);
}

/// Verifies: [DESIGN] RULES.md D25, `Log` contract, file log: the same
/// contract, with records spread over many small segments; durable unless
/// its durability is `None`.
#[test]
fn the_file_log_keeps_the_contract() {
    let scratch = Scratch::new("log-contract");
    let log = file_log(&scratch.path("log"), small_segments());
    contract(&log);
    assert!(log.capabilities().durable && log.counts().rotations > 1, "{:?}", log.counts());
    let volatile = file_log(&scratch.path("volatile"), FileLogOptions { durability: Durability::None, ..small_segments() });
    assert!(!volatile.capabilities().durable);
}

/// The segment files in `dir`, sorted.
fn segments(dir: &Path) -> Vec<PathBuf> {
    OsFileSystem.read_dir(dir).unwrap()
}

/// Verifies: [DESIGN] RULES.md D25, file log: a log synced, closed and
/// reopened has the same records, across its segments, nothing cut, and
/// appends continue after them; truncation removes whole segments before
/// the position, never the one holding it or a later one.
#[test]
fn a_reopened_file_log_keeps_its_records() {
    let scratch = Scratch::new("log-reopen");
    let dir = scratch.path("log");
    let log = file_log(&dir, small_segments());
    let positions: Vec<Lsn> = (0..60).map(|n| log.append(&record(n)).unwrap()).collect();
    log.sync(positions[59]).unwrap();
    let before = all(&log, Lsn(0));
    let end = log.end();
    drop(log);
    let (log, recovery) = reopen(&dir);
    let count = u64::try_from(segments(&dir).len()).unwrap();
    assert_eq!(recovery, LogRecovery { segments: count, end, cut: 0 });
    assert_eq!(all(&log, Lsn(0)), before);
    assert!(log.append(b"after reopening").unwrap() >= end);
    log.truncate_before(positions[40]).unwrap();
    assert!(u64::try_from(segments(&dir).len()).unwrap() < count, "older segments removed");
    let kept = all(&log, Lsn(0));
    assert!(kept.iter().any(|(p, _)| *p == positions[40]) && kept.len() >= 21);
}

/// Verifies: [DESIGN] RULES.md D25, file log recovery: a torn end of the
/// last segment is cut and the records before it remain; a last segment a
/// crash left half created (shorter than a block file's header) is removed;
/// a segment missing between others is refused with SQLCODE -916.
#[test]
fn a_crashed_file_log_recovers() {
    let scratch = Scratch::new("log-crash");
    let dir = scratch.path("log");
    let log = file_log(&dir, small_segments());
    let positions: Vec<Lsn> = (0..30).map(|n| log.append(&record(n)).unwrap()).collect();
    log.sync(positions[29]).unwrap();
    drop(log);
    let files = segments(&dir);
    let last = files.last().unwrap();
    let len = std::fs::metadata(last).unwrap().len();
    std::fs::OpenOptions::new().write(true).open(last).unwrap().set_len(len - 5).unwrap();
    let (log, recovery) = reopen(&dir);
    assert!(recovery.cut > 0);
    let records = all(&log, Lsn(0));
    assert_eq!(records.len(), 29, "the torn last record is gone");
    assert_eq!(records, positions[..29].iter().copied().zip((0..29).map(record)).collect::<Vec<_>>());
    drop(log);
    let half = dir.join(format!("segment-{:010}.log", files.len()));
    std::fs::write(&half, b"TBBL").unwrap();
    assert!(u64::try_from(b"TBBL".len()).unwrap() < HEADER_LEN);
    let (log, _) = reopen(&dir);
    assert!(!half.exists(), "the half-created segment is removed");
    assert_eq!(all(&log, Lsn(0)).len(), 29);
    drop(log);
    std::fs::remove_file(&files[1]).unwrap();
    assert_eq!(FileLog::open(fs(), &dir, &[crc32c()], &[], small_segments()).err().map(|e| e.code), Some(Code::Corrupt));
}

/// Verifies: [DESIGN] RULES.md D25, file log: creating where a log exists,
/// opening where none is, and a segment size out of range are refused with
/// SQLCODE -910.
#[test]
fn file_logs_are_created_and_opened_where_they_should_be() {
    let scratch = Scratch::new("log-places");
    let dir = scratch.path("log");
    drop(file_log(&dir, small_segments()));
    assert_eq!(FileLog::create(fs(), &dir, crc32c(), small_segments()).err().map(|e| e.code), Some(Code::Api));
    let empty = scratch.path("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(FileLog::open(fs(), &empty, &[crc32c()], &[], small_segments()).err().map(|e| e.code), Some(Code::Api));
    for size in [0, 100, u64::MAX] {
        let made = FileLog::create(
            fs(),
            &scratch.path(&format!("size{size}")),
            crc32c(),
            FileLogOptions { segment_size: size, ..small_segments() },
        );
        assert_eq!(made.err().map(|e| e.code), Some(Code::Api), "segment size {size}");
    }
}

/// Committers syncing at once, and commits each makes.
const COMMITTERS: usize = 8;
const COMMITS_EACH: usize = 10;

/// Verifies: [DESIGN] RULES.md D25, file log, group commit: threads that
/// append and sync at once all have their records durable and read back,
/// and never need more syncs than they asked for.
#[test]
fn syncs_at_once_are_grouped() {
    let scratch = Scratch::new("log-group");
    let dir = scratch.path("log");
    let log = file_log(&dir, FileLogOptions::default());
    thread::scope(|s| {
        for t in 0..COMMITTERS {
            let log = &log;
            s.spawn(move || {
                for n in 0..COMMITS_EACH {
                    let at = log.append(format!("commit {t}.{n}").as_bytes()).unwrap();
                    log.sync(at).unwrap();
                }
            });
        }
    });
    let counts = log.counts();
    assert_eq!(counts.appends, u64::try_from(COMMITTERS * COMMITS_EACH).unwrap());
    assert!(counts.syncs <= counts.sync_requests, "{counts:?}");
    drop(log);
    let (log, _) = FileLog::open(fs(), &dir, &[crc32c()], &[], FileLogOptions::default()).unwrap();
    assert_eq!(all(&log, Lsn(0)).len(), COMMITTERS * COMMITS_EACH);
}

/// Verifies: [DESIGN] RULES.md D25, file log: a position in a segment that
/// is no longer current can be synced; reading meets a record damaged in
/// the middle of the log as an error (SQLCODE -916), after the records
/// before it, and then stops rather than repeat it.
#[test]
fn old_segments_sync_and_damage_stops_a_read() {
    let scratch = Scratch::new("log-damage");
    let dir = scratch.path("log");
    let log = file_log(&dir, small_segments());
    let positions: Vec<Lsn> = (0..40).map(|n| log.append(&record(n)).unwrap()).collect();
    assert!(log.counts().rotations > 1);
    log.sync(positions[0]).unwrap();
    log.sync(positions[39]).unwrap();
    let first_segment = segments(&dir)[0].clone();
    let mut bytes = std::fs::read(&first_segment).unwrap();
    let at = usize::try_from(HEADER_LEN).unwrap() + 20;
    bytes[at] ^= 0xFF;
    std::fs::write(&first_segment, bytes).unwrap();
    let mut records = log.read_from(Lsn(0));
    let first = records.next().unwrap();
    assert_eq!(first.err().map(|e| e.code), Some(Code::Corrupt), "the first record was damaged");
    assert!(records.next().is_none(), "the read stops");
}

/// Verifies: [DESIGN] RULES.md D25, log records through a file log: a
/// transaction's records (rows inserted, updated and deleted, then its
/// commit) appended, synced at the commit, and read back after reopening,
/// decode to what was written, in order.
#[test]
fn records_go_through_the_log() {
    let scratch = Scratch::new("log-records");
    logs::records_go_through(&BuiltIn(small_segments()), &scratch.path("log"));
}

/// Verifies: [DESIGN] RULES.md D25, file log: opened as known durable
/// through a position, its segments before that position's are taken
/// unchecked, so damage in them is not read as it opens; a segment cut
/// short ends the log, the segments after it removed, so no record after
/// the gap is taken.
#[test]
fn a_file_log_opened_through_a_position_checks_what_follows() {
    let scratch = Scratch::new("log-through");
    let dir = scratch.path("log");
    let log = file_log(&dir, small_segments());
    let positions: Vec<Lsn> = (0..60).map(|n| log.append(&record(n)).unwrap()).collect();
    log.sync(positions[59]).unwrap();
    let end = log.end();
    drop(log);
    let files = segments(&dir);
    assert!(files.len() > 3, "{}", files.len());
    // A byte of the first segment's first block's payload, damaged.
    let first = &files[0];
    let mut bytes = std::fs::read(first).unwrap();
    let at = usize::try_from(HEADER_LEN).unwrap() + 10;
    bytes[at] ^= 0xFF;
    std::fs::write(first, &bytes).unwrap();
    let through = |at: Lsn| FileLog::open_through(fs(), &dir, (&[crc32c()], &[]), small_segments(), at);
    let (log, recovery) = through(end).unwrap();
    assert_eq!((recovery.end, recovery.cut), (end, 0), "the damage is before what it checks");
    drop(log);
    bytes[at] ^= 0xFF;
    std::fs::write(first, &bytes).unwrap();

    // The second segment cut short: the log ends there.
    let second = &files[1];
    let len = std::fs::metadata(second).unwrap().len();
    std::fs::OpenOptions::new().write(true).open(second).unwrap().set_len(len - 3).unwrap();
    let (log, recovery) = through(Lsn(0)).unwrap();
    assert!(recovery.cut > len, "the later segments go too: {recovery:?}");
    assert_eq!(segments(&dir).len(), 2);
    assert!(log.end() < positions[59]);
}

/// Every file in `dir`, sorted, with its bytes.
fn contents(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    segments(dir).into_iter().map(|p| (p.clone(), std::fs::read(&p).unwrap())).collect()
}

/// Verifies: [DESIGN] RULES.md D25, file log recovery: a segment damaged in
/// its middle, a block failing its checksum before valid ones, refuses the
/// log with SQLCODE -916, naming the segment and the offset, and so does
/// one whose last block is damaged while a later segment holds valid
/// blocks; either way no segment is cut or removed. Mended, the log opens
/// with nothing cut.
///
/// Regression test (R-TEST-7): the damaged segment was cut at the damaged
/// block and every later segment removed, losing committed records.
#[test]
fn a_damaged_file_log_is_refused_unchanged() {
    let scratch = Scratch::new("log-damaged");
    let dir = scratch.path("log");
    let log = file_log(&dir, small_segments());
    let positions: Vec<Lsn> = (0..60).map(|n| log.append(&record(n)).unwrap()).collect();
    log.sync(positions[59]).unwrap();
    drop(log);
    let second = segments(&dir)[1].clone();
    let size = std::fs::metadata(&second).unwrap().len();
    let bytes = std::fs::read(&second).unwrap();
    let trailing = u64::from(u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap()));
    let last_block = size - trailing - 12;
    assert!(last_block > HEADER_LEN, "segment 1 holds more than one block");
    for (at, whose) in [(HEADER_LEN, "a valid block follows"), (last_block, "segment 2 after it holds a valid block")] {
        let mut damaged = bytes.clone();
        damaged[usize::try_from(at + 9).unwrap()] ^= 0x55;
        std::fs::write(&second, &damaged).unwrap();
        let before = contents(&dir);
        let refused = FileLog::open(fs(), &dir, &[crc32c()], &[], small_segments()).err().unwrap();
        assert_eq!(refused.code, Code::Corrupt, "{whose}");
        assert!(
            refused.msg.contains(&format!("segment-0000000001.log is damaged at offset {at}")) && refused.msg.contains(whose),
            "{refused}"
        );
        assert_eq!(contents(&dir), before, "{whose}: unchanged");
    }
    std::fs::write(&second, &bytes).unwrap();
    let (log, recovery) = reopen(&dir);
    assert_eq!(recovery.cut, 0);
    assert_eq!(all(&log, Lsn(0)).len(), 60);
}
