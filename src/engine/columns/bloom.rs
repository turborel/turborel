//! \[DESIGN\] Bloom filters of a column's parts (RULES.md D31): of each
//! `PART_ROWS` rows of a column COLUMNSTORE.BLOOM names, a filter of
//! `FILTER_BYTES`, or a page's capacity if less, each value not null set in
//! `PROBES` of its bits, so a scan skips the parts an equality's values
//! cannot be in. A value's bits are those of its bucket (`value::bucket`),
//! the form every two values that compare equal share, hashed by
//! MurmurHash3 x64-128 (`hash/murmur.rs`): the i-th bit is h1 + i × h2 of
//! its two halves (Kirsch and Mitzenmacher, "Less Hashing, Same
//! Performance: Building a Better Bloom Filter", 2006). Of a full part,
//! about 10 bits a value, a value not in it is taken as in it about once in
//! 120. Filters are kept on pages of their own, as many as a page holds,
//! read only by a scan of an equality.

use super::group::{Filter, Group, PART_ROWS, Pages, Seal};
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::hash::murmur::murmur3_x64_128;
use crate::index::Probe;
use crate::storage::{Condition, Conjunct};
use crate::value::{Bucket, CompOp, Value, bucket};

/// \[DESIGN\] The bytes of a part's filter: 10 bits for each of its 1024
/// rows.
pub const FILTER_BYTES: usize = 1280;
/// \[DESIGN\] The bits each value sets: the fewest false matches of 10
/// bits a value, ln 2 × 10 rounded.
const PROBES: u64 = 7;

/// The tags of a bucket's kinds, before its bytes.
const CHAR_TAG: u8 = 1;
const NUMBER_TAG: u8 = 2;
const BITS_TAG: u8 = 3;
const INSTANT_TAG: u8 = 4;
const SPAN_TAG: u8 = 5;
const EXTENSION_TAG: u8 = 6;

/// The two halves of the hash of `v`'s bucket; none for the null value,
/// which equals nothing.
pub fn hashed(v: &Value) -> Option<(u64, u64)> {
    let mut key = Vec::new();
    match bucket(v)? {
        Bucket::Char(s) => {
            key.push(CHAR_TAG);
            key.extend_from_slice(s.as_bytes());
        }
        Bucket::Number(bits) => {
            key.push(NUMBER_TAG);
            key.extend_from_slice(&bits.to_le_bytes());
        }
        Bucket::Bits(b) => {
            key.push(BITS_TAG);
            key.extend_from_slice(&b);
        }
        Bucket::Instant(utc) => {
            key.push(INSTANT_TAG);
            key.extend_from_slice(&utc.to_le_bytes());
        }
        Bucket::Span(units) => {
            key.push(SPAN_TAG);
            key.extend_from_slice(&units.to_le_bytes());
        }
        Bucket::Extension(b) => {
            key.push(EXTENSION_TAG);
            key.extend_from_slice(&b);
        }
    }
    Some(murmur3_x64_128(&key, 0))
}

/// The byte and the bit in it of the `i`th probe of `hash`, of a filter of
/// `bytes`.
fn bit_of((h1, h2): (u64, u64), i: u64, bytes: usize) -> Option<(usize, u8)> {
    let bits = u64::try_from(bytes).ok()?.checked_mul(8)?;
    let bit = h1.wrapping_add(i.wrapping_mul(h2)).checked_rem(bits)?;
    Some((usize::try_from(bit >> 3_u32).ok()?, 1_u8 << u32::try_from(bit & 7).ok()?))
}

/// The filter of `values`, of `bytes`.
pub fn filter_of(values: &[Value], bytes: usize) -> Vec<u8> {
    let mut filter = vec![0_u8; bytes];
    for hash in values.iter().filter_map(hashed) {
        for i in 0..PROBES {
            let Some((byte, bit)) = bit_of(hash, i, bytes) else { continue };
            if let Some(b) = filter.get_mut(byte) {
                *b |= bit;
            }
        }
    }
    filter
}

/// Whether a value of `hash` may be among those `filter` was made of:
/// every one of its bits set.
pub fn may_hold(filter: &[u8], hash: (u64, u64)) -> bool {
    (0..PROBES).all(|i| bit_of(hash, i, filter.len()).is_none_or(|(byte, bit)| filter.get(byte).is_none_or(|b| b & bit != 0)))
}

