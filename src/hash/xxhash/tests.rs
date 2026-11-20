//! Unit tests of xxHash: the reference's published sanity vectors
//! (`sanity.vectors`, made from its `tests/sanity_test_vectors.h`), every
//! split of a streamed input against the input at once, and the hash
//! functions' contract (RULES.md R-TEST-1, R-TEST-2).

use super::*;
use crate::hash::crc::{CRC32_ISO_HDLC, Crc};

/// The published vectors: a subset whole, and a fold of every one.
const VECTORS: &str = include_str!("sanity.vectors");

/// The reference's `SANITY_BUFFER_SIZE`.
const SANITY_BUFFER_SIZE: usize = 4096 + 64 + 1;
/// Where the sanity tests' secret starts in the buffer, and its length.
const SECRET_AT: usize = 7;
const SECRET_LEN: usize = SECRET_SIZE_MIN + 11;

/// The reference's `fillTestBuffer`: each byte the top of a product of
/// powers of 2654435761 and 11400714785074694797.
fn sanity_buffer() -> Vec<u8> {
    let mut generator: u64 = 2_654_435_761;
    (0..SANITY_BUFFER_SIZE)
        .map(|_| {
            let byte = u8::try_from(generator >> 56_u32).unwrap();
            generator = generator.wrapping_mul(11_400_714_785_074_694_797);
            byte
        })
        .collect()
}

/// The result of `table`'s function of `input` and `seed`, as the table
/// writes it: a 64-bit value, or a 128-bit one's low and high halves.
fn computed(table: &str, input: &[u8], seed: u64, secret: Secret<'_>) -> Vec<u64> {
    let halves = |v: u128| vec![u64::try_from(v & u128::from(u64::MAX)).unwrap(), u64::try_from(v >> 64_u32).unwrap()];
    match table {
        "XXH64" => vec![xxh64(input, seed)],
        "XXH3" => vec![xxh3_64_with_seed(input, seed)],
        "XXH3S" => vec![xxh3_64_with_secret(input, secret)],
        "XXH128" => halves(xxh3_128_with_seed(input, seed)),
        "XXH128S" => halves(xxh3_128_with_secret(input, secret)),
        other => panic!("no table {other}"),
    }
}

fn hex(word: &str) -> u64 {
    u64::from_str_radix(word, 16).unwrap()
}

/// Verifies: [DESIGN] RULES.md D30, D460: XXH64, XXH3-64 and XXH3-128,
/// seeded and of a secret, give the reference's published sanity vectors
/// at every length boundary of their paths (0 to 20, around 32, 64, 128,
/// 240, a stripe, a block and the buffer's end).
#[test]
fn published_vectors_at_the_boundaries() {
    let buffer = sanity_buffer();
    let secret = Secret::new(&buffer[SECRET_AT..SECRET_AT + SECRET_LEN]).unwrap();
    let mut checked = 0_usize;
    for line in VECTORS.lines().filter(|l| !l.starts_with('#') && !l.starts_with("fold")) {
        let fields: Vec<&str> = line.split(' ').collect();
        let len: usize = fields[1].parse().unwrap();
        let expected: Vec<u64> = fields[3..].iter().map(|w| hex(w)).collect();
        assert_eq!(computed(fields[0], &buffer[..len], hex(fields[2]), secret), expected, "{line}");
        checked += 1_usize;
    }
    assert!(checked > 700_usize, "{checked} vectors");
}

