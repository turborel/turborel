//! \[DESIGN\] (RULES.md D369) A hash join's runtime filter: made of the
//! keys of its built rows once its right operand is held, it drops the
//! rows of its left operand no built row can match before the join reads
//! them, as Zhu, Potti, Saurabh and Patel (PVLDB 2017) push a build side's
//! Bloom filter into the probe side's scan, and as Lang et al. (SIGMOD
//! 2016) skip a block by its least and greatest values. Of one key of the
//! join: the least and greatest of an exact key's built values, a range
//! the scan is told (`range_conjuncts`), so a store skips its groups and
//! parts outside it; and a blocked Bloom filter of the keys' hashes, as
//! the join hashes them (`join/keys.rs`), applied to each batch the scan
//! gives (`kept`), before any column given late is decoded.
//!
//! Of an exact key of a build of no more than `SPANS_MAX_ROWS` rows, its
//! distinct values too (RULES.md D600), as Qiao, Boncz and Zhang's RPT+
//! (PVLDB 2026) cascade a range, then a key set, then a Bloom filter: of
//! no more than `KEYSET_MAX` of them, each a span the scan is told and a
//! set a vector of exact numbers finds each row's key in exactly, at any
//! scale, in place of the Bloom filter, which judges a vector of others;
//! of more, the spans between the `SPANS - 1` widest gaps among them, so
//! a store skips a group or part that lies in a gap.
//!
//! A row it drops is one the join drops: its key outside the range, or
//! not in the filter, matches no built key, and a null matches none but
//! on a key whose nulls match, of which no range is made and whose
//! filter holds a null's hash if a built key is null. It never raises an
//! error. A filter dropping few rows costs a hash a row for nothing, so
//! one that kept more than `KEEP_MOST` of the rows of its first `TRIED`
//! batches stops judging them, as Behm et al. (SIGMOD 2022) stop a
//! runtime filter that does not pay.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::hash::Seed;
use super::join::{Hashing, KeyHashes};
use crate::error::Result;
use crate::index::Probe;
use crate::kernels::Kernels;
use crate::kernels::portable::bloom::{BLOCK_WORDS, bits_of, block_of};
use crate::storage::{Batch, Column};
use crate::storage::{Condition, Conjunct};
use crate::value::{CompOp, Decimal, Value};

/// \[DESIGN\] The most built rows a Bloom filter is made of: its bits, 8 a
/// key, then 4 MiB, charged to the join's work area beside its table.
pub(in crate::exec) const BLOOM_MAX_ROWS: usize = 4 << 20;

/// \[DESIGN\] Bits of a Bloom filter for each key it holds: about 3% of
/// the keys it does not hold pass it, of 4 bits a key in blocks of 512.
const BITS_PER_KEY: usize = 8;

/// \[DESIGN\] (RULES.md D600) The most distinct keys a filter holds as a
/// set: a row's found by halving in a dozen steps, about a Bloom
/// filter's hash and read.
pub(in crate::exec) const KEYSET_MAX: usize = 4_096;

/// \[DESIGN\] (RULES.md D600) The most spans of more keys a scan is told:
/// enough to skip what lies in their widest gaps, few enough that a zone
/// finds its span in six steps.
const SPANS: usize = 64;

/// \[DESIGN\] (RULES.md D600) The most built rows whose keys are sorted
/// for a key set or spans, a sort that costs little beside the build.
const SPANS_MAX_ROWS: usize = 1 << 20;

/// \[DESIGN\] The batches a filter judges before it may stop.
pub(in crate::exec) const TRIED: usize = 16;

/// \[DESIGN\] The share of the rows of its first `TRIED` batches, in
/// tenths, above which a filter stops judging them: 9 in 10.
pub(in crate::exec) const KEEP_MOST: usize = 9;

/// \[DESIGN\] Tenths, of `KEEP_MOST`.
const TENTHS: usize = 10;

/// A runtime filter of a hash join's key.
#[derive(Debug)]
pub(in crate::exec) struct RuntimeFilter {
    /// Whether a null of the key matches a null.
    nulls: bool,
    /// The least and greatest of an exact key's built values, as units of
    /// their scale.
    range: Option<(i64, i64, u32)>,
    /// The distinct values of an exact key, ascending, as units of the
    /// range's scale, of no more than `KEYSET_MAX`.
    keys: Option<Vec<i64>>,
    /// The spans of an exact key's values the scan is told, as units of
    /// the range's scale, ascending and apart.
    spans: Option<Vec<(i64, i64)>>,
    /// The words of a blocked Bloom filter of the keys' hashes, if made.
    bloom: Option<Vec<u64>>,
    seed: Seed,
    /// Whether texts are hashed by SipHash (RULES.md D369), as the join's
    /// table hashed them.
    keyed: bool,
    /// The rows of its first batches judged, and those it kept.
    judged: AtomicUsize,
    kept: AtomicUsize,
    batches: AtomicUsize,
}

