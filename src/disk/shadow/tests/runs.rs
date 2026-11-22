//! Unit tests of runs of whole granules in side files: a page written
//! whole takes one block, read back with one read and no check, and the
//! side file's blocks are still checked where a crash leaves them
//! (RULES.md R-TEST-1, R-TEST-2, R-TEST-7).

use super::*;
use crate::disk::block::{BlockFile, BlockId, BlockOptions, HEADER_LEN};
use crate::disk::encoding::Pipeline;

/// A page's worth of granules.
const PAGE: usize = 2 * GRANULE;

/// Where, in a side file of CRC-32 blocks, its first block's payload
/// starts: past the file's header, the length and the checksum.
const FIRST_PAYLOAD: u64 = HEADER_LEN + 8;

/// A file committed holding `kept`, then a page of 5s and one of 6s
/// written whole from granule 2, sealed and its side file ended with a
/// commit block, a crash then: the side file `1` committed.
fn committed_runs(disk: &Arc<dyn FileSystem>, kept: &[u8]) {
    let shadows = ShadowFileSystem::new(Arc::clone(disk), hash());
    let file = shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap();
    file.write_all_at(kept, 0).unwrap();
    shadows.commit().unwrap();
    file.write_all_at(&[5_u8; PAGE], side::offset(2)).unwrap();
    file.write_all_at(&[6_u8; PAGE], side::offset(4)).unwrap();
    shadows.seal().unwrap();
    crash_after_commit_block(&shadows, PATH, file::closing_ops);
}

/// Verifies: [DESIGN] RULES.md D30, D471, runs: a page written whole
/// takes one block, written over in place as it is written again, each
/// granule read from its place in it; a
/// granule of it written in part takes a block of its own, read back with
/// the rest of the page from the run; the page written whole again takes
/// a new block; reads see the newest throughout, and a commit copies it.
#[test]
fn a_page_written_whole_takes_one_block() {
    let disk = disk();
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    let file = shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap();
    file.write_all_at(&[1_u8; PAGE], 0).unwrap();
    let one = shadows.pending_bytes();
    assert!(one < u64::try_from(PAGE + GRANULE).unwrap(), "one block, {one} bytes");
    for round in 2_u8..20 {
        file.write_all_at(&[[round; GRANULE], [round + 100; GRANULE]].concat(), 0).unwrap();
    }
    assert_eq!(shadows.pending_bytes(), one, "written over in place");
    file.write_all_at(b"part", u64::try_from(GRANULE).unwrap() + 10).unwrap();
    assert!(shadows.pending_bytes() > one, "a block of its own");
    let expected = |at_part: &[u8]| {
        let mut page = [[19_u8; GRANULE], [119_u8; GRANULE]].concat();
        page.splice(GRANULE + 10..GRANULE + 14, at_part.iter().copied());
        page
    };
    let mut back = vec![0_u8; PAGE];
    file.read_exact_at(&mut back, 0).unwrap();
    assert_eq!(back, expected(b"part"));
    let two = shadows.pending_bytes();
    file.write_all_at(&[30_u8; PAGE], 0).unwrap();
    assert!(shadows.pending_bytes() > two, "the run broken, a new block");
    shadows.commit().unwrap();
    assert_eq!(content(&*disk), vec![30_u8; PAGE]);
}

/// Verifies: [DESIGN] RULES.md D30, D471, runs: a committed side file of
/// runs recovers into the file, each granule where its run put it.
#[test]
fn runs_recover_into_the_file() {
    let disk = disk();
    committed_runs(&disk, b"kept");
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    drop(shadows.open(Path::new(PATH), OpenMode::ReadWrite).unwrap());
    let mut expected = b"kept".to_vec();
    expected.resize(2 * GRANULE, 0);
    expected.extend([[5_u8; PAGE], [6_u8; PAGE]].concat());
    assert_eq!(content(&*disk), expected);
}

/// Verifies: [DESIGN] RULES.md D30, D471, runs: a byte of a run's head
/// (its first granule's number) or of its body changed in a committed
/// side file is found as it is opened, its block failing its checksum:
/// refused with SQLCODE -916, the file as its last commit left it.
#[test]
fn a_run_changed_on_disk_is_refused() {
    for at in [FIRST_PAYLOAD + 3, FIRST_PAYLOAD + 9 + 1_000] {
        let disk = disk();
        committed_runs(&disk, b"kept");
        let side = disk.open(&side_path(Path::new(PATH), 1), OpenMode::ReadWrite).unwrap();
        let mut byte = [0_u8];
        side.read_exact_at(&mut byte, at).unwrap();
        side.write_all_at(&[byte[0] ^ 0x10], at).unwrap();
        drop(side);
        let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
        assert_eq!(shadows.open(Path::new(PATH), OpenMode::ReadWrite).err().map(|e| e.code), Some(Code::Corrupt), "{at}");
        assert_eq!(content(&*disk), b"kept", "{at}");
    }
}