/// The bytes of each filter of a page of `capacity`.
pub fn filter_bytes(capacity: usize) -> usize {
    FILTER_BYTES.min(capacity).max(1)
}

/// Writes `filters`, each of the same bytes, as many to a page as it
/// holds, and gives where each is, in order.
///
/// # Errors
///
/// As `Pages::put`.
pub fn write_filters<S: Seal>(pages: &Pages<'_, S>, filters: &[Vec<u8>], written: &mut Vec<PageId>) -> Result<Vec<Filter>> {
    let len = filters.first().map_or(1, Vec::len).max(1);
    let per_page = pages.capacity.checked_div(len).unwrap_or(1).max(1);
    let mut out = Vec::with_capacity(filters.len());
    for these in filters.chunks(per_page) {
        let page = pages.put(&these.concat(), written)?;
        let len32 = u32::try_from(len).unwrap_or(u32::MAX);
        out.extend((0..these.len()).map(|n| Filter { page, at: u32::try_from(n).unwrap_or(u32::MAX).saturating_mul(len32), len: len32 }));
    }
    Ok(out)
}

/// The column an equality or IN of one column, `condition`, compares, and
/// the values it asks for; none for another condition.
fn asked(condition: &Conjunct) -> Option<(usize, &[Value])> {
    match condition {
        Conjunct::Condition(Condition { columns, probe }) => {
            let [column] = columns.as_slice() else { return None };
            match probe {
                Probe::Compare { op: CompOp::Eq, value } => Some((*column, std::slice::from_ref(value))),
                Probe::Prefix { equal, then: None } => Some((*column, equal.as_slice())),
                Probe::All | Probe::Compare { .. } | Probe::Prefix { .. } | Probe::Range { .. } => None,
            }
        }
        Conjunct::In { column, values } => Some((*column, values.as_slice())),
        Conjunct::Null { .. } | Conjunct::NotIn { .. } | Conjunct::Any(_) => None,
    }
}

/// The runs of `group`'s rows, among `within`, whose parts' Bloom filters
/// may hold a value an equality or IN, `condition`, asks for of their
/// column: those of every part without a filter, and none of a part
/// whose filter holds none of them. None if `condition` is of another
/// kind, or its column has no filters. Only the filters of parts among
/// `within` are read, each page once.
///
/// # Errors
///
/// As `Pages::with`.
pub fn kept<S: Seal>(group: &Group, pages: &Pages<'_, S>, condition: &Conjunct, within: &[(u32, u32)]) -> Result<Option<Vec<(u32, u32)>>> {
    let Some((column, values)) = asked(condition) else { return Ok(None) };
    let Some(zones) = group.columns.get(column).filter(|z| z.iter().flat_map(|z| &z.parts).any(|p| p.filter.is_some())) else {
        return Ok(None);
    };
    let hashes: Vec<(u64, u64)> = values.iter().filter_map(hashed).collect();
    let among = |(from, to): (u32, u32)| within.iter().any(|&(a, b)| a < to && from < b);
    let parts: Vec<((u32, u32), Option<Filter>)> = zones
        .iter()
        .flat_map(|zone| {
            let end = zone.run.first.saturating_add(zone.run.rows);
            zone.parts.iter().enumerate().map(move |(n, part)| {
                let from = zone.run.first.saturating_add(u32::try_from(n).unwrap_or(u32::MAX).saturating_mul(PART_ROWS));
                ((from, from.saturating_add(PART_ROWS).min(end)), part.filter)
            })
        })
        .filter(|&(range, _)| among(range))
        .collect();
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for same in parts.chunk_by(|a, b| a.1.map(|f| f.page) == b.1.map(|f| f.page)) {
        let held: Vec<bool> = match same.first().and_then(|p| p.1) {
            None => vec![true; same.len()],
            Some(first) => pages.with(first.page, |bytes| Ok(same.iter().map(|p| holds(bytes, p.1, &hashes)).collect()))?,
        };
        for (&((from, to), _), _) in same.iter().zip(held).filter(|(_, h)| *h) {
            match runs.last_mut().filter(|last| last.1 == from) {
                Some(last) => last.1 = to,
                None => runs.push((from, to)),
            }
        }
    }
    Ok(Some(runs))
}

