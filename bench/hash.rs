//! How fast the built-in hash functions are (RULES.md D30, D460): for every
//! CRC, slicing-by-16 against the byte-at-a-time reference, on inputs the
//! size of a small record, a page and a large block; XXH3, XXH64 and
//! MurmurHash3 from a word to a large block; and the hashers of the
//! engine's maps on the keys they hold. Run with `cargo xtask bench`, on
//! the harness (RULES.md R-BENCH-1).

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

use std::hash::BuildHasher;

use harness::{Bench, Case};
use turborel::hash::crc::{CATALOGUE, Crc};
use turborel::hash::murmur::{MurmurBuildHasher, murmur3_x64_128};
use turborel::hash::xxhash::{Words, Xxh3BuildHasher, xxh3_64, xxh3_64_with_seed, xxh3_128, xxh64};

/// Input sizes of the non-cryptographic functions: a word, a short key, a
/// row's text, a short record, a page and a large block.
const KEY_SIZES: [usize; 7] = [8, 16, 40, 64, 256, 8 * 1024, 1024 * 1024];
/// Bytes hashed per run by them.
const KEY_BYTES: usize = 64 * 1024 * 1024;
/// Keys hashed per run by a map's hasher.
const MAP_KEYS: u64 = 16 * 1024 * 1024;

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
    for size in KEY_SIZES {
        let input = &data[..size];
        let case = |name: &str| Case::new(name.to_string(), "hashes").bytes(size);
        bench.measure(case("XXH3-64"), || hash(input, KEY_BYTES, xxh3_64));
        bench.measure(case("XXH3-64, seeded"), || hash(input, KEY_BYTES, |d| xxh3_64_with_seed(d, 0x5EED)));
        bench.measure(case("XXH3-128"), || hash(input, KEY_BYTES, |d| xxh3_128(d) as u64));
        bench.measure(case("XXH64"), || hash(input, KEY_BYTES, |d| xxh64(d, 0)));
        bench.measure(case("MurmurHash3 x64 128"), || hash(input, KEY_BYTES, |d| murmur3_x64_128(d, 0).0));
    }
    maps(&bench);
}

/// The hashers of the engine's maps, on a buffer pool's key of two words
/// and on a name of 24 bytes, and a word by `Words` against `fmix64`.
fn maps(bench: &Bench) {
    let (murmur, xxh3) = (MurmurBuildHasher::default(), Xxh3BuildHasher::default());
    let words = Words::new(0x5EED);
    let (short, name) = ("ORDER_LINE", "PUBLIC.ORDER_LINE_ITEMS_");
    let keys = |label: &str| Case::new(label.to_string(), "keys");
    let wide = |n: u64| -> [u8; 64] { std::array::from_fn(|i| (n as u8).wrapping_add(i as u8)) };
    bench.measure(keys("map key u64, MurmurHasher"), || each(|n| murmur.hash_one(n)));
    bench.measure(keys("map key u64, Xxh3Hasher"), || each(|n| xxh3.hash_one(n)));
    bench.measure(keys("map key (u64, u64), MurmurHasher"), || each(|n| murmur.hash_one((n, n ^ 7))));
    bench.measure(keys("map key (u64, u64), Xxh3Hasher"), || each(|n| xxh3.hash_one((n, n ^ 7))));
    bench.measure(keys("map key short name, MurmurHasher"), || each(|n| murmur.hash_one(&short[..10 - (n & 1) as usize])));
    bench.measure(keys("map key short name, Xxh3Hasher"), || each(|n| xxh3.hash_one(&short[..10 - (n & 1) as usize])));
    bench.measure(keys("map key (name, u64), MurmurHasher"), || each(|n| murmur.hash_one((name, n))));
    bench.measure(keys("map key (name, u64), Xxh3Hasher"), || each(|n| xxh3.hash_one((name, n))));
    bench.measure(keys("map key 64 bytes, MurmurHasher"), || each(|n| murmur.hash_one(wide(n))));
    bench.measure(keys("map key 64 bytes, Xxh3Hasher"), || each(|n| xxh3.hash_one(wide(n))));
    bench.measure(keys("word, fmix64"), || each(|n| turborel::hash::murmur::fmix64(0x5EED ^ n)));
    bench.measure(keys("word, XXH3 Words"), || each(|n| words.hash(n)));
}

/// Hashes `MAP_KEYS` keys made of their numbers by `f`; the keys hashed.
fn each(f: impl Fn(u64) -> u64) -> u64 {
    for n in 0..MAP_KEYS {
        black_box(f(black_box(n)));
    }
    MAP_KEYS
}

/// Hashes `input` with `f` until `total` bytes are hashed; the hashes made.
fn hash(input: &[u8], total: usize, f: impl Fn(&[u8]) -> u64) -> u64 {
    let rounds = total / input.len();
    for _ in 0..rounds {
        black_box(f(black_box(input)));
    }
    rounds as u64
}