impl RuntimeFilter {
    /// The filter of the key of `built`'s column `at`, whose nulls match
    /// as `nulls` says, its rows' hashes `hashes` as the join made them,
    /// hashed by `how`: none if it would be neither range nor Bloom filter.
    pub(in crate::exec) fn of_built(
        built: &Batch,
        (at, nulls): (usize, bool),
        hashes: &KeyHashes,
        how: Hashing<'_>,
    ) -> Option<RuntimeFilter> {
        let range = built.columns.get(at).and_then(|c| if nulls { None } else { range_of(c) });
        let held = hashes.misses.iter().filter(|&&m| !m).count();
        let (keys, spans) = if let (Some(_), Some(Column::Ints(ints)), true) = (range, built.columns.get(at), built.rows <= SPANS_MAX_ROWS)
        {
            keyset_of(ints)
        } else {
            (None, None)
        };
        let bloom = (held <= BLOOM_MAX_ROWS).then(|| bloom_of(hashes, held));
        if range.is_none() && bloom.is_none() {
            return None;
        }
        let (seed, keyed) = (how.seed.clone(), how.keyed);
        Some(RuntimeFilter {
            nulls,
            range,
            keys,
            spans,
            bloom,
            seed,
            keyed,
            judged: AtomicUsize::new(0),
            kept: AtomicUsize::new(0),
            batches: AtomicUsize::new(0),
        })
    }

    /// The conjuncts a scan of column `column` is told of its range: the
    /// key at least its least and at most its greatest; and its spans, if
    /// it has them (RULES.md D600).
    pub(in crate::exec) fn range_conjuncts(&self, column: usize) -> Vec<Conjunct> {
        let Some((least, most, scale)) = self.range else { return Vec::new() };
        let exact = |units: i64| Value::Exact(Decimal::new(i128::from(units), scale));
        let bound =
            |op, units: i64| Conjunct::Condition(Condition { columns: vec![column], probe: Probe::Compare { op, value: exact(units) } });
        let mut told = vec![bound(CompOp::Ge, least), bound(CompOp::Le, most)];
        if let Some(spans) = &self.spans {
            told.push(Conjunct::Spans { column, spans: spans.iter().map(|&(a, b)| (exact(a), exact(b))).collect() });
        }
        told
    }

    /// Whether it judges each row past its range: by its Bloom filter, of
    /// no more than `BLOOM_MAX_ROWS` built rows, or its key set.
    pub(in crate::exec) fn blooms(&self) -> bool {
        self.bloom.is_some() || self.keys.is_some()
    }

    /// Whether it still judges rows: not once its first `TRIED` batches
    /// kept more than `KEEP_MOST` tenths of theirs.
    pub(in crate::exec) fn judging(&self) -> bool {
        if self.batches.load(Ordering::Relaxed) < TRIED {
            return true;
        }
        let (judged, kept) = (self.judged.load(Ordering::Relaxed), self.kept.load(Ordering::Relaxed));
        kept.saturating_mul(TENTHS) <= judged.saturating_mul(KEEP_MOST)
    }

    /// The rows of `batch` at `rows`, or of all, whose key, of its column
    /// at `at`, the filter may hold, in order: of an exact key, those in
    /// its range, if `range` asks; then, if `bloom` asks, those its key
    /// set holds, of a vector of exact numbers, else those its Bloom
    /// filter holds.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value nests deeper than a type may.
    pub(in crate::exec) fn kept(
        &self,
        (batch, at): (&Batch, usize),
        (rows, (range, bloom)): (Option<&[u32]>, (bool, bool)),
        kernels: &dyn Kernels,
    ) -> Result<Vec<u32>> {
        let given: Vec<usize> = crate::storage::sel::places_of(rows.map(|r| crate::storage::Sel::of(r.to_vec())), batch.rows);
        let ranged: Vec<usize> = if let (Some((least, most, scale)), Some(Column::Ints(ints)), true) =
            (self.range, batch.columns.get(at), range)
            && ints.scale == scale
        {
            given.into_iter().filter(|&r| ints.units.get(r).is_some_and(|u| (least..=most).contains(u)) && !ints.is_null(r)).collect()
        } else {
            given
        };
        let set = self.keys.as_ref().filter(|_| bloom).zip(self.range).zip(batch.columns.get(at));
        if let Some(((keys, (_, _, scale)), Column::Ints(ints))) = set {
            let held = |u: i64| at_scale(u, ints.scale, scale).is_some_and(|k| keys.binary_search(&k).is_ok());
            let kept: Vec<usize> =
                ranged.into_iter().filter(|&r| !ints.is_null(r) && ints.units.get(r).is_some_and(|&u| held(u))).collect();
            self.count((rows.map_or(batch.rows, <[u32]>::len), kept.len()));
            return Ok(crate::storage::sel::rows_of(kept));
        }
        let kept = match self.bloom.as_ref().filter(|_| bloom) {
            Some(words) => {
                let mut hashes = KeyHashes::default();
                hashes.of_batch((batch, Some(&ranged)), (&[at], &[self.nulls]), Hashing { seed: &self.seed, keyed: self.keyed })?;
                let mut held = Vec::with_capacity(ranged.len());
                kernels.bloom_probe(words, &hashes.hashes, &mut held);
                let held = held.into_iter().map(crate::storage::mask::index).filter(|&n| hashes.misses.get(n).is_some_and(|m| !m));
                held.filter_map(|n| ranged.get(n).copied()).collect::<Vec<usize>>()
            }
            None => ranged,
        };
        self.count((rows.map_or(batch.rows, <[u32]>::len), kept.len()));
        Ok(crate::storage::sel::rows_of(kept))
    }

