//! How fast the block manager appends and reads (RULES.md D30): appends of
//! several block sizes on one and on several threads, a cursor reading
//! them back, and appends each followed by a sync. Run with `cargo xtask
//! bench`, on the harness (RULES.md R-BENCH-1).

#![allow(
    clippy::unwrap_used,
    clippy::as_conversions,
    clippy::arithmetic_side_effects,
    clippy::integer_division,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    missing_docs,
    reason = "benchmark code: a failure ends the run (RULES.md R-DEV-3)"
)]

mod harness;

use std::path::Path;

use harness::{Bench, Case, crc, on_threads};
use turborel::disk::block::{BlockFile, BlockOptions};
use turborel::disk::encoding::Pipeline;
use turborel::disk::io::{Durability, OsFileSystem};

/// Payload sizes: a small record, a page, a large block.
const SIZES: [usize; 3] = [64, 4096, 64 * 1024];
/// Payload bytes appended per run.
const TOTAL_BYTES: usize = 32 * 1024 * 1024;
/// Thread counts for appending.
const THREADS: [usize; 2] = [1, 4];
/// Appends each followed by a sync, per run.
const SYNCED_APPENDS: u64 = 100;

fn main() {
    let bench = Bench::new("blocks");
    for size in SIZES {
        let file = format!("append-{size}");
        for threads in THREADS {
            let each = TOTAL_BYTES / size / threads;
            bench.measure_fresh(
                Case::new("append", "blocks").threads(threads).bytes(size),
                || create(&bench.fresh(&file)),
                |blocks| {
                    on_threads(threads, |_| append(&blocks, size, each));
                    (each * threads) as u64
                },
            );
        }
        // Opening checks the whole file, so it is done once, untimed.
        let (blocks, _) = BlockFile::open(&OsFileSystem, &bench.path(&file), &[crc()], &[], BlockOptions::default()).unwrap();
        bench.measure(Case::new("read with a cursor", "blocks").bytes(size), || read_all(&blocks));
    }
    for durability in [Durability::Data, Durability::Full] {
        let payload = vec![1_u8; SIZES[0]];
        bench.measure_fresh(
            Case::new(format!("append + sync {durability:?}"), "blocks").bytes(SIZES[0]),
            || create(&bench.fresh("synced")),
            |blocks| {
                for _ in 0..SYNCED_APPENDS {
                    blocks.append(&payload).unwrap();
                    blocks.sync(durability).unwrap();
                }
                SYNCED_APPENDS
            },
        );
    }
}

/// A new block file at `path`.
fn create(path: &Path) -> BlockFile {
    BlockFile::create(&OsFileSystem, path, crc(), Pipeline::none(), BlockOptions::default()).unwrap()
}

/// Appends `count` blocks of `size` bytes to `blocks`.
fn append(blocks: &BlockFile, size: usize, count: usize) {
    let payload = vec![7_u8; size];
    for _ in 0..count {
        blocks.append(&payload).unwrap();
    }
}

/// Reads every block of `file` with a cursor; the blocks read.
fn read_all(file: &BlockFile) -> u64 {
    let mut cursor = file.cursor(file.first());
    let mut payload = Vec::new();
    let mut blocks = 0;
    while cursor.next(&mut payload).unwrap().is_some() {
        blocks += 1;
    }
    blocks
}
