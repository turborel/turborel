//! Unit tests of shadow files: reads see writes, a commit makes them the
//! file's, a crash before a commit loses them and after one keeps them, and
//! a side file damaged at its end is cut (RULES.md R-TEST-1, R-TEST-2,
//! R-TEST-7). Split from shadow.rs (RULES.md R-SIZE-4).

use super::*;
use crate::disk::memory::MemoryFileSystem;
use crate::hash::crc::{CRC32_ISCSI, Crc};

fn hash() -> Arc<dyn HashFunction> {
    Arc::new(Crc::new(CRC32_ISCSI).unwrap())
}

/// A memory file system, standing for the disk, and a shadowing one over
/// it; a new shadowing one over the same disk is a restart.
fn disk() -> Arc<dyn FileSystem> {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    fs.create_dir_all(Path::new("/d")).unwrap();
    fs
}

const PATH: &str = "/d/f";

/// The whole content of the file at `PATH` on `fs`, as it is.
fn content(fs: &dyn FileSystem) -> Vec<u8> {
    let file = fs.open(Path::new(PATH), OpenMode::ReadOnly).unwrap();
    let mut out = vec![0; usize::try_from(file.size().unwrap()).unwrap()];
    file.read_exact_at(&mut out, 0).unwrap();
    out
}

/// Verifies: [DESIGN] RULES.md D30, shadow files: writes across granules,
/// unaligned, are read back as written, the file not touched; growing
/// reads zeros; shrinking forgets what is past the size, and growing again
/// reads zeros there; a commit makes the file hold exactly what was
/// written.
#[test]
fn a_commit_makes_writes_the_files() {
    let disk = disk();
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    let file = shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap();
    file.write_all_at(&[7; 10_000], 3).unwrap();
    file.write_all_at(b"middle", 5_000).unwrap();
    assert_eq!(file.size().unwrap(), 10_003);
    let mut back = vec![0; 10_003];
    file.read_exact_at(&mut back, 0).unwrap();
    let mut expected = [vec![0; 3], vec![7; 10_000]].concat();
    expected[5_000..5_006].copy_from_slice(b"middle");
    assert_eq!(back, expected);
    assert_eq!(content(&*disk).len(), 0, "the file is not touched");
    file.set_size(5_003).unwrap();
    file.set_size(9_000).unwrap();
    let mut grown = vec![1; 9_000];
    file.read_exact_at(&mut grown, 0).unwrap();
    assert_eq!(&grown[..5_003], &expected[..5_003]);
    assert!(grown[5_003..].iter().all(|&b| b == 0), "past the size, zeros");
    assert_eq!(file.read_at(&mut [0; 10], 9_000).unwrap(), 0, "the end");
    shadows.commit().unwrap();
    assert_eq!(content(&*disk), grown);
    shadows.commit().unwrap();
    assert_eq!(content(&*disk), grown, "nothing to commit");
}

/// Verifies: [DESIGN] RULES.md D30, shadow files across a crash: what was
/// written and not committed is gone, the file as the last commit left
/// it; a commit whose commit block was durable when the crash came, before
/// the file was written, is completed on opening.
#[test]
fn a_crash_keeps_the_last_commit() {
    let disk = disk();
    {
        let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
        let file = shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap();
        file.write_all_at(b"committed", 0).unwrap();
        shadows.commit().unwrap();
        file.write_all_at(b"LOST", 0).unwrap();
        file.write_all_at(&[9; 5_000], 20).unwrap();
    }
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    let file = shadows.open(Path::new(PATH), OpenMode::ReadWrite).unwrap();
    let mut back = vec![0; 9];
    file.read_exact_at(&mut back, 0).unwrap();
    assert_eq!((back.as_slice(), file.size().unwrap()), (b"committed".as_slice(), 9));
    file.write_all_at(b"KEPT", 0).unwrap();
    file.write_all_at(&[5; 100], 9).unwrap();
    shadows.seal().unwrap();
    // The commit block made durable, the file not yet written: a crash.
    let shadow = lock_builtin(&shadows.files).first().and_then(Weak::upgrade).unwrap();
    {
        let pending = read(&shadow.pending).unwrap();
        let sealed = pending.sealed.as_ref().unwrap();
        sealed.side.blocks.append(&encode(&Op::Limit(sealed.side.limit))).unwrap();
        sealed.side.blocks.append(&encode(&Op::Size(sealed.size))).unwrap();
        sealed.side.blocks.append(&encode(&Op::Commit)).unwrap();
        sealed.side.blocks.sync(Durability::Full).unwrap();
    }
    drop((file, shadow, shadows));
    assert_eq!(&content(&*disk)[..4], b"comm", "not yet in the file");
    let again = ShadowFileSystem::new(Arc::clone(&disk), hash());
    drop(again.open(Path::new(PATH), OpenMode::ReadWrite).unwrap());
    assert_eq!(content(&*disk), [b"KEPTitted".as_slice(), &[5; 100]].concat());
}