/// Verifies: [DESIGN] RULES.md D30, D460: every one of the reference's
/// published sanity vectors, of every table, length and seed: the CRC-32
/// of the results in the table's order equals that of the published ones.
#[test]
fn every_published_vector_folds_alike() {
    let buffer = sanity_buffer();
    let secret = Secret::new(&buffer[SECRET_AT..SECRET_AT + SECRET_LEN]).unwrap();
    let crc = Crc::new(CRC32_ISO_HDLC).unwrap();
    let folds: Vec<&str> = VECTORS.lines().filter(|l| l.starts_with("fold")).collect();
    assert_eq!(folds.len(), 5);
    for line in folds {
        let fields: Vec<&str> = line.split(' ').collect();
        let (table, entries, expected) = (fields[1], fields[2].parse::<usize>().unwrap(), hex(fields[3]));
        let seeds: Vec<u64> = fields[4..].iter().map(|w| hex(w)).collect();
        let mut bytes = Vec::new();
        for n in 0..entries {
            let (len, seed) = (n.div_euclid(seeds.len()), seeds[n % seeds.len()]);
            for value in computed(table, &buffer[..len], seed, secret) {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        assert_eq!(crc.checksum(&bytes), expected, "{table}");
    }
}

/// Lengths whose every split is streamed: each path's ends, a stripe's,
/// the buffer's, a block's.
const SPLIT_LENGTHS: [usize; 24] =
    [0, 1, 3, 4, 8, 9, 16, 17, 128, 129, 240, 241, 255, 256, 257, 320, 511, 512, 513, 1023, 1024, 1025, 1089, 2049];

/// Verifies: [DESIGN] RULES.md D30, D460: XXH3 streamed, 64 and 128 bits,
/// of no seed, a seed and a secret, gives what the input gives at once, at
/// every split into two pieces and into three, of lengths at every
/// boundary.
#[test]
fn xxh3_streamed_equals_at_once_at_every_split() {
    let buffer = sanity_buffer();
    let secret = Secret::new(&buffer[SECRET_AT..SECRET_AT + SECRET_LEN]).unwrap();
    let states = || [(Xxh3::new(), None), (Xxh3::with_seed(0x9E37_79B1), Some(0x9E37_79B1_u64)), (Xxh3::with_secret(secret), None)];
    for len in SPLIT_LENGTHS {
        let input = &buffer[..len];
        for (n, (fresh, seed)) in states().into_iter().enumerate() {
            let (whole64, whole128) = match (n, seed) {
                (2, _) => (xxh3_64_with_secret(input, secret), xxh3_128_with_secret(input, secret)),
                (_, s) => (xxh3_64_with_seed(input, s.unwrap_or(0)), xxh3_128_with_seed(input, s.unwrap_or(0))),
            };
            for cut in 0..=len {
                let two = fresh.clone().updated(&input[..cut]).updated(&input[cut..]);
                assert_eq!((two.digest64(), two.digest128()), (whole64, whole128), "len {len} cut {cut} state {n}");
                let second = cut.midpoint(len);
                let three = fresh.clone().updated(&input[..cut]).updated(&input[cut..second]).updated(&input[second..]);
                assert_eq!(three.digest64(), whole64, "len {len} cuts {cut} {second} state {n}");
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D30, D460: a long input streamed in pieces
/// of every size from 1 to 300 bytes gives what it gives at once, and a
/// digest leaves the state to go on.
#[test]
fn xxh3_streamed_in_pieces_of_every_size() {
    let buffer = sanity_buffer();
    for piece in 1..=300 {
        let mut state = Xxh3::with_seed(42);
        for chunk in buffer.chunks(piece) {
            state.update(chunk);
            let _ = state.digest64();
        }
        assert_eq!(state.digest64(), xxh3_64_with_seed(&buffer, 42), "piece {piece}");
        assert_eq!(state.digest128(), xxh3_128_with_seed(&buffer, 42), "piece {piece}");
    }
}

/// Verifies: [DESIGN] RULES.md D30, D460: XXH64 streamed gives what the
/// input gives at once, at every split.
#[test]
fn xxh64_streamed_equals_at_once_at_every_split() {
    let buffer = sanity_buffer();
    for len in [0, 1, 4, 7, 8, 31, 32, 33, 63, 64, 65, 100, 257] {
        let input = &buffer[..len];
        for cut in 0..=len {
            let mut state = Xxh64::new(7);
            state.update(&input[..cut]);
            state.update(&input[cut..]);
            assert_eq!(state.digest(), xxh64(input, 7), "len {len} cut {cut}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D460: a word hashed by `Words` is XXH3-64
/// of its 8 bytes, little-endian, by the same seed, and a pair of words of
/// their 16.
#[test]
fn words_hash_as_their_bytes() {
    for seed in [0, 1, 0x9E37_79B1, u64::MAX, 0x0123_4567_89AB_CDEF] {
        let words = Words::new(seed);
        for word in [0, 1, u64::MAX, 0x8000_0000_0000_0000, 0xDEAD_BEEF, 42_u64 << 40_u32] {
            assert_eq!(words.hash(word), xxh3_64_with_seed(&word.to_le_bytes(), seed), "seed {seed} word {word}");
            let pair = [word.to_le_bytes(), seed.to_le_bytes()].concat();
            assert_eq!(words.pair(word, seed), xxh3_64_with_seed(&pair, seed), "seed {seed} pair {word}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D30: a secret shorter than 136 bytes is
/// refused; of seed 0 the derived secret is the default one, and of
/// another seed its words move by the seed.
#[test]
fn secrets_are_checked_and_derived() {
    assert!(Secret::new(&[0; SECRET_SIZE_MIN - 1]).is_none());
    assert_eq!(Secret::new(&[0; SECRET_SIZE_MIN]).unwrap().as_bytes().len(), SECRET_SIZE_MIN);
    assert_eq!(derive_secret(0), DEFAULT_SECRET);
    let derived = derive_secret(5);
    let word = |s: &[u8], at: usize| u64::from_le_bytes(s[at..at + 8].try_into().unwrap());
    assert_eq!(word(&derived, 0), word(&DEFAULT_SECRET, 0).wrapping_add(5));
    assert_eq!(word(&derived, 8), word(&DEFAULT_SECRET, 8).wrapping_sub(5));
}

/// Verifies: [DESIGN] RULES.md D30 (the `HashFunction` contract): XXH3-64
/// and XXH3-128 as hash functions give their canonical big-endian bytes,
/// of their own lengths and ids, and the digest of parts is that of the
/// parts concatenated.
#[test]
fn xxh3_hash_functions_keep_the_contract() {
    let buffer = sanity_buffer();
    let (a, b) = buffer.split_at(1000);
    let functions: [&dyn HashFunction; 2] = [&Xxh3Hash, &Xxh128Hash];
    assert_eq!(Xxh3Hash.hash(&buffer).as_bytes(), xxh3_64(&buffer).to_be_bytes());
    assert_eq!(Xxh128Hash.hash(&buffer).as_bytes(), xxh3_128(&buffer).to_be_bytes());
    assert_eq!((XXH3_64, XXH3_128), (HashId(14), HashId(15)));
    for function in functions {
        assert_eq!(function.hash_parts(&[a, b]), function.hash(&buffer), "{}", function.name());
        assert_eq!(function.hash_parts(&[]), function.hash(&[]), "{}", function.name());
        assert_eq!(function.hash(&buffer).as_bytes().len(), function.digest_len());
    }
}

/// Verifies: [DESIGN] RULES.md D460: a secret long enough for blocks of
/// more stripes than the default's hashes long inputs as the reference
/// does: streamed or at once alike, and apart from the default's.
#[test]
fn a_longer_secret_makes_longer_blocks() {
    let buffer = sanity_buffer();
    let secret = Secret::new(&buffer[..1000]).unwrap();
    let whole = xxh3_64_with_secret(&buffer, secret);
    assert_ne!(whole, xxh3_64(&buffer));
    for cut in [0, 1, 255, 256, 257, 3000, 4000] {
        let state = Xxh3::with_secret(secret).updated(&buffer[..cut]).updated(&buffer[cut..]);
        assert_eq!((state.digest64(), state.digest128()), (whole, xxh3_128_with_secret(&buffer, secret)), "cut {cut}");
    }
}

/// Verifies: [DESIGN] RULES.md D460: `Xxh3Hasher` gives XXH3-64 of the
/// bytes written, by its seed, every integer little-endian, whether the
/// bytes stay within its buffer or pass it; a map keyed so finds its keys.
#[test]
fn the_hasher_is_xxh3_of_the_bytes_written() {
    use std::hash::{BuildHasher, Hasher};
    let buffer = sanity_buffer();
    for len in [0, 1, 8, 63, 64, 65, 200, 1000, 4000] {
        for pieces in [1, 3, 64] {
            let mut hasher = Xxh3BuildHasher::with_seed(9).build_hasher();
            buffer[..len].chunks(pieces).for_each(|piece| hasher.write(piece));
            assert_eq!(hasher.finish(), xxh3_64_with_seed(&buffer[..len], 9), "len {len} pieces {pieces}");
        }
    }
    let mut hasher = Xxh3Hasher::default();
    hasher.write_u8(1);
    hasher.write_u16(0x0302);
    hasher.write_u32(0x0706_0504);
    hasher.write_u64(0x0F0E_0D0C_0B0A_0908);
    hasher.write_u128(0x1F1E_1D1C_1B1A_1918_1716_1514_1312_1110);
    hasher.write_usize(0x2726_2524_2322_2120);
    let bytes: Vec<u8> = (1..=0x27).collect();
    assert_eq!(hasher.finish(), xxh3_64(&bytes));
    let map: Xxh3Map<String, usize> = (0..100).map(|n| (format!("key {n}"), n)).collect();
    assert_eq!(map.get("key 42"), Some(&42));
    let set: Xxh3Set<u64> = (0..100).collect();
    assert!(set.contains(&99) && !set.contains(&100));
}
