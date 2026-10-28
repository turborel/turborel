//! \[DESIGN\] The bytes of a page of a hash table's bucket (`engine/hash.rs`):
//! one page of a bucket's chain, its entries in the order they were added,
//! with a slot for each, sorted by a fragment of its key's hash, so a key
//! is found by binary search, and only keys whose fragment matches are
//! compared.
//!
//! ```text
//! header (10 bytes, integers little-endian):
//!   entries u16, next page of the chain u64 (none: all ones)
//! heap:   entries, each key length u16, key, value length u16, value,
//!         in the order added
//! slots:  for each entry, its fragment u32 and heap offset u16, sorted by
//!         fragment; entries of one fragment in the order added
//! ```
//!
//! An entry is appended to the heap, its slot put in its place among the
//! slots: the heap's offsets never move but when an entry is removed. A
//! page holds as many bytes as its entries take, up to the page's
//! capacity; one that would not fit goes on the next page of the chain.
//! Split from hash.rs (RULES.md R-SIZE-2).

use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};

/// Bytes of a page's header.
pub(super) const HEADER: usize = 10;
/// Bytes of an entry beyond its key and value: their two lengths, and its
/// slot.
pub(super) const ENTRY_OVERHEAD: usize = 4 + SLOT;
/// Bytes of a slot: a fragment u32 and an offset u16.
const SLOT: usize = 6;

/// The `next` of a page that is the last of its chain.
const NONE: u64 = u64::MAX;

/// An entry: a key and its value.
pub(super) type Entry = (Vec<u8>, Vec<u8>);

/// The error for a page that is not a bucket's page.
fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a hash table's bucket page is malformed: {why}"))
}

/// The bytes an entry of `key` and `value` takes, its slot too.
pub(super) fn size(key: &[u8], value: &[u8]) -> usize {
    ENTRY_OVERHEAD.saturating_add(key.len()).saturating_add(value.len())
}

/// The fragment of a key's hash `h` its slot keeps: its high half, as the
/// low bits choose the bucket.
pub(super) fn fragment(h: u64) -> u32 {
    u32::try_from(h >> 32_u32).unwrap_or(u32::MAX)
}

fn u16_at(bytes: &[u8], at: usize) -> Result<usize> {
    let field: [u8; 2] = bytes.get(at..at.saturating_add(2)).and_then(|b| b.try_into().ok()).ok_or_else(|| corrupt("it is cut short"))?;
    Ok(usize::from(u16::from_le_bytes(field)))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    let field: [u8; 4] = bytes.get(at..at.saturating_add(4)).and_then(|b| b.try_into().ok()).ok_or_else(|| corrupt("it is cut short"))?;
    Ok(u32::from_le_bytes(field))
}

/// The number of entries of `page`, and where its slots begin.
fn layout(page: &[u8]) -> Result<(usize, usize)> {
    let count = u16_at(page, 0)?;
    let slots =
        page.len().checked_sub(count.saturating_mul(SLOT)).filter(|&s| s >= HEADER).ok_or_else(|| corrupt("its slots do not fit"))?;
    Ok((count, slots))
}

/// The fragment and heap offset of slot `n` of a page whose slots begin at
/// `slots`.
fn slot(page: &[u8], slots: usize, n: usize) -> Result<(u32, usize)> {
    let at = slots.saturating_add(n.saturating_mul(SLOT));
    Ok((u32_at(page, at)?, u16_at(page, at.saturating_add(4))?))
}

/// The key and value of the entry at heap offset `at`, as where they are.
fn entry_at(page: &[u8], at: usize, slots: usize) -> Result<((usize, usize), (usize, usize))> {
    let k = u16_at(page, at)?;
    let key = (at.saturating_add(2), at.saturating_add(2).saturating_add(k));
    let v = u16_at(page, key.1)?;
    let value = (key.1.saturating_add(2), key.1.saturating_add(2).saturating_add(v));
    if value.1 > slots {
        return Err(corrupt("an entry runs past its heap"));
    }
    Ok((key, value))
}