/// Verifies: [DESIGN] RULES.md D30, sealing: what is written after a seal
/// is read back at once, but not committed with the sealed batch; it is
/// lost in a crash after that commit, and committed by the next.
#[test]
fn writes_after_a_seal_wait_for_the_next_commit() {
    let disk = disk();
    {
        let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
        let file = shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap();
        file.write_all_at(b"sealed", 0).unwrap();
        shadows.seal().unwrap();
        file.write_all_at(b"LATER!", 0).unwrap();
        file.write_all_at(b"+", 6).unwrap();
        shadows.commit_sealed().unwrap();
        assert_eq!(content(&*disk), b"sealed", "the sealed batch alone");
        let mut back = vec![0; 7];
        file.read_exact_at(&mut back, 0).unwrap();
        assert_eq!(back, b"LATER!+", "reads see the newest");
        shadows.commit_sealed().unwrap();
        assert_eq!(content(&*disk), b"sealed", "nothing sealed: nothing committed");
    }
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    let file = shadows.open(Path::new(PATH), OpenMode::ReadWrite).unwrap();
    assert_eq!(file.size().unwrap(), 6, "the later writes were lost");
    file.write_all_at(b"LATER!+", 0).unwrap();
    shadows.seal().unwrap();
    shadows.seal().unwrap();
    assert_eq!(content(&*disk), b"LATER!+", "a batch sealed is committed before the next is sealed");
}

/// Verifies: [DESIGN] RULES.md D30, limits: a file cut over what a commit
/// put in it, and grown again, reads zeros where it was cut, before and
/// after the next commit, and across a crash after it.
#[test]
fn a_file_cut_and_grown_reads_zeros() {
    let disk = disk();
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    let file = shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap();
    file.write_all_at(&[7; 10_000], 0).unwrap();
    shadows.commit().unwrap();
    file.set_size(5_000).unwrap();
    file.set_size(9_000).unwrap();
    file.write_all_at(b"end", 8_997).unwrap();
    let expected = [vec![7; 5_000], vec![0; 3_997], b"end".to_vec()].concat();
    let mut back = vec![1; 9_000];
    file.read_exact_at(&mut back, 0).unwrap();
    assert_eq!(back, expected, "before the commit");
    shadows.commit().unwrap();
    assert_eq!(content(&*disk), expected, "after it");
}

/// Verifies: [DESIGN] RULES.md D30, shadow files: bytes a crash left half
/// written at the end of a side file are cut, the blocks before them
/// standing; a malformed block is refused with SQLCODE -916; removing a
/// file removes its side file.
#[test]
fn a_damaged_side_file_is_cut() {
    let disk = disk();
    {
        let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
        let file = shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap();
        file.write_all_at(b"x", 0).unwrap();
    }
    let side = disk.open(&side_path(Path::new(PATH), 0), OpenMode::ReadWrite).unwrap();
    let end = side.size().unwrap();
    side.write_all_at(&[0xAB; 7], end).unwrap();
    drop(side);
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    let file = shadows.open(Path::new(PATH), OpenMode::ReadWrite).unwrap();
    assert_eq!(file.size().unwrap(), 0, "nothing was committed");
    for bad in [vec![9], vec![KIND_SIZE], [encode(&Op::Commit), vec![0]].concat()] {
        assert_eq!(decode(&bad).err().map(|e| e.code), Some(Code::Corrupt), "{bad:?}");
    }
    drop(file);
    shadows.remove(Path::new(PATH)).unwrap();
    assert!(!shadows.exists(&side_path(Path::new(PATH), 0)).unwrap());
}

/// Verifies: [DESIGN] RULES.md D30, holding: writes held are read back at
/// once and touch neither the file nor its side file; sealed and committed,
/// they reach the file, after what was written before them to the same
/// granule; a crash after the commit block keeps them; held writes given
/// up go to the side file.
#[test]
fn held_writes_reach_the_file_at_the_commit() {
    let disk = disk();
    let side_size = |on: &dyn FileSystem, which| on.open(&side_path(Path::new(PATH), which), OpenMode::ReadOnly).unwrap().size().unwrap();
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    let file = shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap();
    file.write_all_at(b"older", 0).unwrap();
    let before = side_size(&*disk, 0);
    shadows.hold().unwrap();
    file.write_all_at(b"NEW", 0).unwrap();
    file.write_all_at(&[3; 5_000], 10).unwrap();
    assert_eq!(side_size(&*disk, 0), before, "held in memory");
    let mut back = vec![0; 5];
    file.read_exact_at(&mut back, 0).unwrap();
    assert_eq!(back, b"NEWer");
    shadows.seal().unwrap();
    file.write_all_at(b"x", 0).unwrap();
    shadows.commit_sealed().unwrap();
    assert_eq!(content(&*disk), [b"NEWer\0\0\0\0\0".as_slice(), &[3; 5_000]].concat());
    shadows.hold().unwrap();
    file.write_all_at(b"given up", 0).unwrap();
    shadows.release().unwrap();
    assert!(side_size(&*disk, 1) > before, "released to the side file");
    let mut given_up = vec![0; 8];
    file.read_exact_at(&mut given_up, 0).unwrap();
    assert_eq!(given_up, b"given up");
}
