//! The block manager (RULES.md D30): appending and reading blocks, cursors
//! in both directions, recovery of a torn or corrupt tail, concurrent
//! appenders, and failed writes, on the real file system and on a test
//! file system that injects torn writes.

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::thread;

use turborel::disk::block::{BlockFile, BlockId, BlockOptions, HEADER_LEN, Recovery, TailPolicy};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::{Durability, OsFileSystem};
use turborel::error::{Code, Result};
use turborel::hash::HashFunction;
use turborel::hash::crc::{CRC64_NVME, Crc};

use crate::common::{Scratch, crc32c};
use crate::faults::{FaultyFs, Rule};

fn create(path: &Path) -> BlockFile {
    BlockFile::create(&OsFileSystem, path, crc32c(), Pipeline::none(), BlockOptions::default()).unwrap()
}

fn open(path: &Path, tail: TailPolicy) -> Result<(BlockFile, Recovery)> {
    BlockFile::open(&OsFileSystem, path, &[crc32c()], &[], BlockOptions { tail, ..BlockOptions::default() })
}

/// Bytes a block of `len` payload bytes takes with CRC-32C: two lengths and
/// a 4-byte digest around the payload.
fn frame(len: u64) -> u64 {
    len + 12
}

/// Every block from `cursor`'s position forwards, or backwards.
fn walk(file: &BlockFile, from: BlockId, forwards: bool) -> Vec<(BlockId, Vec<u8>)> {
    let mut cursor = file.cursor(from);
    let mut payload = Vec::new();
    let mut out = Vec::new();
    loop {
        let step = if forwards { cursor.next(&mut payload) } else { cursor.prev(&mut payload) };
        let Some(id) = step.unwrap() else { return out };
        out.push((id, payload.clone()));
    }
}

/// Changes byte `at` of the file at `path`.
fn corrupt(path: &Path, at: u64, byte: u8) {
    let mut f = OpenOptions::new().write(true).open(path).unwrap();
    f.seek(SeekFrom::Start(at)).unwrap();
    f.write_all(&[byte]).unwrap();
}

/// Verifies: [DESIGN] RULES.md D30, block manager: blocks of any size up to
/// the limit, the empty one included, are appended at increasing ids that
/// are their byte offsets, read back by id, and walked by a cursor forwards
/// from any block and backwards from the end; the counters count them.
#[test]
fn appended_blocks_read_back_in_both_directions() {
    let scratch = Scratch::new("blocks-read");
    let file = create(&scratch.path("b"));
    assert_eq!((file.first(), file.end()), (BlockId(HEADER_LEN), BlockId(HEADER_LEN)));
    let payloads: Vec<Vec<u8>> = vec![b"first".to_vec(), Vec::new(), vec![7; 5000], b"last".to_vec()];
    let ids: Vec<BlockId> = payloads.iter().map(|p| file.append(p).unwrap()).collect();
    let mut expected = HEADER_LEN;
    for (id, p) in ids.iter().zip(&payloads) {
        assert_eq!(*id, BlockId(expected));
        assert_eq!(&file.read(*id).unwrap(), p);
        expected += frame(u64::try_from(p.len()).unwrap());
    }
    assert_eq!(file.end(), BlockId(expected));
    let forwards = walk(&file, file.first(), true);
    assert_eq!(forwards, ids.iter().copied().zip(payloads.iter().cloned()).collect::<Vec<_>>());
    let mut backwards = walk(&file, file.end(), false);
    backwards.reverse();
    assert_eq!(backwards, forwards);
    assert_eq!(walk(&file, ids[2], true).len(), 2, "from the third block");
    assert_eq!(walk(&file, ids[2], false).len(), 2, "back from the third block");
    let counts = file.counts();
    assert_eq!((counts.appends, counts.appended_bytes, counts.corrupt_reads), (4, 5009, 0));
    assert!(counts.reads >= 4);
}

