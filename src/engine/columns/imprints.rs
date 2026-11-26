//! \[DESIGN\] Column imprints of a frozen group's chunks (RULES.md D511),
//! as Sidirourgos and Kersten, "Column Imprints: A Secondary Index
//! Structure" (SIGMOD 2013), describe them: of a column of exact numbers,
//! datetimes or character strings, the group's values cut into at most
//! `BINS` bins of about as many rows each, by edges drawn from a sorted
//! sample (an equi-depth histogram), and of each `BLOCK` rows of a chunk a
//! word, a bit set for each bin a value of them falls in. A condition of
//! the column makes a mask of the bins it may match, by the same test the
//! zone maps take of each bin's bounds; a block whose word shares no bit
//! with it holds no row the condition matches, and is skipped. A null
//! sets no bit, so a condition true of a null is not judged so.
//!
//! A column whose blocks fall in many bins each, its values scattered,
//! gets none: of a narrow condition, most blocks would hold a bin it may
//! match, and the few skipped would cut the scan's runs short, costing
//! more than they save (`useful`). Of the rest, a scan skips only gaps of
//! at least `GAP` rows (`bridged`), so its runs stay long.
//!
//! The words are kept on pages of their own, those of a part of
//! `PART_ROWS` rows together, as many parts' as a page holds, read only by
//! a scan whose condition can use them; the edges in the group's header.

use std::borrow::Borrow;
use std::cmp::Ordering;

use super::group::{Column, Filter, Group, PART_ROWS, Pages, Seal, Zone};
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::storage::{Conjunct, Conjunction};
use crate::value::{DataType, Value, compare};

/// Runs of a group's rows, each its first position and one past its last.
pub type Runs = Vec<(u32, u32)>;

/// \[DESIGN\] The bins of a column: the bits of a word.
pub const BINS: usize = 64;
/// \[DESIGN\] The rows of a block a word stands for: a cache line of
/// 8-byte values, and the block the decoders decode at once (RULES.md
/// D352).
pub const BLOCK: u32 = 64;
/// \[DESIGN\] The most values the edges are drawn from, taken evenly
/// apart, so a group's histogram costs a sort of a sample, not of its
/// rows.
const SAMPLE: usize = 4096;
/// Bytes of a word.
const WORD_BYTES: usize = 8;
/// \[DESIGN\] The most bins a column's blocks fall in on average for it
/// to get imprints: a quarter of them.
const MOST_SET: usize = BINS >> 2_u32;
/// \[DESIGN\] The fewest rows between two runs a scan reads apart: four
/// blocks. A shorter gap is read, as a run cut short costs more to start
/// than its rows cost to judge.
pub const GAP: u32 = 4 * BLOCK;
/// The words of a part, of `PART_ROWS` rows.
const PART_WORDS: usize = 16;

/// Whether a column of type `t` gets imprints: exact numbers, datetimes
/// and character strings, whose values `compare` orders totally.
pub fn imprinted(t: &DataType) -> bool {
    match t {
        DataType::Exact { .. }
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Character { .. }
        | DataType::Varying { .. } => true,
        DataType::Distinct(d) => imprinted(&d.source),
        DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Boolean
        | DataType::Interval(_)
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Row(_)
        | DataType::Json
        | DataType::Extension(_)
        | DataType::Xml(_) => false,
    }
}

/// `a` against `b`, a value `compare` cannot order taken as equal.
fn order(a: &Value, b: &Value) -> Ordering {
    compare(a, b).unwrap_or(Ordering::Equal)
}

/// The edges of `values`' bins, ascending and apart, at most `BINS` - 1:
/// the greatest value of each bin but the last. A value is in the bin of
/// the first edge not below it, or the last bin if none is. Of a column of
/// no more distinct values than bins, each value is an edge, so each has
/// a bin of its own.
pub fn edges<'v>(values: impl ExactSizeIterator<Item = &'v Value>) -> Vec<Value> {
    let step = values.len().div_ceil(SAMPLE).max(1);
    of_sample(values.step_by(step).collect())
}

