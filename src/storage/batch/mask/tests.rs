//! Unit tests of null bitmaps (`mask.rs`) and selections (`sel.rs`).

use super::*;
use crate::storage::sel::{Sel, kept_count, nth, places_of, rows_of};

/// A pattern of `len` bits, set on both sides of each word's edge.
fn pattern(len: usize) -> Vec<bool> {
    (0..len).map(|n| n % 3 == 0 || n % 64 == 63 || n % 64 == 0).collect()
}

/// Verifies: [DESIGN] RULES.md D354 a mask of every length from none to
/// past two words keeps each bit as the `bool`s it is made of, counts
/// them, writes them as bytes and reads them back, and its bits past its
/// length stay unset under each operation.
#[test]
fn masks_keep_their_bits_at_every_length() {
    for len in 0..=130 {
        let bits = pattern(len);
        let mask = Mask::from_bools(&bits);
        assert_eq!((mask.len(), mask.is_empty()), (len, len == 0));
        assert_eq!(mask.to_bools(), bits, "of {len}");
        assert_eq!(mask.count(), bits.iter().filter(|&&b| b).count(), "count of {len}");
        assert_eq!(mask.any(), bits.contains(&true));
        assert_eq!(Mask::from_lsb_bytes(&mask.lsb_bytes(), 0, len), mask, "bytes of {len}");
        assert_eq!(mask.lsb_bytes().len(), lsb_len(len));
        let mut pushed = Mask::default();
        for &b in &bits {
            pushed.push(b);
        }
        assert_eq!(pushed, mask, "pushed of {len}");
        let mut set = Mask::none(len);
        bits.iter().enumerate().filter(|(_, b)| **b).for_each(|(n, _)| set.set(n));
        assert_eq!(set, mask, "set of {len}");
        joined_alike(&mask, &bits);
    }
}

/// Checks the masks `mask`, of `bits`, makes joined with every bit set.
fn joined_alike(mask: &Mask, bits: &[bool]) {
    let len = bits.len();
    let all = Mask::from_bools(&vec![true; len]);
    let inverse: Vec<bool> = bits.iter().map(|b| !b).collect();
    assert_eq!(all.and_not(mask).to_bools(), inverse, "and_not of {len}");
    assert_eq!(all.and_not(mask).count(), len.saturating_sub(mask.count()), "nothing past {len}");
    assert_eq!(mask.or(&all.and_not(mask)), all);
    assert_eq!(&mask.and(&all), mask);
    assert!(!mask.get(len) && !mask.get(len.saturating_add(64)), "past {len}");
}

/// Verifies: [DESIGN] RULES.md D354 bits read from bytes at an offset not
/// on a byte's edge are those from the offset on, as a chunk's bitmap of
/// a group read from a row within it keeps them, none past its length.
#[test]
fn masks_read_from_bytes_at_any_offset() {
    let bits = pattern(200);
    let bytes = Mask::from_bools(&bits).lsb_bytes();
    for from in [0, 1, 7, 8, 9, 63, 64, 65, 100] {
        for len in [0, 1, 63, 64, 65, 100] {
            let want: Vec<bool> = (from..from + len).map(|n| bits.get(n).copied().unwrap_or(false)).collect();
            let read = Mask::from_lsb_bytes(&bytes, from, len);
            assert_eq!(read.to_bools(), want, "{len} from {from}");
            assert_eq!(read.count(), want.iter().filter(|&&b| b).count(), "no bit past {len} from {from}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D354 masks appended one after another at
/// every length, each of the last on or off a word's edge, and unset bits
/// added between them, keep each bit and its count, none past the length.
#[test]
fn masks_append_at_any_length() {
    let bits = pattern(300);
    for first in [0, 1, 63, 64, 65, 130] {
        for second in [0, 1, 63, 64, 65, 130] {
            let mut mask = Mask::from_bools(&bits[..first]);
            mask.append(&Mask::from_bools(&bits[first..first + second]));
            mask.extend_unset(3);
            mask.append(&Mask::from_bools(&bits[..5]));
            let mut want = bits[..first + second].to_vec();
            want.extend([false; 3]);
            want.extend(&bits[..5]);
            assert_eq!(mask.to_bools(), want, "{first} then {second}");
            assert_eq!(mask.count(), want.iter().filter(|&&b| b).count(), "{first} then {second}");
            assert_eq!(mask.words().len(), want.len().div_ceil(64), "{first} then {second}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D354 a mask's bits gathered at rows, and
/// whether any of them is set; none of a mask of no bit set.
#[test]
fn masks_gather_and_look_at_rows() {
    let mask = Mask::from_bools(&pattern(130));
    let picks = [129_u32, 0, 64, 1, 63];
    assert_eq!(mask.gathered(&picks).unwrap().to_bools(), [true, true, true, false, true]);
    assert_eq!(mask.gathered(&[1, 2]), None, "no bit of them set");
    assert!(mask.any_of(Some(&[1, 3])) && !mask.any_of(Some(&[1, 2])) && mask.any_of(None));
    assert!(!Mask::none(70).any_of(None));
    assert_eq!(mask_of(&[false, false]), None);
    assert_eq!(mask_of(&[false, true]).unwrap().to_bools(), [false, true]);
}

/// Verifies: [DESIGN] RULES.md D354 a selection's rows, counts and places,
/// and the `n`th row of a selection or of all.
#[test]
fn selections_name_their_rows() {
    let sel = Sel::of(vec![1, 4, 9]);
    assert_eq!((sel.len(), sel.is_empty(), sel.as_slice()), (3, false, &[1_u32, 4, 9][..]));
    assert_eq!(sel.places().collect::<Vec<_>>(), [1, 4, 9]);
    assert_eq!(Sel::all(3).into_vec(), [0, 1, 2]);
    assert_eq!([2_usize, 5].into_iter().collect::<Sel>(), Sel::of(vec![2, 5]));
    assert_eq!(rows_of([3_usize, 7]), [3, 7]);
    assert_eq!((kept_count(Some(&sel), 10), kept_count(None, 10)), (3, 10));
    assert_eq!((nth(Some(&sel), 1), nth(None, 1), nth(Some(&sel), 3)), (4, 1, usize::MAX));
    assert_eq!((places_of(Some(sel.clone()), 10), places_of(None, 3)), (vec![1, 4, 9], vec![0, 1, 2]));
    let mut grown = Sel::default();
    grown.buffer().push(6);
    assert_eq!(&*grown, &[6]);
}
