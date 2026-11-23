//! Writes in and out of order, reads over the file, the buffer's bounds
//! and a failed write (RULES.md D921, R-TEST-1).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use crate::disk::memory::MemoryFileSystem;
use crate::error::Code;

/// A file that counts the writes it takes, and refuses them once told to.
struct Counting {
    file: Box<dyn DiskFile>,
    writes: Arc<AtomicUsize>,
    refusing: Arc<AtomicBool>,
}

impl DiskFile for Counting {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize> {
        self.file.read_at(buf, offset)
    }
    fn write_all_at(&self, data: &[u8], offset: u64) -> Result<()> {
        if self.refusing.load(Ordering::SeqCst) {
            return Err(SqlError::new(Code::Io, "[DESIGN] the test's file takes no write"));
        }
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.file.write_all_at(data, offset)
    }
    fn size(&self) -> Result<u64> {
        self.file.size()
    }
    fn set_size(&self, size: u64) -> Result<()> {
        self.file.set_size(size)
    }
    fn sync(&self, durability: Durability) -> Result<()> {
        self.file.sync(durability)
    }
}

/// A buffered file of `limit` bytes over a file in memory, the same file
/// opened beside it, the count of the writes that reach it, and the switch
/// that has it refuse them.
fn buffered(limit: usize) -> (Buffered, Box<dyn DiskFile>, Arc<AtomicUsize>, Arc<AtomicBool>) {
    let fs = MemoryFileSystem::new();
    let path = Path::new("segment");
    let file = fs.open(path, OpenMode::CreateNew).unwrap();
    let (writes, refusing) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
    let counting = Counting { file, writes: Arc::clone(&writes), refusing: Arc::clone(&refusing) };
    (Buffered::over(Box::new(counting), limit), fs.open(path, OpenMode::ReadWrite).unwrap(), writes, refusing)
}

/// The bytes of `file` from `offset`, `len` of them or as many as it has.
fn bytes_of(file: &dyn DiskFile, offset: u64, len: usize) -> Vec<u8> {
    let mut out = vec![0xEE; len];
    let read = file.read_at(&mut out, offset).unwrap();
    out.truncate(read);
    out
}

/// Verifies: [DESIGN] RULES.md D921, writes that follow one another are
/// held in memory, read back at once though the file holds none, and
/// reach the file in one write at the sync.
#[test]
fn appended_writes_reach_the_file_in_one_write_at_a_sync() {
    let (file, beside, writes, _) = buffered(64);
    file.write_all_at(b"abcd", 10).unwrap();
    file.write_all_at(b"efgh", 14).unwrap();
    file.write_all_at(b"ij", 18).unwrap();
    assert_eq!(writes.load(Ordering::SeqCst), 0, "held, not written");
    assert_eq!(beside.size().unwrap(), 0);
    assert_eq!(file.size().unwrap(), 20, "its size counts what it holds");
    assert_eq!(bytes_of(&file, 12, 6), b"cdefgh");
    file.sync(Durability::Data).unwrap();
    assert_eq!(writes.load(Ordering::SeqCst), 1, "one write for the three");
    assert_eq!(bytes_of(&*beside, 10, 10), b"abcdefghij");
    file.sync(Durability::Data).unwrap();
    assert_eq!(writes.load(Ordering::SeqCst), 1, "nothing held, nothing written");
}

/// Verifies: [DESIGN] RULES.md D921, a write that arrives before the one
/// reserved behind it shares the buffer with it; the bytes between read
/// as zeros until they are written, as the file's would.
#[test]
fn writes_out_of_order_share_the_buffer() {
    let (file, beside, writes, _) = buffered(64);
    file.write_all_at(b"aa", 0).unwrap();
    file.write_all_at(b"cc", 4).unwrap();
    assert_eq!(bytes_of(&file, 0, 6), [b'a', b'a', 0, 0, b'c', b'c']);
    file.write_all_at(b"bb", 2).unwrap();
    assert_eq!(bytes_of(&file, 0, 6), b"aabbcc");
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    file.sync(Durability::None).unwrap();
    assert_eq!((writes.load(Ordering::SeqCst), bytes_of(&*beside, 0, 6)), (1, b"aabbcc".to_vec()));
}