    /// Counts a batch judged, of `judged` rows of which it kept `kept`.
    fn count(&self, (judged, kept): (usize, usize)) {
        if self.batches.fetch_add(1, Ordering::Relaxed) < TRIED {
            self.judged.fetch_add(judged, Ordering::Relaxed);
            self.kept.fetch_add(kept, Ordering::Relaxed);
        }
    }
}

/// The least and greatest of the integers of `column`, not null, and
/// their scale, if it is a vector of integers with a row not null.
fn range_of(column: &Column) -> Option<(i64, i64, u32)> {
    let Column::Ints(ints) = column else { return None };
    let held = ints.units.iter().enumerate().filter(|&(r, _)| !ints.is_null(r)).map(|(_, &u)| u);
    let (least, most) = held.fold(None, |m: Option<(i64, i64)>, u| Some(m.map_or((u, u), |(l, h)| (l.min(u), h.max(u)))))?;
    Some((least, most, ints.scale))
}

/// The units `units` of scale `from` as units of scale `to`, if a value
/// of that scale is equal to them.
fn at_scale(units: i64, from: u32, to: u32) -> Option<i64> {
    let ten = |d: u32| 10_i64.checked_pow(d);
    if from > to {
        let by = ten(from.saturating_sub(to))?;
        (units.checked_rem(by) == Some(0)).then(|| units.checked_div(by)).flatten()
    } else {
        units.checked_mul(ten(to.saturating_sub(from))?)
    }
}

/// A filter's key set, if any, and the spans a scan is told, if any.
type KeySet = (Option<Vec<i64>>, Option<Vec<(i64, i64)>>);

/// The distinct values of `ints` not null, ascending, if no more than
/// `KEYSET_MAX`; and the spans a scan is told of them: each a span, or of
/// more, the spans between the `SPANS - 1` widest gaps of more than one
/// unit, none if there is no such gap (RULES.md D600).
fn keyset_of(ints: &crate::storage::Ints) -> KeySet {
    let mut keys: Vec<i64> = ints.units.iter().enumerate().filter(|&(r, _)| !ints.is_null(r)).map(|(_, &u)| u).collect();
    keys.sort_unstable();
    keys.dedup();
    if keys.len() <= KEYSET_MAX {
        let spans = keys.iter().map(|&k| (k, k)).collect();
        return (Some(keys), Some(spans));
    }
    let mut gaps: Vec<(i64, usize)> = keys
        .windows(2)
        .enumerate()
        .filter_map(|(n, w)| w.first().zip(w.get(1)).map(|(a, b)| (b.saturating_sub(*a), n)))
        .filter(|&(g, _)| g > 1)
        .collect();
    if gaps.is_empty() {
        return (None, None);
    }
    gaps.sort_unstable_by(|a, b| b.cmp(a));
    gaps.truncate(SPANS.saturating_sub(1));
    let mut cuts: Vec<usize> = gaps.into_iter().map(|(_, n)| n).collect();
    cuts.sort_unstable();
    let (mut spans, mut start) = (Vec::with_capacity(cuts.len().saturating_add(1)), 0);
    for cut in cuts {
        spans.extend(keys.get(start).zip(keys.get(cut)).map(|(&a, &b)| (a, b)));
        start = cut.saturating_add(1);
    }
    spans.extend(keys.get(start).zip(keys.last()).map(|(&a, &b)| (a, b)));
    (None, Some(spans))
}

/// A blocked Bloom filter of the hashes of `hashes` not misses, `held` of
/// them: blocks of 512 bits, `BITS_PER_KEY` bits a key, at least one.
fn bloom_of(hashes: &KeyHashes, held: usize) -> Vec<u64> {
    let blocks = held.saturating_mul(BITS_PER_KEY).div_ceil(BLOCK_WORDS.saturating_mul(u64::BITS.try_into().unwrap_or(1))).max(1);
    let mut words = vec![0_u64; blocks.saturating_mul(BLOCK_WORDS)];
    for (&h, _) in hashes.hashes.iter().zip(&hashes.misses).filter(|(_, m)| !**m) {
        let base = block_of(h, blocks).saturating_mul(BLOCK_WORDS);
        for (w, bit) in bits_of(h) {
            if let Some(word) = words.get_mut(base.saturating_add(w)) {
                *word |= bit;
            }
        }
    }
    words
}