/// Verifies: [DESIGN] RULES.md D30: a block file synced, closed and opened
/// again has the same blocks, nothing is cut, and appends continue at its
/// end; it remembers its hash function, and opening it without that
/// function fails with SQLCODE -910.
#[test]
fn a_reopened_file_keeps_its_blocks() {
    let scratch = Scratch::new("blocks-reopen");
    let path = scratch.path("b");
    let nvme: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC64_NVME).unwrap());
    let file = BlockFile::create(&OsFileSystem, &path, Arc::clone(&nvme), Pipeline::none(), BlockOptions::default()).unwrap();
    let first = file.append(b"one").unwrap();
    file.sync(Durability::Full).unwrap();
    let end = file.end();
    drop(file);
    let (file, recovery) = BlockFile::open(&OsFileSystem, &path, &[crc32c(), nvme], &[], BlockOptions::default()).unwrap();
    assert_eq!(recovery, Recovery { end, cut: 0, follows: None, interrupted: false });
    assert_eq!(file.hash_id(), CRC64_NVME.id);
    assert_eq!(file.read(first).unwrap(), b"one");
    assert_eq!(file.append(b"two").unwrap(), end);
    drop(file);
    let without = BlockFile::open(&OsFileSystem, &path, &[crc32c()], &[], BlockOptions::default());
    assert_eq!(without.err().map(|e| e.code), Some(Code::Api));
}

/// Three synced blocks "aaaa", "bbbb", "cccc" in a closed file; the ids.
fn three_blocks(path: &Path) -> [BlockId; 3] {
    let file = create(path);
    let ids = [b"aaaa", b"bbbb", b"cccc"].map(|p| file.append(p).unwrap());
    file.sync(Durability::Full).unwrap();
    ids
}

/// A way to damage a closed block file, given its last block.
type Damage = fn(&Path, BlockId);

/// Verifies: [DESIGN] RULES.md D30, recovery: on open, a last block that is
/// incomplete, fails its checksum, or has a wrong trailing length, and
/// bytes after the last block, are cut off under `TailPolicy::Truncate`,
/// durably, reporting how many bytes; the earlier blocks remain and
/// appending continues where the valid blocks end.
///
/// MC/DC of the block check (`format::verify`): trailing length wrong with
/// the digest right; digest wrong with the trailing length right; both
/// right (the earlier blocks).
#[test]
fn a_torn_or_corrupt_tail_is_cut() {
    let scratch = Scratch::new("blocks-tail");
    let damages: [(&str, Damage); 4] = [
        ("incomplete", |p, last| OpenOptions::new().write(true).open(p).unwrap().set_len(last.0 + 6).unwrap()),
        ("payload changed", |p, last| corrupt(p, last.0 + 8, b'X')),
        ("trailing length changed", |p, last| corrupt(p, last.0 + 12, 9)),
        ("garbage after the blocks", |p, _| corrupt(p, frame(4) * 3 + HEADER_LEN + 20, 1)),
    ];
    for (i, (damage, apply)) in damages.into_iter().enumerate() {
        let path = scratch.path(&format!("b{i}"));
        let [a, b, c] = three_blocks(&path);
        let end = c.0 + frame(4);
        apply(&path, c);
        let size = std::fs::metadata(&path).unwrap().len();
        let valid_end = if i == 3 { end } else { c.0 };
        let (file, recovery) = open(&path, TailPolicy::Truncate).unwrap();
        let torn = Recovery { end: BlockId(valid_end), cut: size - valid_end, follows: None, interrupted: i == 0 };
        assert_eq!(recovery, torn, "{damage}: only the incomplete one runs past the end");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), valid_end, "{damage}: cut in the file");
        assert_eq!((file.read(a).unwrap(), file.read(b).unwrap()), (b"aaaa".to_vec(), b"bbbb".to_vec()));
        assert_eq!(file.append(b"next").unwrap(), BlockId(valid_end), "{damage}");
    }
}