/// As `edges`, of the values of `column`, only those sampled read.
fn edges_in<C: Column + ?Sized>(column: &C) -> Vec<Value> {
    let step = column.len().div_ceil(SAMPLE).max(1);
    of_sample((0..column.len()).step_by(step).map(|n| column.value(n)).collect())
}

/// The edges of the bins of `sample`, values taken evenly apart.
fn of_sample<V: Borrow<Value>>(mut sample: Vec<V>) -> Vec<Value> {
    sample.retain(|v| !v.borrow().is_null());
    sample.sort_by(|a, b| order(a.borrow(), b.borrow()));
    sample.dedup_by(|a, b| order((*a).borrow(), (*b).borrow()) == Ordering::Equal);
    let wanted = BINS.saturating_sub(1);
    if sample.len() <= wanted {
        return sample.iter().map(|v| v.borrow().clone()).collect();
    }
    let mut out: Vec<Value> = Vec::with_capacity(wanted);
    for n in 1..=wanted {
        let at = n.saturating_mul(sample.len()).checked_div(BINS).unwrap_or(0);
        if let Some(v) = sample.get(at).map(Borrow::borrow).filter(|v| out.last().is_none_or(|last| order(last, v) == Ordering::Less)) {
            out.push(v.clone());
        }
    }
    out
}

/// The bin of `v` by `edges`; none for the null value.
fn bin(edges: &[Value], v: &Value) -> Option<usize> {
    (!v.is_null()).then(|| edges.partition_point(|e| order(e, v) == Ordering::Less))
}

/// Whether imprints of the values of `column` by `edges` pay: their
/// blocks fall in no more than `MOST_SET` bins on average.
pub fn useful<C: Column + ?Sized>(edges: &[Value], column: &C) -> bool {
    let words = words(edges, (0..column.len()).map(|n| column.value(n)));
    let set: usize = words.iter().map(|w| usize::try_from(w.count_ones()).unwrap_or(BINS)).sum();
    set <= MOST_SET.saturating_mul(words.len())
}

/// `runs`, in order and apart, those less than `GAP` rows apart joined.
pub fn bridged(runs: &[(u32, u32)]) -> Runs {
    let mut out: Runs = Vec::with_capacity(runs.len());
    for &(from, to) in runs {
        if let Some(last) = out.last_mut().filter(|last| from.saturating_sub(last.1) < GAP) {
            last.1 = to;
        } else {
            out.push((from, to));
        }
    }
    out
}

/// The words of `values`, a part's, a word of each `BLOCK` of them.
pub fn words<V: Borrow<Value>>(edges: &[Value], values: impl Iterator<Item = V>) -> Vec<u64> {
    let mut out = Vec::new();
    let (mut word, mut held) = (0_u64, 0_u32);
    for v in values {
        if let Some(b) = bin(edges, v.borrow()).and_then(|b| u32::try_from(b).ok()) {
            word |= 1_u64.checked_shl(b).unwrap_or(0);
        }
        held = held.saturating_add(1);
        if held == BLOCK {
            out.push(word);
            (word, held) = (0, 0);
        }
    }
    if held > 0 {
        out.push(word);
    }
    out
}

/// The mask of the bins of a column, by `edges`, whose values lie within
/// `whole`, its least and greatest, that `conjunct` of it may match; none
/// if it is true of a null, or the column has no imprints.
pub fn mask(edges: &[Value], whole: Option<&(Value, Value)>, (column, conjunct): (usize, &Conjunct)) -> Option<u64> {
    let (least, greatest) = whole?;
    if edges.is_empty() || conjunct.matches_key(&[&Value::Null]) {
        return None;
    }
    let mut bounds: Vec<Option<(Value, Value)>> = vec![None; column.saturating_add(1)];
    let mut mask = 0_u64;
    for b in 0..=edges.len() {
        let low = b.checked_sub(1).and_then(|p| edges.get(p)).unwrap_or(least);
        let high = edges.get(b).unwrap_or(greatest);
        if let Some(slot) = bounds.get_mut(column) {
            *slot = Some((low.clone(), high.clone()));
        }
        if conjunct.may_match(&bounds) {
            mask |= 1_u64.checked_shl(u32::try_from(b).unwrap_or(u32::MAX)).unwrap_or(0);
        }
    }
    Some(mask)
}