/// An empty page, last of its chain.
pub(super) fn empty() -> Vec<u8> {
    let mut page = vec![0_u8; 2];
    page.extend_from_slice(&NONE.to_le_bytes());
    page
}

/// A page of `entries`, each with its key's fragment, in order, followed
/// by `next`: its heap written in order and its slots sorted once.
pub(super) fn build(entries: &[(u32, &Entry)], next: Option<PageId>) -> Result<Vec<u8>> {
    let mut page = empty();
    set_next(&mut page, next)?;
    let mut slots = Vec::with_capacity(entries.len());
    for (f, (k, v)) in entries {
        slots.push((*f, u16::try_from(page.len()).map_err(|_| corrupt("its heap is past a u16 offset"))?));
        page.extend_from_slice(&u16::try_from(k.len()).unwrap_or(u16::MAX).to_le_bytes());
        page.extend_from_slice(k);
        page.extend_from_slice(&u16::try_from(v.len()).unwrap_or(u16::MAX).to_le_bytes());
        page.extend_from_slice(v);
    }
    // Stable: entries of one fragment stay in the order added.
    slots.sort_by_key(|s| s.0);
    for (f, offset) in slots {
        page.extend_from_slice(&f.to_le_bytes());
        page.extend_from_slice(&offset.to_le_bytes());
    }
    let count = u16::try_from(entries.len()).map_err(|_| corrupt("it holds too many entries"))?;
    page.get_mut(..2).ok_or_else(|| corrupt("it has no header"))?.copy_from_slice(&count.to_le_bytes());
    Ok(page)
}

/// The next page of the chain after `page`, if any.
pub(super) fn next(page: &[u8]) -> Result<Option<PageId>> {
    let field: [u8; 8] = page.get(2..HEADER).and_then(|b| b.try_into().ok()).ok_or_else(|| corrupt("it has no header"))?;
    let next = u64::from_le_bytes(field);
    Ok((next != NONE).then_some(PageId(next)))
}

/// Makes `next` the page after `page`.
pub(super) fn set_next(page: &mut [u8], next: Option<PageId>) -> Result<()> {
    let field = page.get_mut(2..HEADER).ok_or_else(|| corrupt("it has no header"))?;
    field.copy_from_slice(&next.map_or(NONE, |p| p.0).to_le_bytes());
    Ok(())
}

/// The first slot of `page` whose fragment is not below `wanted`, and the
/// first above it.
fn range(page: &[u8], (count, slots): (usize, usize), wanted: u32) -> Result<(usize, usize)> {
    let bound = |above: bool| -> Result<usize> {
        let (mut low, mut high) = (0, count);
        while low < high {
            let mid = low.saturating_add(high.saturating_sub(low) >> 1_u32);
            let f = slot(page, slots, mid)?.0;
            if f < wanted || (above && f == wanted) {
                low = mid.saturating_add(1);
            } else {
                high = mid;
            }
        }
        Ok(low)
    };
    Ok((bound(false)?, bound(true)?))
}

/// Whether `page` holds no entry.
pub(super) fn is_empty(page: &[u8]) -> Result<bool> {
    Ok(u16_at(page, 0)? == 0)
}

/// The entries of `page`, in the order added.
pub(super) fn entries(page: &[u8]) -> Result<Vec<Entry>> {
    let (count, slots) = layout(page)?;
    let bytes = |(from, to): (usize, usize)| page.get(from..to).map(<[u8]>::to_vec).unwrap_or_default();
    let mut at = HEADER;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let (k, v) = entry_at(page, at, slots)?;
        out.push((bytes(k), bytes(v)));
        at = v.1;
    }
    Ok(out)
}

/// The values of the entries of `page` whose key is `key`, whose fragment
/// is `wanted`, in the order added.
pub(super) fn find(page: &[u8], key: &[u8], wanted: u32) -> Result<Vec<Vec<u8>>> {
    let shape = layout(page)?;
    let (from, to) = range(page, shape, wanted)?;
    let mut out = Vec::new();
    for n in from..to {
        let (k, v) = entry_at(page, slot(page, shape.1, n)?.1, shape.1)?;
        if page.get(k.0..k.1) == Some(key) {
            out.extend(page.get(v.0..v.1).map(<[u8]>::to_vec));
        }
    }
    Ok(out)
}