/// Verifies: [DESIGN] RULES.md D921, the buffer full, what it holds is
/// written, in one write; a write as long as the buffer goes to the file.
#[test]
fn a_full_buffer_is_written() {
    let (file, beside, writes, _) = buffered(8);
    file.write_all_at(b"abcd", 0).unwrap();
    file.write_all_at(b"efgh", 4).unwrap();
    assert_eq!((writes.load(Ordering::SeqCst), bytes_of(&*beside, 0, 8)), (1, b"abcdefgh".to_vec()), "full: written");
    file.write_all_at(b"ijklm", 8).unwrap();
    assert_eq!((writes.load(Ordering::SeqCst), beside.size().unwrap()), (1, 8), "held");
    file.write_all_at(b"nopqr", 13).unwrap();
    assert_eq!((writes.load(Ordering::SeqCst), beside.size().unwrap()), (2, 18), "full again: both in one write");
    file.write_all_at(b"0123456789", 18).unwrap();
    assert_eq!(writes.load(Ordering::SeqCst), 3, "one as long as the buffer goes to the file");
    assert_eq!(bytes_of(&*beside, 0, 28), b"abcdefghijklmnopqr0123456789");
}

/// Verifies: [DESIGN] RULES.md D921, writes apart are held apart, nothing
/// held for the bytes between them, which the file keeps; a write as long
/// as the buffer, as zeros written ahead, goes to the file and leaves what
/// is held; a write over bytes held has them written first, a write a
/// run, so the file takes both in the order written.
#[test]
fn writes_apart_are_held_apart() {
    let (file, beside, writes, _) = buffered(8);
    beside.write_all_at(b"0123456789abcdef", 0).unwrap();
    file.write_all_at(b"CD", 10).unwrap();
    file.write_all_at(b"AB", 4).unwrap();
    assert_eq!(writes.load(Ordering::SeqCst), 0, "both held");
    assert_eq!(bytes_of(&file, 2, 12), b"23AB6789CDcd", "the file's bytes between them read");
    file.write_all_at(&[0; 16], 40).unwrap();
    assert_eq!(writes.load(Ordering::SeqCst), 1, "as long as the buffer: written, what is held kept");
    assert_eq!((bytes_of(&file, 10, 2), file.size().unwrap()), (b"CD".to_vec(), 56));
    file.write_all_at(b"XYZ", 9).unwrap();
    assert_eq!(writes.load(Ordering::SeqCst), 3, "over bytes held: the two runs written first");
    assert_eq!((bytes_of(&*beside, 9, 3), bytes_of(&file, 9, 3)), (b"9CD".to_vec(), b"XYZ".to_vec()));
    file.sync(Durability::None).unwrap();
    assert_eq!((writes.load(Ordering::SeqCst), bytes_of(&*beside, 2, 12)), (4, b"23AB678XYZcd".to_vec()));
}

/// Verifies: [DESIGN] RULES.md D921, differential: the same writes, cuts
/// and syncs made through a buffer and directly leave the same file, and
/// every read between them gives the same bytes and the same count.
#[test]
fn a_buffered_file_reads_as_one_written_directly() {
    for limit in [1_usize, 7, 16, 64, 1024] {
        let (file, _, _, _) = buffered(limit);
        let fs = MemoryFileSystem::new();
        let direct = fs.open(Path::new("direct"), OpenMode::CreateNew).unwrap();
        // A fixed sequence (R-TEST-5): a linear congruential step.
        let mut state = 0x9E37_79B9_u64;
        let mut next = |bound: u64| {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (state >> 33_u32) % bound
        };
        let mut end = 0_u64;
        for step in 0..400_u64 {
            let len = usize::try_from(next(24)).unwrap() + 1;
            let data: Vec<u8> = (0..len).map(|i| u8::try_from((step + u64::try_from(i).unwrap()) % 251).unwrap() + 1).collect();
            // Mostly appends; some writes back over what was written, some ahead.
            let offset = match next(10) {
                0 => next(end + 1),
                1 => end + next(40),
                _ => end,
            };
            file.write_all_at(&data, offset).unwrap();
            direct.write_all_at(&data, offset).unwrap();
            end = end.max(offset + u64::try_from(len).unwrap());
            match next(12) {
                0 => {
                    file.sync(Durability::Data).unwrap();
                }
                1 => {
                    let size = next(end + 8);
                    file.set_size(size).unwrap();
                    direct.set_size(size).unwrap();
                    end = size;
                }
                _ => {}
            }
            let (at, want) = (next(end + 4), usize::try_from(next(48)).unwrap());
            assert_eq!(bytes_of(&file, at, want), bytes_of(&*direct, at, want), "limit {limit} step {step} read {at}+{want}");
            assert_eq!(file.size().unwrap(), direct.size().unwrap(), "limit {limit} step {step}");
        }
        file.sync(Durability::None).unwrap();
        let size = usize::try_from(direct.size().unwrap()).unwrap();
        assert_eq!(bytes_of(&*file.file, 0, size + 1), bytes_of(&*direct, 0, size + 1), "limit {limit}: the files");
    }
}