// Unit tests: a filter's rows and its stop (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernels::Portable;
    use crate::storage::Ints;

    /// The filter of the built keys `keys`, integers, none of whose nulls
    /// match.
    fn filter(keys: &[i64]) -> RuntimeFilter {
        let seed = Seed::fixed(0x5EED);
        let how = Hashing { seed: &seed, keyed: false };
        let built = Batch { columns: vec![Column::Ints(Ints::new(keys.to_vec(), 0, None))], rows: keys.len() };
        let mut hashes = KeyHashes::default();
        hashes.of_batch((&built, None), (&[0], &[false]), how).unwrap();
        RuntimeFilter::of_built(&built, (0, false), &hashes, how).unwrap()
    }

    /// The rows of a batch of `probes` at a scale of 2 the filter keeps.
    fn kept(filter: &RuntimeFilter, probes: &[i64]) -> Vec<u32> {
        let batch = Batch { columns: vec![Column::Ints(Ints::new(probes.to_vec(), 2, None))], rows: probes.len() };
        filter.kept((&batch, 0), (None, (true, true)), &Portable).unwrap()
    }

    /// Verifies: [DESIGN] RULES.md D369: a filter keeps every row whose key
    /// a built row holds, 7.00 of a built 7 among them, and drops a key of
    /// the range none holds, but for the Bloom filter's few false hits.
    #[test]
    fn a_filter_keeps_every_built_key() {
        let built: Vec<i64> = (0..1_000).map(|k| k * 3).collect();
        let f = filter(&built);
        let probes: Vec<i64> = (0..3_000).map(|k| k * 100).collect();
        let kept = kept(&f, &probes);
        assert!(built.iter().all(|&k| kept.contains(&u32::try_from(k).unwrap())), "every built key kept");
        assert!(kept.len() < 1_100, "few of the 2,000 keys not built kept, found {}", kept.len());
        assert_eq!(f.range_conjuncts(4).len(), 3, "the range and the spans of its keys");
    }

    /// Verifies: [DESIGN] RULES.md D369 (MC/DC): a filter judges its first
    /// `TRIED` batches, then goes on only if they kept no more than nine in
    /// ten of their rows.
    #[test]
    fn a_filter_that_keeps_most_rows_stops() {
        let built: Vec<i64> = (0..100).collect();
        let (all, few) = (filter(&built), filter(&built));
        let every: Vec<i64> = (0..100).map(|k| k * 100).collect();
        let none: Vec<i64> = (1_000..1_100).map(|k| k * 100).collect();
        for _ in 0..TRIED {
            assert!(all.judging() && few.judging());
            kept(&all, &every);
            kept(&few, &none);
        }
        assert!(!all.judging(), "every row kept");
        assert!(few.judging(), "no row kept");
    }

    /// Verifies: [DESIGN] RULES.md D600 (MC/DC): a build of few keys is a
    /// set, each key a span, judged exactly, with no Bloom filter; one of
    /// more keys with gaps is spans cut at its widest gaps, every key in
    /// one; one of no gap wider than a unit has none.
    #[test]
    fn few_keys_are_a_set_and_more_are_spans() {
        let few = filter(&[9, 3, 3, 700]);
        assert_eq!(few.keys.as_deref(), Some(&[3_i64, 9, 700][..]));
        assert!(few.blooms());
        assert_eq!(kept(&few, &[300, 400, 900, 70_000, 70_001]), vec![0, 2, 3], "exactly the keys at another scale");
        assert_eq!(few.range_conjuncts(0).len(), 3, "the range and the spans");
        let most = i64::try_from(KEYSET_MAX).unwrap();
        let wide: Vec<i64> = (0..most + 100).map(|k| k * 10 - k % 50 * 9).collect();
        let (keys, spans) = keyset_of(&Ints::new(wide.clone(), 0, None));
        let spans = spans.unwrap();
        assert!(keys.is_none() && spans.len() == SPANS, "{} spans", spans.len());
        assert!(wide.iter().all(|k| spans.iter().any(|(a, b)| (a..=b).contains(&k))), "every key in a span");
        assert!(spans.iter().zip(spans.iter().skip(1)).all(|(a, b)| a.1 < b.0), "ascending and apart");
        let dense: Vec<i64> = (0..=most).collect();
        assert_eq!(keyset_of(&Ints::new(dense, 0, None)), (None, None));
    }
}