/// Adds the entry of `key` and `value`, of fragment `f`, to `page`, its
/// slot after those of its fragment.
fn insert(page: &mut Vec<u8>, (key, value): (&[u8], &[u8]), f: u32) -> Result<()> {
    let shape @ (count, slots) = layout(page)?;
    let place = range(page, shape, f)?.1;
    let offset = u16::try_from(slots).map_err(|_| corrupt("its heap is past a u16 offset"))?;
    let mut bytes = Vec::with_capacity(key.len().saturating_add(value.len()).saturating_add(4));
    bytes.extend_from_slice(&u16::try_from(key.len()).unwrap_or(u16::MAX).to_le_bytes());
    bytes.extend_from_slice(key);
    bytes.extend_from_slice(&u16::try_from(value.len()).unwrap_or(u16::MAX).to_le_bytes());
    bytes.extend_from_slice(value);
    let grown = bytes.len();
    page.splice(slots..slots, bytes);
    let mut new_slot = f.to_le_bytes().to_vec();
    new_slot.extend_from_slice(&offset.to_le_bytes());
    let at = slots.saturating_add(grown).saturating_add(place.saturating_mul(SLOT));
    page.splice(at..at, new_slot);
    let count = u16::try_from(count.saturating_add(1)).map_err(|_| corrupt("it holds too many entries"))?;
    page.get_mut(..2).ok_or_else(|| corrupt("it has no header"))?.copy_from_slice(&count.to_le_bytes());
    Ok(())
}

/// Adds the entry of `key` and `value`, of fragment `f`, to `page` if it
/// fits in `capacity` bytes; whether it did.
pub(super) fn append(page: &mut Vec<u8>, (key, value): (&[u8], &[u8]), (f, capacity): (u32, usize)) -> Result<bool> {
    let count = u16_at(page, 0)?;
    if page.len().saturating_add(size(key, value)) > capacity || count == usize::from(u16::MAX) {
        return Ok(false);
    }
    insert(page, (key, value), f)?;
    Ok(true)
}

/// Removes from `page` the first entry of `key` and `value`, of fragment
/// `f`; whether there was one.
pub(super) fn remove(page: &mut Vec<u8>, (key, value): (&[u8], &[u8]), f: u32) -> Result<bool> {
    let shape @ (count, slots) = layout(page)?;
    let (from, to) = range(page, shape, f)?;
    let mut found = None;
    for n in from..to {
        let offset = slot(page, slots, n)?.1;
        let (k, v) = entry_at(page, offset, slots)?;
        if page.get(k.0..k.1) == Some(key) && page.get(v.0..v.1) == Some(value) && found.is_none_or(|(_, o, _)| offset < o) {
            found = Some((n, offset, v.1));
        }
    }
    let Some((n, offset, end)) = found else { return Ok(false) };
    let gone = end.saturating_sub(offset);
    let at = slots.saturating_add(n.saturating_mul(SLOT));
    page.drain(at..at.saturating_add(SLOT));
    page.drain(offset..end);
    let slots = slots.saturating_sub(gone);
    for s in 0..count.saturating_sub(1) {
        let field = slots.saturating_add(s.saturating_mul(SLOT)).saturating_add(4);
        let o = u16_at(page, field)?;
        if o > offset {
            let moved = u16::try_from(o.saturating_sub(gone)).unwrap_or(0);
            page.get_mut(field..field.saturating_add(2))
                .ok_or_else(|| corrupt("a slot is cut short"))?
                .copy_from_slice(&moved.to_le_bytes());
        }
    }
    let count = u16::try_from(count.saturating_sub(1)).unwrap_or(0);
    page.get_mut(..2).ok_or_else(|| corrupt("it has no header"))?.copy_from_slice(&count.to_le_bytes());
    Ok(true)
}