/// Verifies: [DESIGN] RULES.md D921, a write of what is held that fails is
/// kept: the sync fails, and every write, cut and sync after it, though
/// the file would take them again; what was held is still read.
#[test]
fn a_failed_write_of_the_buffer_fails_every_later_one() {
    let (file, _, writes, refusing) = buffered(64);
    file.write_all_at(b"abcd", 0).unwrap();
    refusing.store(true, Ordering::SeqCst);
    assert_eq!(file.sync(Durability::Data).err().map(|e| e.code), Some(Code::Io));
    refusing.store(false, Ordering::SeqCst);
    assert_eq!(file.write_all_at(b"efgh", 4).err().map(|e| e.code), Some(Code::Io));
    assert_eq!(file.sync(Durability::None).err().map(|e| e.code), Some(Code::Io));
    assert_eq!(file.set_size(0).err().map(|e| e.code), Some(Code::Io));
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert_eq!(bytes_of(&file, 0, 4), b"abcd");
}

/// Verifies: [DESIGN] RULES.md D921, a file dropped writes what it holds;
/// a cut writes it first, then cuts the file as written.
#[test]
fn a_drop_and_a_cut_write_what_is_held() {
    let (file, beside, writes, _) = buffered(64);
    file.write_all_at(b"abcdef", 0).unwrap();
    file.set_size(4).unwrap();
    assert_eq!((writes.load(Ordering::SeqCst), bytes_of(&*beside, 0, 8)), (1, b"abcd".to_vec()));
    file.write_all_at(b"gh", 4).unwrap();
    assert_eq!(beside.size().unwrap(), 4);
    drop(file);
    assert_eq!(bytes_of(&*beside, 0, 8), b"abcdgh");
}

/// Verifies: [DESIGN] RULES.md D921, what is held is late once its oldest
/// record has been held `WRITE_AFTER`, and not before; nothing held is
/// never late.
#[test]
fn what_is_held_is_late_after_the_bound() {
    let since = Instant::now();
    assert!(!late(Some(since), since));
    assert!(!late(Some(since), (since + WRITE_AFTER).checked_sub(Duration::from_millis(1)).unwrap()));
    assert!(late(Some(since), since + WRITE_AFTER));
    assert!(!late(None, since + WRITE_AFTER));
}

/// Verifies: [DESIGN] RULES.md D921, a file system that buffers wraps the
/// files it opens for writing, and none when it buffers no bytes, nor one
/// opened for reading alone.
#[test]
fn only_files_written_are_buffered() {
    let fs = MemoryFileSystem::new();
    let buffering = Buffering { fs: &fs, bytes: 64 };
    let made = buffering.open(Path::new("a"), OpenMode::CreateNew).unwrap();
    made.write_all_at(b"abcd", 0).unwrap();
    let read_only = buffering.open(Path::new("a"), OpenMode::ReadOnly).unwrap();
    assert_eq!(read_only.size().unwrap(), 0, "held by the file written, not in the file");
    made.sync(Durability::None).unwrap();
    assert_eq!(read_only.size().unwrap(), 4);
    let direct = Buffering { fs: &fs, bytes: 0 }.open(Path::new("b"), OpenMode::CreateNew).unwrap();
    direct.write_all_at(b"abcd", 0).unwrap();
    assert_eq!(fs.open(Path::new("b"), OpenMode::ReadOnly).unwrap().size().unwrap(), 4, "no buffer: written at once");
}