/// Whether the filter at `filter` of page content `bytes` may hold one of
/// `hashes`: so if it is not there.
fn holds(bytes: &[u8], filter: Option<Filter>, hashes: &[(u64, u64)]) -> bool {
    let Some(f) = filter else { return true };
    let (at, len) = (usize::try_from(f.at).unwrap_or(usize::MAX), usize::try_from(f.len).unwrap_or(0));
    bytes.get(at..at.saturating_add(len)).is_none_or(|bits| hashes.iter().any(|&h| may_hold(bits, h)))
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use crate::engine::{MEMORY_POOL_BYTES, memory_file};
    use crate::memory::MemoryBudget;
    use crate::value::Decimal;

    fn int(n: i128) -> Value {
        Value::Exact(Decimal::new(n, 0))
    }

    /// Pages stored as they are given.
    struct Plain;

    impl Seal for Plain {
        fn seal(&self, _: u64, content: &[u8]) -> Result<Vec<u8>> {
            Ok(content.to_vec())
        }
        fn open<'a>(&self, _: u64, stored: &'a [u8]) -> Result<Cow<'a, [u8]>> {
            Ok(Cow::Borrowed(stored))
        }
    }

    /// Verifies: [DESIGN] RULES.md D31, Bloom filters: a filter of a full
    /// part holds each of its values, and each value that compares equal
    /// to one [X3.135-1986 §5.11 GR4, GR5]: of another scale, approximate,
    /// padded with spaces; the null value hashes to nothing; of values not
    /// among them, fewer than one in fifty are taken as held.
    #[test]
    fn filters_hold_their_values_and_few_others() {
        let mut values: Vec<Value> = (0..1020_i128).map(|n| int(n * 7919)).collect();
        values.extend([Value::Exact(Decimal::new(150, 2)), Value::Char("abc".into()), Value::Null, Value::Approx(2.5)]);
        let filter = filter_of(&values, FILTER_BYTES);
        let held = |v: &Value| hashed(v).is_some_and(|h| may_hold(&filter, h));
        assert!(values.iter().filter(|v| !v.is_null()).all(held), "each of its values");
        assert!(held(&Value::Exact(Decimal::new(15, 1))), "1.5 as 1.50");
        assert!(held(&Value::Approx(1.5)), "1.5E0 as 1.50");
        assert!(held(&Value::Approx(7919.0)), "an approximate number equal to an exact one");
        assert!(held(&Value::Exact(Decimal::new(25, 1))), "2.5 as 2.5E0");
        assert!(held(&Value::Char("abc   ".into())), "padded");
        assert_eq!(hashed(&Value::Null), None);
        let false_matches = (0..10_000_i128).filter(|n| held(&int(n * 7919 + 1))).count();
        assert!(false_matches < 200, "{false_matches} of 10,000");
    }

    /// Verifies: [DESIGN] RULES.md D31, Bloom filters: as many filters are
    /// written to a page as it holds, each found where it was put; a page
    /// smaller than a filter holds a filter of its size; a part without a
    /// filter, or one past its page's end, may hold any value.
    #[test]
    fn filters_are_packed_on_pages() {
        let (pool, file) = memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES), "a test").unwrap();
        let pages = Pages { pool: &pool, file, capacity: 4000, sealing: &Plain, compress: true };
        let filters: Vec<Vec<u8>> = (0..7_i128).map(|n| filter_of(&[int(n)], FILTER_BYTES)).collect();
        let mut written = Vec::new();
        let places = write_filters(&pages, &filters, &mut written).unwrap();
        assert_eq!(written.len(), 3, "three of 1280 bytes a page of 4000");
        for (n, (place, filter)) in places.iter().zip(&filters).enumerate() {
            let bytes = pages.get(place.page).unwrap();
            let at = usize::try_from(place.at).unwrap();
            assert_eq!(&bytes[at..at + FILTER_BYTES], filter.as_slice(), "filter {n}");
            let hash = hashed(&int(i128::try_from(n).unwrap())).unwrap();
            assert!(holds(&bytes, Some(*place), &[hash]));
        }
        assert_eq!(filter_bytes(512), 512);
        assert_eq!(filter_bytes(1 << 16), FILTER_BYTES);
        assert!(holds(&[], None, &[]), "no filter");
        let past = Filter { page: written[0], at: 9000, len: 1280 };
        assert!(holds(&[0; 16], Some(past), &[]), "past its page's end");
        pages.free(&written);
    }
}