/// Verifies: [DESIGN] RULES.md D30, D471, runs: a side file cut in the
/// middle of a granule of a run, as a crash leaves an append, has no
/// commit block: the file opens as its last commit left it, the side file
/// thrown away.
#[test]
fn a_side_file_cut_mid_granule_loses_the_uncommitted() {
    let disk = disk();
    committed_runs(&disk, b"kept");
    let side = disk.open(&side_path(Path::new(PATH), 1), OpenMode::ReadWrite).unwrap();
    side.set_size(FIRST_PAYLOAD + 9 + u64::try_from(GRANULE).unwrap() + 100).unwrap();
    drop(side);
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    let file = shadows.open(Path::new(PATH), OpenMode::ReadWrite).unwrap();
    assert_eq!(file.size().unwrap(), 4);
    assert_eq!(content(&*disk), b"kept");
    assert!(!disk.exists(&side_path(Path::new(PATH), 1)).unwrap(), "thrown away");
}

/// Verifies: [DESIGN] RULES.md D471, blocks read and written over by
/// their reader: a read past the appended blocks and a block written over
/// with a payload of another length are refused with SQLCODE -910.
#[test]
fn blocks_read_unchecked_stay_within_the_file() {
    let fs = disk();
    let blocks = BlockFile::create(&*fs, Path::new("/d/b"), hash(), Pipeline::none(), BlockOptions::default()).unwrap();
    let id = blocks.append(b"twelve bytes").unwrap();
    let mut out = [0_u8; 6];
    blocks.read_payload_at(id, 6, &mut out).unwrap();
    assert_eq!(&out, b" bytes");
    assert_eq!(blocks.read_payload_at(id, 12, &mut out).err().map(|e| e.code), Some(Code::Api), "past the end");
    assert_eq!(blocks.read_payload_at(BlockId(blocks.end().0), 0, &mut out).err().map(|e| e.code), Some(Code::Api));
    assert_eq!(blocks.overwrite(id, b"eleven byte", 12).err().map(|e| e.code), Some(Code::Api));
    blocks.overwrite(id, b"TWELVE BYTES", 12).unwrap();
    assert_eq!(blocks.read(id).unwrap(), b"TWELVE BYTES");
}

/// Verifies: [DESIGN] RULES.md D472, pages written over with no lock
/// held: two writers write their pages again and again while two readers
/// read every page, and each page read is whole, of one write; a commit
/// after takes each page's newest; each page still takes one block.
#[test]
fn pages_written_over_with_no_lock_read_whole() {
    let disk = disk();
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    let file = shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap();
    let at = |page: usize| u64::try_from(page * PAGE).unwrap();
    for page in 0..4 {
        file.write_all_at(&[1_u8; PAGE], at(page)).unwrap();
    }
    let once = shadows.pending_bytes();
    std::thread::scope(|s| {
        for page in 0..2 {
            let file = &file;
            s.spawn(move || (2_u8..200).for_each(|round| file.write_all_at(&[round; PAGE], at(page)).unwrap()));
        }
        for _ in 0..2_u8 {
            let file = &file;
            s.spawn(move || {
                for _ in 0..300_u16 {
                    let mut back = vec![0_u8; 4 * PAGE];
                    file.read_exact_at(&mut back, 0).unwrap();
                    assert!(back.chunks(PAGE).all(|page| page.iter().all(|&b| b == page[0])), "whole pages");
                }
            });
        }
    });
    assert_eq!(shadows.pending_bytes(), once, "written over in place");
    shadows.commit().unwrap();
    assert_eq!(content(&*disk), [vec![199_u8; 2 * PAGE], vec![1_u8; 2 * PAGE]].concat());
}

/// Verifies: [DESIGN] RULES.md D472, granules in flight: a run marked in
/// flight refuses a second marking of any of its granules; a reader of
/// one of them waits until it lands, one of another granule does not.
#[test]
fn a_reader_waits_for_its_granule_in_flight() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let flight = flight::Flight::new();
    assert!(flight.take_off(4..6));
    assert!(!flight.take_off(5..7), "granule 5 is in flight");
    flight.wait_for(9);
    let read = AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            flight.wait_for(5);
            read.store(true, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(!read.load(Ordering::SeqCst), "waiting while it flies");
        flight.land(4..6);
    });
    assert!(read.load(Ordering::SeqCst));
    assert!(flight.take_off(5..7), "landed");
}
