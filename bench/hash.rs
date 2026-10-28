//! How fast the built-in hash functions are (RULES.md D30): for every CRC,
//! slicing-by-16 against the byte-at-a-time reference, on inputs the size
//! of a small record, a page and a large block. Run with `cargo xtask
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

use std::hint::black_box;

use harness::{Bench, Case};
use turborel::hash::crc::{CATALOGUE, Crc};

/// Input sizes: a small record, a page, a large block.
const SIZES: [usize; 3] = [64, 8 * 1024, 1024 * 1024];
/// Bytes hashed per run with slicing, and with the reference.
const SLICED_BYTES: usize = 128 * 1024 * 1024;
const BYTEWISE_BYTES: usize = 16 * 1024 * 1024;

fn main() {
    let bench = Bench::new("hash");
    let largest = SIZES[SIZES.len() - 1];
    let data: Vec<u8> = (0..largest).map(|i| (i % 251) as u8).collect();
    for params in CATALOGUE {
        let crc = Crc::new(params).unwrap();
        for size in SIZES {
            let input = &data[..size];
            bench.measure(Case::new(format!("{}, slicing-by-16", params.name), "hashes").bytes(size), || {
                hash(input, SLICED_BYTES, |d| crc.update(crc.start(), d))
            });
            bench.measure(Case::new(format!("{}, byte at a time", params.name), "hashes").bytes(size), || {
                hash(input, BYTEWISE_BYTES, |d| crc.update_bytewise(crc.start(), d))
            });
        }
    }
}

/// Hashes `input` with `f` until `total` bytes are hashed; the hashes made.
fn hash(input: &[u8], total: usize, f: impl Fn(&[u8]) -> u64) -> u64 {
    let rounds = total / input.len();
    for _ in 0..rounds {
        black_box(f(black_box(input)));
    }
    rounds as u64
}