/// Writes the words of each part of the chunks `zones`, of the values of
/// `column`, by `edges`, each part's `PART_WORDS` words long, on pages of
/// their own, as many as a page holds, and puts where each is in its part.
///
/// # Errors
///
/// As `Pages::put`.
pub fn put_words<S: Seal, C: Column + ?Sized>(
    pages: &Pages<'_, S>,
    (edges, column): (&[Value], &C),
    zones: &mut [Zone],
    written: &mut Vec<PageId>,
) -> Result<()> {
    let mut each = Vec::new();
    for (from, to) in zones.iter().flat_map(parts_of) {
        let held = held_in(column, (from, to));
        let mut bytes: Vec<u8> = words(edges, held.map(|n| column.value(n))).iter().flat_map(|w| w.to_le_bytes()).collect();
        bytes.resize(PART_WORDS.saturating_mul(WORD_BYTES), 0);
        each.push(bytes);
    }
    let mut places = super::bloom::write_filters(pages, &each, written)?.into_iter();
    for part in zones.iter_mut().flat_map(|z| z.parts.iter_mut()) {
        part.imprint = places.next();
    }
    Ok(())
}

/// The rows from `from` to `to` of `column`, none if it holds not all of
/// them.
pub(super) fn held_in<C: Column + ?Sized>(column: &C, (from, to): (u32, u32)) -> std::ops::Range<usize> {
    let (from, to) = (usize::try_from(from).unwrap_or(usize::MAX), usize::try_from(to).unwrap_or(0));
    if from > to || to > column.len() { 0..0 } else { from..to }
}

/// The runs of the parts of `zone`, in order.
pub(super) fn parts_of(zone: &Zone) -> impl Iterator<Item = (u32, u32)> + '_ {
    let end = zone.run.first.saturating_add(zone.run.rows);
    (0..zone.parts.len()).map(move |n| {
        let from = zone.run.first.saturating_add(u32::try_from(n).unwrap_or(u32::MAX).saturating_mul(PART_ROWS));
        (from, from.saturating_add(PART_ROWS).min(end))
    })
}

/// Of the runs `within` of `group`'s rows, those whose blocks' words
/// share a bit with `conjunct`'s mask, at the grain of `BLOCK` rows; none
/// if `conjunct` cannot be judged by imprints, its column has none, or
/// it reads more than one column. Only the words of parts `within`
/// touches are read, each page once.
///
/// # Errors
///
/// As `Pages::with`.
pub fn kept<S: Seal>(group: &Group, pages: &Pages<'_, S>, conjunct: &Conjunct, within: &[(u32, u32)]) -> Result<Option<Vec<(u32, u32)>>> {
    let columns = conjunct.columns();
    let [column] = columns.as_slice() else { return Ok(None) };
    let (Some(edges), Some(zones)) = (group.edges.get(*column), group.columns.get(*column)) else { return Ok(None) };
    let Some(mask) = mask(edges, whole(zones).as_ref(), (*column, conjunct)) else { return Ok(None) };
    let touched: Vec<((u32, u32), Option<Filter>)> =
        zones.iter().flat_map(|z| parts_of(z).zip(z.parts.iter().map(|p| p.imprint))).filter(|&(run, _)| among(within, run)).collect();
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for same in touched.chunk_by(|a, b| a.1.map(|f| f.page) == b.1.map(|f| f.page)) {
        let Some(page) = same.first().and_then(|p| p.1).map(|f| f.page) else {
            for &(run, _) in same {
                push(&mut runs, run);
            }
            continue;
        };
        pages.with(page, |bytes| {
            for &(run, place) in same {
                blocks(bytes, (run, place), mask, &mut runs);
            }
            Ok(())
        })?;
    }
    Ok(Some(super::scan::both(within, &runs)))
}

