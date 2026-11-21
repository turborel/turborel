//! Unit tests: blocks published in order, each waiting appender woken
//! alone as its turn comes (RULES.md D741, R-TEST-1).

use std::path::Path;
use std::sync::Arc;
use std::thread;

use super::{BlockFile, BlockOptions};
use crate::disk::encoding::Pipeline;
use crate::disk::io::FileSystem;
use crate::disk::memory::MemoryFileSystem;
use crate::hash::HashFunction;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::sync::lock_builtin;

/// A block file in a memory file system.
fn blocks() -> BlockFile {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    fs.create_dir_all(Path::new("/d")).unwrap();
    let hash: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    BlockFile::create(&*fs, Path::new("/d/b"), hash, Pipeline::none(), BlockOptions::default()).unwrap()
}

/// The offsets parked appenders wait under.
fn parked(blocks: &BlockFile) -> Vec<u64> {
    lock_builtin(&blocks.parking).keys().copied().collect()
}

/// Verifies: [DESIGN] RULES.md D741: an appender publishing its block
/// wakes only the appender parked under the offset its block ends at, the
/// next in line; the one after stays parked until that one publishes.
#[test]
fn a_publish_wakes_only_the_next_appender() {
    let blocks = blocks();
    let ranges: Vec<(u64, u64)> = (0_u8..3).map(|_| blocks.reserve(8).map(|s| (s, s + 8)).unwrap()).collect();
    for &(start, _) in &ranges[1..] {
        lock_builtin(&blocks.parking).insert(start, thread::current());
        blocks.parked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    let [(a, a_end), (b, b_end), (c, c_end)] = ranges[..] else { panic!("three ranges") };
    blocks.publish(a, a_end, Ok(())).unwrap();
    assert_eq!(parked(&blocks), [c], "the second woken alone");
    blocks.publish(b, b_end, Ok(())).unwrap();
    assert_eq!(parked(&blocks), Vec::<u64>::new(), "then the third");
    blocks.publish(c, c_end, Ok(())).unwrap();
    assert_eq!((blocks.end().0, blocks.parked.load(std::sync::atomic::Ordering::SeqCst)), (c_end, 0));
}

/// Verifies: [DESIGN] RULES.md D30, D741: appenders whose writes finish
/// in the reverse of their order each wait for those before, parked, and
/// all are published, the end reaching the last.
#[test]
fn appenders_finishing_out_of_order_are_all_published() {
    let blocks = blocks();
    let ranges: Vec<(u64, u64)> = (0_u8..6).map(|_| blocks.reserve(8).map(|s| (s, s + 8)).unwrap()).collect();
    thread::scope(|scope| {
        for &(start, end) in ranges.iter().skip(1).rev() {
            let blocks = &blocks;
            scope.spawn(move || blocks.publish(start, end, Ok(())).unwrap());
        }
        while blocks.parked.load(std::sync::atomic::Ordering::SeqCst) < ranges.len() - 1 {
            thread::yield_now();
        }
        let (start, end) = ranges[0];
        blocks.publish(start, end, Ok(())).unwrap();
    });
    assert_eq!(blocks.end().0, ranges[5].1);
    assert_eq!(parked(&blocks), Vec::<u64>::new());
}
