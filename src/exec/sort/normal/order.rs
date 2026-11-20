//! \[DESIGN\] RULES.md D364: rows put in the order of their normal keys,
//! stably. Keys of at most 8 bytes are sorted as `u64`s, of at most 16 as
//! `u128`s, each beside its row's place, by radix sort (`radix.rs`) if
//! there are enough of them, else by comparison, the place breaking ties;
//! longer keys as byte strings, each row's place sorted by its key and
//! then by itself. Rows of equal keys of strings longer than their bytes
//! hold are then sorted by the comparator `ties`, stably, each run of
//! them apart. The bytes every key shares first order nothing, so each
//! key is read past them, and keys of strings of a long shared start are
//! sorted as short ones.

use std::cmp::Ordering;

use super::radix;

/// \[DESIGN\] The fewest keys a radix sort sorts, as below them its passes
/// over the counts cost more than comparisons.
const RADIX_LEAST: usize = 512;

/// The places, from 0, of `rows` rows whose keys `keys` holds, `width`
/// bytes each, in the keys' order, rows of equal keys in their places' order;
/// and if `truncates`, rows of equal keys ordered by `ties` of their
/// places. None if the rows are more than a place holds.
pub(in crate::exec::sort) fn order(
    (keys, rows): (&[u8], usize),
    width: usize,
    truncates: bool,
    ties: impl FnMut(u32, u32) -> Ordering,
) -> Option<Vec<u32>> {
    let count = u32::try_from(rows).ok()?;
    let skip = shared(keys, width);
    let rest = width.saturating_sub(skip);
    let mut places: Vec<u32> = if rest == 0 {
        (0..count).collect()
    } else if rest <= size_of::<u64>() {
        by_radix(packed::<u64, { size_of::<u64>() }>(keys, (width, skip), u64::from_be_bytes))
    } else if rest <= size_of::<u128>() {
        by_radix(packed::<u128, { size_of::<u128>() }>(keys, (width, skip), u128::from_be_bytes))
    } else {
        let mut places: Vec<u32> = (0_u32..).take(rows).collect();
        let past = |place| key(keys, width, place).get(skip..).unwrap_or_default();
        places.sort_unstable_by(|&a, &b| past(a).cmp(past(b)).then(a.cmp(&b)));
        places
    };
    if truncates {
        tied(&mut places, |a, b| key(keys, width, a) == key(keys, width, b), ties);
    }
    Some(places)
}

/// The bytes every one of `keys`, `width` bytes each, has alike first.
fn shared(keys: &[u8], width: usize) -> usize {
    let mut each = keys.chunks_exact(width.max(1));
    let Some(first) = each.next() else { return 0 };
    let mut alike = width;
    for k in each {
        alike = first.iter().zip(k).take(alike).take_while(|(a, b)| a == b).count();
        if alike == 0 {
            break;
        }
    }
    alike
}

/// The key of the row at `place`.
fn key(keys: &[u8], width: usize, place: u32) -> &[u8] {
    let from = usize::try_from(place).unwrap_or(usize::MAX).saturating_mul(width);
    keys.get(from..from.saturating_add(width)).unwrap_or_default()
}

/// Each key of `keys`, `width` bytes, past its first `skip`, as an
/// unsigned integer of `N` bytes, its bytes first and zeros after, by
/// `of`; beside its place.
fn packed<K, const N: usize>(keys: &[u8], (width, skip): (usize, usize), of: impl Fn([u8; N]) -> K) -> Vec<(K, u32)>
where
    K: radix::Key,
{
    keys.chunks_exact(width)
        .zip(0_u32..)
        .map(|(k, place)| {
            let mut bytes = [0_u8; N];
            bytes.iter_mut().zip(k.get(skip..).unwrap_or_default()).for_each(|(to, from)| *to = *from);
            (of(bytes), place)
        })
        .collect()
}

/// The places of `pairs`, in their keys' order, stably.
fn by_radix<K: radix::Key + Ord>(mut pairs: Vec<(K, u32)>) -> Vec<u32> {
    if pairs.len() >= RADIX_LEAST {
        return radix::sort(pairs).into_iter().map(|(_, place)| place).collect();
    }
    // The place breaks ties, so the sort is stable.
    pairs.sort_unstable();
    pairs.into_iter().map(|(_, place)| place).collect()
}

/// Each run of `places` whose keys are `equal` sorted by `ties`, stably.
fn tied(places: &mut [u32], equal: impl Fn(u32, u32) -> bool, mut ties: impl FnMut(u32, u32) -> Ordering) {
    let mut from = 0_usize;
    while from < places.len() {
        let first = places.get(from).copied().unwrap_or(0);
        let run = places.get(from..).unwrap_or_default().iter().take_while(|&&p| equal(first, p)).count().max(1);
        let to = from.saturating_add(run);
        if let Some(each) = places.get_mut(from..to) {
            each.sort_by(|&a, &b| ties(a, b));
        }
        from = to;
    }
}