/// Whether the run `(from, to)` overlaps one of `within`.
fn among(within: &[(u32, u32)], (from, to): (u32, u32)) -> bool {
    within.iter().any(|&(a, b)| a < to && from < b)
}

/// The least and greatest values of the chunks `zones`, of a column.
pub(super) fn whole(zones: &[Zone]) -> Option<(Value, Value)> {
    let mut out: Option<(Value, Value)> = None;
    for (least, greatest) in zones.iter().filter_map(|z| z.bounds.as_ref()) {
        out = Some(match out {
            None => (least.clone(), greatest.clone()),
            Some((l, g)) => (
                if order(least, &l) == Ordering::Less { least.clone() } else { l },
                if order(greatest, &g) == Ordering::Greater { greatest.clone() } else { g },
            ),
        });
    }
    out
}

/// Pushes to `runs` the blocks of the part of rows `run`, whose words are
/// at `place` of page content `bytes`, that share a bit with `mask`; every
/// row of it if its words are not there.
fn blocks(bytes: &[u8], ((from, to), place): ((u32, u32), Option<Filter>), mask: u64, runs: &mut Vec<(u32, u32)>) {
    let words = place.and_then(|f| {
        let at = usize::try_from(f.at).ok()?;
        bytes.get(at..at.saturating_add(usize::try_from(f.len).ok()?))
    });
    let Some(words) = words else {
        push(runs, (from, to));
        return;
    };
    for (n, word) in words.as_chunks::<WORD_BYTES>().0.iter().enumerate() {
        let start = from.saturating_add(u32::try_from(n).unwrap_or(u32::MAX).saturating_mul(BLOCK));
        if start >= to {
            break;
        }
        if u64::from_le_bytes(*word) & mask != 0 {
            push(runs, (start, start.saturating_add(BLOCK).min(to)));
        }
    }
}

/// Pushes the run `(from, to)` to `runs`, joined to the last if it goes on
/// from it.
fn push(runs: &mut Vec<(u32, u32)>, (from, to): (u32, u32)) {
    if let Some(last) = runs.last_mut().filter(|last| last.1 == from) {
        last.1 = to;
    } else {
        runs.push((from, to));
    }
}

/// The conjuncts of `conditions`, each with the runs of `group`'s rows it
/// alone may keep within `within`, by imprints where it can be judged by
/// them; the conjuncts it cannot are left out.
///
/// # Errors
///
/// As `Pages::with`.
pub fn each_kept<S: Seal>(
    group: &Group,
    pages: &Pages<'_, S>,
    conditions: &Conjunction,
    within: &[(u32, u32)],
) -> Result<Vec<(usize, Runs)>> {
    let mut out = Vec::new();
    for (n, conjunct) in conditions.conjuncts().iter().enumerate() {
        if let Some(runs) = kept(group, pages, conjunct, within)? {
            out.push((n, runs));
        }
    }
    Ok(out)
}

/// Writes the imprints of each column of `types` that takes them, of
/// its values `columns`, if `pages` says groups have them, in its chunks
/// `chunks`: of each column, the edges of its bins, none if it has none.
pub(super) fn put_all<S: Seal, C: Column>(
    pages: &Pages<'_, S>,
    (types, columns): (&[DataType], &[C]),
    chunks: &mut [Vec<Zone>],
    written: &mut Vec<PageId>,
) -> Result<Vec<Vec<Value>>> {
    let mut out = Vec::with_capacity(types.len());
    for ((t, zones), column) in types.iter().zip(chunks.iter_mut()).zip(columns) {
        if !pages.imprints || !imprinted(t) {
            out.push(Vec::new());
            continue;
        }
        let mut edges = edges_in(column);
        // A column scattered so its blocks fall in many bins gets none.
        if !useful(&edges, column) {
            edges.clear();
        }
        if !edges.is_empty() {
            put_words(pages, (&edges, column), zones, written)?;
        }
        out.push(edges);
    }
    Ok(out)
}

// Unit tests: bins, words and masks; no matching row left out of scattered,
// sorted and skewed values; conjuncts ordered narrowest first (RULES.md
// R-TEST-1).
#[cfg(test)]
mod tests;
