//! Unit tests of the keys a runtime filter tells a scan (`members.rs`,
//! RULES.md D1294).

use super::*;
use crate::kernels::portable::bloom::{BLOCK_WORDS, bits_of, block_of};

/// XXH3's seed of the Bloom filters made here.
const SEED: u64 = 0x5EED;

/// A blocked Bloom filter of `blocks` blocks of the hashes of `keys`,
/// each the hash of the bits of the `f64` equal to it, as a join's table
/// hashes a whole number.
fn bloom(keys: &[i32], blocks: usize) -> Held {
    let hash = Words::new(SEED);
    let mut words = vec![0_u64; blocks.checked_mul(BLOCK_WORDS).unwrap()];
    for &k in keys {
        let h = hash.hash(f64::from(k).to_bits());
        let base = block_of(h, blocks).checked_mul(BLOCK_WORDS).unwrap();
        for (w, bit) in bits_of(h) {
            words[base.checked_add(w).unwrap()] |= bit;
        }
    }
    Held::Bloom { words, hash }
}

/// Verifies: [DESIGN] RULES.md D1294: each form holds every built key. A
/// bitmap holds, from its least on, the values whose bit is set, across
/// its words, and none below its least or past its words; a key set its
/// keys alone; a Bloom filter every built key, negative ones too, by the
/// hash a join gives a whole number, and few of the others. An integer
/// of another scale than the keys', or of 2^51 or more under a Bloom
/// filter, cannot be asked and is held.
#[test]
fn each_form_holds_its_keys() {
    let bits = Members::new(Held::Bits { least: 10, words: vec![0b101, 1 << 11_u32] }, 0, 16);
    let held: Vec<i64> = (0..200).filter(|&u| bits.may_hold(u, 0)).collect();
    assert_eq!(held, vec![10, 12, 85], "bits 0 and 2 of the first word, bit 11 of the second");
    assert!(!bits.may_hold(9, 0) && !bits.may_hold(i64::MIN, 0) && !bits.may_hold(i64::MAX, 0));
    assert!(bits.may_hold(11, 2), "another scale is not asked");
    let keys = Members::new(Held::Keys(vec![-7, 3, 9, 100]), 2, 16);
    let found: Vec<i64> = (-20..200).filter(|&u| keys.may_hold(u, 2)).collect();
    assert_eq!(found, vec![-7, 3, 9, 100]);
    assert!(keys.may_hold(4, 0), "another scale is not asked");
    let built: Vec<i32> = (-500_i32..500_i32).map(|k| k.checked_mul(37).unwrap()).collect();
    let filter = Members::new(bloom(&built, 16), 0, 16);
    assert!(built.iter().all(|&k| filter.may_hold(i64::from(k), 0)), "every built key");
    let others = (100_000..110_000_i64).filter(|&u| filter.may_hold(u, 0)).count();
    assert!(others < 1_000, "few of 10,000 keys not built: {others}");
    assert!(filter.may_hold(1_i64 << 51_u32, 0) && filter.may_hold(i64::MIN, 0), "too large to ask");
    assert!(filter.may_hold(100_001, 1), "another scale is not asked");
    let hundredths = Members::new(bloom(&built, 16), 2, 16);
    assert!((100_000..101_000_i64).all(|u| hundredths.may_hold(u, 2)), "keys not whole numbers are not asked of a Bloom filter");
}

/// Verifies: [DESIGN] RULES.md D1294 (MC/DC): a scan asks through its
/// first runs whatever they keep; after them it asks only if they kept
/// fewer than one in `share` of the rows asked, not at exactly that
/// share; and runs past the first change nothing.
#[test]
fn a_scan_stops_asking_once_its_first_runs_kept_too_many() {
    let counted = |each: usize| {
        let members = Members::new(Held::Keys(vec![1]), 0, 3);
        for _ in 0_i32..2_i32 {
            members.count(80, each);
            assert!(members.asking(8), "through its first runs");
        }
        members.count(80, each);
        members
    };
    assert!(counted(9).asking(8), "27 of 240, under one in eight");
    assert!(!counted(10).asking(8), "30 of 240, one in eight");
    assert!(!counted(80).asking(8), "every row kept");
    let (few, many) = (counted(0), counted(80));
    few.count(80, 80);
    many.count(80, 0);
    assert!(few.asking(8) && !many.asking(8), "a run past the first is not counted");
}