/// Verifies: [DESIGN] RULES.md D30: under `TailPolicy::Refuse`, a file with
/// invalid bytes at its end is refused with SQLCODE -916 and left as it
/// was; one without opens.
#[test]
fn refusing_a_bad_tail_leaves_the_file_unchanged() {
    let scratch = Scratch::new("blocks-refuse");
    let path = scratch.path("b");
    let [_, _, c] = three_blocks(&path);
    assert_eq!(open(&path, TailPolicy::Refuse).unwrap().1.cut, 0);
    corrupt(&path, c.0 + 8, b'X');
    let size = std::fs::metadata(&path).unwrap().len();
    assert_eq!(open(&path, TailPolicy::Refuse).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(std::fs::metadata(&path).unwrap().len(), size);
}

/// Verifies: [DESIGN] RULES.md D30: reading where no valid block starts, or
/// a block whose bytes changed, fails with SQLCODE -916 and is counted; a
/// cursor stepping back across a trailing length that leads before the
/// first block fails the same way; a payload longer than `max_block` is
/// refused with SQLCODE -910.
#[test]
fn invalid_reads_and_oversized_blocks_are_refused() {
    let scratch = Scratch::new("blocks-invalid");
    let path = scratch.path("b");
    let [a, b, _] = three_blocks(&path);
    let (file, _) = open(&path, TailPolicy::Truncate).unwrap();
    assert_eq!(file.read(BlockId(a.0 + 1)).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(file.read(file.end()).err().map(|e| e.code), Some(Code::Corrupt), "nothing at the end");
    corrupt(&path, b.0 + 8, b'X');
    assert_eq!(file.read(b).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(file.counts().corrupt_reads, 3);
    corrupt(&path, b.0 - 1, 0xff);
    assert_eq!(file.cursor(b).prev(&mut Vec::new()).err().map(|e| e.code), Some(Code::Corrupt));
    let small = BlockFile::create(
        &OsFileSystem,
        &scratch.path("small"),
        crc32c(),
        Pipeline::none(),
        BlockOptions { max_block: 8, ..BlockOptions::default() },
    )
    .unwrap();
    assert!(small.append(&[0; 8]).is_ok());
    assert_eq!(small.append(&[0; 9]).err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [DESIGN] RULES.md D30: a file that is not a block file (too
/// short, wrong magic number, unknown format version, header failing its
/// checksum) is refused with SQLCODE -916; creating over an existing file
/// fails with SQLCODE -915.
#[test]
fn files_that_are_not_block_files_are_refused() {
    let scratch = Scratch::new("blocks-header");
    let path = scratch.path("b");
    drop(create(&path));
    assert_eq!(
        BlockFile::create(&OsFileSystem, &path, crc32c(), Pipeline::none(), BlockOptions::default()).err().map(|e| e.code),
        Some(Code::Io)
    );
    for (at, byte, damage) in [(0, b'X', "magic"), (8, 9, "version"), (36, 0xA5, "header digest"), (16, 7, "encodings")] {
        let p = scratch.path(damage);
        std::fs::copy(&path, &p).unwrap();
        corrupt(&p, at, byte);
        assert_eq!(open(&p, TailPolicy::Truncate).err().map(|e| e.code), Some(Code::Corrupt), "{damage}");
    }
    let short = scratch.path("short");
    std::fs::write(&short, b"TBBLOCKS").unwrap();
    assert_eq!(open(&short, TailPolicy::Truncate).err().map(|e| e.code), Some(Code::Corrupt));
}

/// Threads appending at once, and blocks each appends.
const APPENDERS: usize = 8;
const BLOCKS_EACH: usize = 200;

/// Verifies: [DESIGN] RULES.md D30: appenders on several threads write at
/// once without losing or mixing blocks: every block is readable, the
/// published blocks have no gaps (a cursor walks them all), and the file
/// reopens with nothing cut.
#[test]
fn appenders_on_many_threads_write_at_once() {
    let scratch = Scratch::new("blocks-threads");
    let path = scratch.path("b");
    let file = Arc::new(create(&path));
    let appenders: Vec<_> = (0..APPENDERS)
        .map(|t| {
            let file = Arc::clone(&file);
            thread::spawn(move || {
                (0..BLOCKS_EACH)
                    .map(|n| {
                        let payload = format!("{t}:{n}:{}", "x".repeat(n % 50)).into_bytes();
                        (file.append(&payload).unwrap(), payload)
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut appended: Vec<(BlockId, Vec<u8>)> = appenders.into_iter().flat_map(|a| a.join().unwrap()).collect();
    appended.sort();
    assert_eq!(walk(&file, file.first(), true), appended);
    file.sync(Durability::Data).unwrap();
    let end = file.end();
    drop(file);
    assert_eq!(open(&path, TailPolicy::Refuse).unwrap().1, Recovery { end, cut: 0, follows: None, interrupted: false });
}

/// Verifies: [DESIGN] RULES.md D30: a torn write fails its append with the
/// file system's error, and every later append is refused (SQLCODE -915),
/// since a gap now precedes them; the blocks before stay readable; on
/// reopening, the torn block is cut and appending resumes there.
#[test]
fn a_failed_write_stops_appends_until_recovery() {
    let scratch = Scratch::new("blocks-torn");
    let path = scratch.path("b");
    let (fs, faults) = FaultyFs::with_faults();
    let file = BlockFile::create(&*fs, &path, crc32c(), Pipeline::none(), BlockOptions::default()).unwrap();
    let kept = file.append(b"kept").unwrap();
    let end = file.end();
    faults.inject(Rule::tear().times(1));
    assert!(file.append(b"torn block").unwrap_err().msg.contains("injected"));
    let refused = file.append(b"after").unwrap_err();
    assert_eq!(refused.code, Code::Io);
    assert!(refused.msg.contains("until it is reopened"), "{refused}");
    assert_eq!((file.read(kept).unwrap(), file.end()), (b"kept".to_vec(), end));
    drop(file);
    let (file, recovery) = BlockFile::open(&*fs, &path, &[crc32c()], &[], BlockOptions::default()).unwrap();
    assert_eq!(recovery, Recovery { end, cut: frame(10).div_euclid(2), follows: None, interrupted: true });
    assert_eq!(file.append(b"again").unwrap(), end);
}

/// Verifies: [DESIGN] RULES.md D30: a block file opened from a point it is
/// known valid through checks only what follows: damage before it is not
/// read as it opens; a point where no valid block ends, or past the file's
/// end, is not taken, and every block is checked, so the damage, valid
/// blocks after it, is refused (SQLCODE -916). A small block is read in
/// one call, a large one in two.
#[test]
fn a_file_opened_from_a_known_end_checks_only_what_follows() {
    use turborel::disk::measured::MeasuredFileSystem;
    let scratch = Scratch::new("blocks-known-end");
    let path = scratch.path("f");
    let ids = three_blocks(&path);
    let opened = |from: u64| {
        BlockFile::open_from(&OsFileSystem, &path, (&[crc32c()], &[]), BlockOptions::default(), BlockId(from))
            .map(|(file, _)| file.end())
            .map_err(|e| e.code)
    };
    let end = std::fs::metadata(&path).unwrap().len();
    corrupt(&path, ids[0].0 + 8, 0xEE);
    assert_eq!(opened(end), Ok(BlockId(end)), "the first block's damage is before the known end");
    assert_eq!(opened(ids[1].0 + 3), Err(Code::Corrupt), "mid-block: every block checked, the first one damaged");
    assert_eq!(opened(end + 100), Err(Code::Corrupt), "past the end");

    let other = scratch.path("g");
    let measured = MeasuredFileSystem::new(Box::new(OsFileSystem));
    let stats = measured.stats();
    let file = BlockFile::create(&measured, &other, crc32c(), Pipeline::none(), BlockOptions::default()).unwrap();
    let small = file.append(&[7; 100]).unwrap();
    let large = file.append(&vec![9; 10_000]).unwrap();
    let before = stats.snapshot();
    assert_eq!(file.read(small).unwrap(), vec![7; 100]);
    assert_eq!(stats.snapshot().since(&before).reads, 1, "a small block in one read");
    let before = stats.snapshot();
    assert_eq!(file.read(large).unwrap(), vec![9; 10_000]);
    assert_eq!(stats.snapshot().since(&before).reads, 2, "a large one in two");
}

/// Blocks of `payloads` in a new file at `path`, synced; their ids.
fn blocks_of(path: &Path, payloads: &[Vec<u8>]) -> Vec<BlockId> {
    let file = create(path);
    let ids = payloads.iter().map(|p| file.append(p).unwrap()).collect();
    file.sync(Durability::Full).unwrap();
    ids
}

/// `len` bytes, none of them zero.
fn nonzero(len: usize) -> Vec<u8> {
    (0..len).map(|n| u8::try_from(n % 251).unwrap() + 1).collect()
}

/// Sets bytes `from..to` of the file at `path` to zero.
fn zero(path: &Path, from: u64, to: u64) {
    let mut f = OpenOptions::new().write(true).open(path).unwrap();
    f.seek(SeekFrom::Start(from)).unwrap();
    f.write_all(&vec![0; usize::try_from(to - from).unwrap()]).unwrap();
}

/// Verifies: [DESIGN] RULES.md D25, D30, recovery: a block in the middle of
/// a file that fails its checksum, valid blocks after it, is damage, not a
/// torn end: opening refuses the file with SQLCODE -916, under either tail
/// policy, naming the offsets of the damage and of the valid block after
/// it, and leaves the file as it was. A damaged stretch longer than a scan's
/// chunk, before a block longer than what a chunk reads ahead, is found
/// the same way.
///
/// Regression test (R-TEST-7): the file was cut at the damaged block,
/// durably, losing the valid blocks after it.
#[test]
fn damage_before_valid_blocks_is_refused() {
    let scratch = Scratch::new("blocks-damage");
    let small = scratch.path("small");
    let [_, b, c] = three_blocks(&small);
    corrupt(&small, b.0 + 8, b'X');
    let large = scratch.path("large");
    let ids = blocks_of(&large, &[b"a".to_vec(), nonzero(200_000), nonzero(100_000), b"d".to_vec()]);
    corrupt(&large, ids[1].0 + 9, 0);
    for (path, damaged, follows) in [(&small, b, c), (&large, ids[1], ids[2])] {
        let before = std::fs::read(path).unwrap();
        for tail in [TailPolicy::Truncate, TailPolicy::Refuse] {
            let refused = open(path, tail).err().unwrap();
            assert_eq!(refused.code, Code::Corrupt, "{tail:?}");
            let offsets = format!(
                "damaged at offset {}: the bytes there are not a valid block, but a valid block follows at offset {}",
                damaged.0, follows.0
            );
            assert!(refused.msg.contains(&offsets), "{refused}");
            assert_eq!(std::fs::read(path).unwrap(), before, "{tail:?}: unchanged");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D25, D30, recovery: invalid bytes before
/// valid blocks that show a write a crash interrupted, as the system may
/// write a file's pages back in any order before a sync, are a torn end,
/// cut with the blocks after them: a sector of them reading as zeros,
/// never written; or a zero length ending them, a block's write cut short.
///
/// MC/DC of `interrupted` (`disk/block/tail.rs`): a zero sector alone; a
/// zero end alone; a block running past the end alone
/// (`a_torn_or_corrupt_tail_is_cut`); none (`damage_before_valid_blocks_is_refused`).
#[test]
fn an_interrupted_write_before_valid_blocks_is_cut() {
    let scratch = Scratch::new("blocks-interrupted");
    // A sector of the second block's payload never written; its ends were.
    let sector = scratch.path("sector");
    let ids = blocks_of(&sector, &[b"a".to_vec(), nonzero(2_000), b"c".to_vec()]);
    zero(&sector, ids[1].0 + 600, ids[1].0 + 1_700);
    // The second half of a small block never written.
    let end = scratch.path("end");
    let [a, b, c] = three_blocks(&end);
    zero(&end, b.0 + 8, c.0);
    for (path, cut_at, follows) in [(&sector, ids[1], ids[2]), (&end, b, c)] {
        let size = std::fs::metadata(path).unwrap().len();
        let (file, recovery) = open(path, TailPolicy::Truncate).unwrap();
        assert_eq!(recovery, Recovery { end: cut_at, cut: size - cut_at.0, follows: Some(follows), interrupted: true });
        assert_eq!(recovery.damage(), None);
        assert_eq!(std::fs::metadata(path).unwrap().len(), cut_at.0, "cut, the blocks after it with it");
        assert_eq!(file.end(), cut_at);
    }
    assert_eq!(open(&end, TailPolicy::Truncate).unwrap().0.read(a).unwrap(), b"aaaa");
}
