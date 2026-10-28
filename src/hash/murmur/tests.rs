//! Unit tests of MurmurHash3: SMHasher's verification value, digests of
//! pieces, and the hasher's spread over keys like a buffer pool's (RULES.md
//! R-TEST-1, R-TEST-2). Split from murmur.rs (RULES.md R-SIZE-4).

use std::collections::HashSet;
use std::hash::{BuildHasher, Hash};

use super::*;

/// Verifies: [DESIGN] RULES.md D30, MurmurHash3 x64 128-bit agrees with
/// the reference implementation: SMHasher's verification (`KeysetTest.cpp`,
/// `VerificationTest`) hashes the keys 0, 0 1, ..., 0 1 ... 254, the key of
/// `i` bytes with seed 256 - i, then the 256 digests together with seed 0;
/// the first four bytes of the result, little-endian, are 0x6384BA69
/// (`main.cpp`, "Murmur3F").
#[test]
fn murmur3_matches_the_reference() {
    let key: Vec<u8> = (0..=u8::MAX).collect();
    let mut digests = Vec::new();
    for (i, seed) in (0..256_usize).zip((1..=256_u32).rev()) {
        let (h1, h2) = murmur3_x64_128(&key[..i], seed);
        digests.extend_from_slice(&h1.to_le_bytes());
        digests.extend_from_slice(&h2.to_le_bytes());
    }
    let (h1, _) = murmur3_x64_128(&digests, 0);
    assert_eq!(h1 & 0xFFFF_FFFF, 0x6384_BA69);
}

/// Verifies: [DESIGN] RULES.md D30, `HashFunction` contract: the digest of
/// parts is that of their concatenation, wherever they are cut; it holds
/// the two halves little-endian; its id and length are fixed.
#[test]
fn pieces_hash_as_the_whole() {
    let data: Vec<u8> = (0..100_u8).collect();
    let whole = Murmur3.hash(&data);
    for cut in 0..data.len() {
        for second in cut..data.len() {
            let parts = [&data[..cut], &data[cut..second], &data[second..]];
            assert_eq!(Murmur3.hash_parts(&parts), whole, "cut at {cut} and {second}");
        }
    }
    let (h1, h2) = murmur3_x64_128(&data, 0);
    assert_eq!(whole.as_bytes(), [h1.to_le_bytes(), h2.to_le_bytes()].concat());
    assert_eq!((Murmur3.id(), Murmur3.digest_len(), Murmur3.name()), (HashId(10), 16, "MurmurHash3_x64_128"));
    assert_eq!(fmix64(0), 0, "the finalizer of 0 is 0");
}

/// A buffer pool's key: a file and a page.
#[derive(Hash)]
struct Page(u32, u64);

/// Verifies: [DESIGN] RULES.md D30, the hasher for hash tables: 100,000
/// pages of four files, numbered in sequence, all hash differently, and
/// fill 1,024 buckets by their low bits as evenly as chance would (a
/// chi-squared within five standard deviations of its mean); the
/// same key hashes the same, another seed differently; every width of
/// integer and bytes are taken.
#[test]
fn the_hasher_spreads_sequential_keys() {
    let build = MurmurBuildHasher::default();
    let hashes: Vec<u64> = (0..100_000_u64).map(|n| build.hash_one(Page(u32::try_from(n % 4).unwrap(), n >> 2_u32))).collect();
    assert_eq!(hashes.iter().collect::<HashSet<_>>().len(), hashes.len());
    let mut buckets = [0_u32; 1_024];
    for h in &hashes {
        buckets[usize::try_from(h % 1_024).unwrap()] += 1;
    }
    // Chi-squared over 1,023 degrees of freedom: mean 1,023, standard
    // deviation about 45 for a uniform spread.
    let mean = 100_000.0_f64 / 1_024.0_f64;
    let chi: f64 = buckets.iter().map(|&b| (f64::from(b) - mean).powi(2) / mean).sum();
    assert!(chi < 1_023.0_f64 + 5.0_f64 * 45.0_f64, "chi-squared {chi}");
    assert_eq!(build.hash_one(Page(1, 2)), build.hash_one(Page(1, 2)));
    assert_ne!(build.hash_one(Page(1, 2)), MurmurBuildHasher::with_seed(7).hash_one(Page(1, 2)));
    let one = |value: &dyn Fn(&mut MurmurHasher)| {
        let mut state = build.build_hasher();
        value(&mut state);
        state.finish()
    };
    let widths = [
        one(&|h| 1_u8.hash(h)),
        one(&|h| 1_u16.hash(h)),
        one(&|h| 1_u32.hash(h)),
        one(&|h| 1_u64.hash(h)),
        one(&|h| 1_u128.hash(h)),
        one(&|h| 1_usize.hash(h)),
        one(&|h| h.write(&[1, 0, 0, 0, 0, 0, 0, 0, 2])),
    ];
    assert_eq!(widths.iter().collect::<HashSet<_>>().len(), 6, "u64 and usize alike; the others by their length: {widths:?}");
}
