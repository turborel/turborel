//! Unit tests: blocks published in order, the appender that publishes up
//! to the blocks written after it publishing them too (RULES.md D741,
//! D796, R-TEST-1).

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

/// Verifies: [DESIGN] RULES.md D796: an appender publishing its block
/// publishes the blocks filed after it that follow on from it, all at
/// once, and leaves one filed after a gap until the gap is published.
#[test]
fn a_publish_publishes_the_filed_blocks_that_follow_it() {
    let blocks = blocks();
    let ranges: Vec<(u64, u64)> = (0_u8..4).map(|_| blocks.reserve(8).map(|s| (s, s + 8)).unwrap()).collect();
    let [(a, a_end), (b, b_end), (c, c_end), (d, d_end)] = ranges[..] else { panic!("four ranges") };
    for (start, end) in [(b, b_end), (d, d_end)] {
        lock_builtin(&blocks.parking).insert(start, (end, thread::current()));
        blocks.parked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    blocks.publish(a, a_end, Ok(())).unwrap();
    assert_eq!((blocks.end().0, parked(&blocks)), (b_end, vec![d]), "the second with it, the fourth after a gap");
    blocks.publish(c, c_end, Ok(())).unwrap();
    assert_eq!((blocks.end().0, parked(&blocks)), (d_end, vec![]), "then the fourth");
    assert_eq!(blocks.parked.load(std::sync::atomic::Ordering::SeqCst), 0);
}

/// Verifies: [DESIGN] RULES.md D30, D796: appenders whose writes finish
/// in the reverse of their order each file their block and wait, and the
/// first's publish publishes them all, the end reaching the last.
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
