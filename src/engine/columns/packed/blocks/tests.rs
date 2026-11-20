//! Unit tests of unpacking a block at a time (`blocks.rs`).

use super::super::{get, pack};
use super::*;

/// 300 values of `bits` bits, the greatest of the width among them every
/// seventh, packed: values, then their bytes.
fn packed(bits: u32) -> (Vec<u64>, Vec<u8>) {
    let top = mask(bits);
    let values: Vec<u64> =
        (0_u64..300_u64).map(|n| if n % 7 == 3 { top } else { n.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(17) & top }).collect();
    let mut out = Vec::new();
    pack(values.iter().copied(), bits, &mut out);
    (values, out)
}

/// What reading the values from `from` to `to` one at a time by `get`
/// gives, each plus `least`, wrapping.
fn one_at_a_time(bytes: &[u8], (bits, least): (u32, i64), (from, to): (usize, usize)) -> Vec<i64> {
    (from..to).map(|n| least.wrapping_add(i64::from_le_bytes(get(bytes, bits, n).unwrap().to_le_bytes()))).collect()
}

/// Verifies: [DESIGN] RULES.md D352, block unpacking: of every width from
/// 0 to 64 bits, every range whose ends lie around the blocks' boundaries,
/// in one block, across one, across several and to the last value, reads
/// as `get` reads each value, each plus a least of 0, 1, -1, the least and
/// the greatest integer, wrapping as a frame of reference adds.
#[test]
fn blocks_read_as_one_at_a_time() {
    let ends = [0_usize, 1, 5, 63, 64, 65, 127, 128, 129, 191, 192, 255, 256, 299, 300];
    for bits in 0_u32..=64_u32 {
        let (_, bytes) = packed(bits);
        for least in [0_i64, 1, -1, i64::MIN, i64::MAX] {
            for (n, &from) in ends.iter().enumerate() {
                for &to in &ends[n..] {
                    let mut out = vec![7_i64];
                    unpack_words(&bytes, (bits, least), (from, to), &mut out).unwrap();
                    assert_eq!(out[0], 7, "what was there is kept");
                    assert_eq!(out[1..], one_at_a_time(&bytes, (bits, least), (from, to)), "{bits} bits, {least}, {from} to {to}");
                }
            }
        }
    }
}

/// Verifies: [DESIGN] RULES.md D352, block unpacking: the values packed are
/// those read, the frame's least added, of every width; bytes holding
/// fewer values than asked, a range backwards and a width above 64 give
/// none.
#[test]
fn blocks_give_the_values_packed() {
    for bits in 0_u32..=64_u32 {
        let (values, bytes) = packed(bits);
        let mut out = Vec::new();
        unpack_words(&bytes, (bits, -5), (0, values.len()), &mut out).unwrap();
        let expected: Vec<i64> = values.iter().map(|&v| (-5_i64).wrapping_add(i64::from_le_bytes(v.to_le_bytes()))).collect();
        assert_eq!(out, expected, "{bits} bits");
        if bits > 0 {
            assert_eq!(unpack_words(&bytes, (bits, 0), (0, values.len() + 64), &mut Vec::new()), None, "{bits} bits, too few");
        }
    }
    assert_eq!(unpack_words(&[0; 64], (8, 0), (5, 4), &mut Vec::new()), None, "backwards");
    assert_eq!(unpack_words(&[0; 1024], (65, 0), (0, 64), &mut Vec::new()), None, "wider than 64 bits");
}

/// Verifies: [DESIGN] RULES.md D352, block unpacking: a property sweep of
/// fixed seeds, of random widths, leasts and ranges over random values,
/// reads as one value at a time reads.
#[test]
fn blocks_sweep_of_fixed_seeds() {
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let mut next = || {
        state ^= state << 13_u32;
        state ^= state >> 7_u32;
        state ^= state << 17_u32;
        state
    };
    for _ in 0_u32..400_u32 {
        let bits = u32::try_from(next() % 65).unwrap();
        let count = usize::try_from(next() % 700).unwrap();
        let values: Vec<u64> = (0..count).map(|_| next() & mask(bits)).collect();
        let mut bytes = Vec::new();
        pack(values.iter().copied(), bits, &mut bytes);
        let least = i64::from_le_bytes(next().to_le_bytes());
        let from = usize::try_from(next()).unwrap() % (count + 1);
        let to = from + usize::try_from(next()).unwrap() % (count - from + 1);
        let mut out = Vec::new();
        unpack_words(&bytes, (bits, least), (from, to), &mut out).unwrap();
        assert_eq!(out, one_at_a_time(&bytes, (bits, least), (from, to)), "{bits} bits, {count} values, {from} to {to}");
    }
}
