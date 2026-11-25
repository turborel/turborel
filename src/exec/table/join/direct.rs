//! \[DESIGN\] (RULES.md D601) A join's table of keys dense in their span:
//! of one key column of whole numbers whose span, the greatest less the
//! least and one, is at most `DENSE_SPAN` times the rows built, the rows
//! of each key are found by their key's place in the span, no hash made,
//! no tag read, no slot walked, as Pedreira et al. (VLDB 2022, §4.5.2)
//! index an array by a key of a small range, and Boncz, Zukowski and Nes
//! (CIDR 2005, §4.1.2) join a key to a dense row number by its place.
//! The rows are put by a counting sort of their keys, which keeps the
//! rows of each key in the order they were built.
//!
//! A key is given as its number's nearest `f64`, as a join's keys are
//! compared by it (`keys.rs`), the bits of it with -0 as 0. A whole
//! number below 2^51 in magnitude is its `f64` exactly, so the place of a
//! key is exact, and a probe's number of a fraction, or out of the span,
//! finds no row. The rows a probe finds are those of its key's number,
//! their keys compared then, as a hashed table's candidates are.
//!
//! On a key whose nulls match, as a join a subquery is unnested into
//! matches its outer references by IS NOT DISTINCT FROM, a null is given
//! as `NULL_NUMBER`, a not-a-number no key's place is, and its rows are
//! kept apart, in the order built.

use super::placed::summed;

/// \[DESIGN\] (RULES.md D601) The widest span of the keys of a direct
/// table, a multiple of its rows: its places of each key in the span, 4
/// bytes each, and its rows, 4 bytes each, take no more than the 32 bytes
/// a row of a hashed table costs (`TABLE_ROW_BYTES`).
pub(in crate::exec) const DENSE_SPAN: usize = 4;

/// \[DESIGN\] 1.5 × 2^52, to which a whole number below 2^51 is added to
/// read it from a double's bits.
const MAGIC: f64 = 6_755_399_441_055_744.0_f64;

/// \[DESIGN\] 2^51, below which a whole number is its `f64` exactly.
const EXACT: f64 = 2_251_799_813_685_248.0_f64;

/// \[DESIGN\] What a null is given as to a direct table, on a key whose
/// nulls match: a quiet not-a-number, of no place in a span. A probe's
/// not-a-number of these bits finds the null rows, and its key, compared
/// then, matches none of them.
pub(super) const NULL_NUMBER: u64 = 0x7FF8_0000_0000_0E11;

/// The rows of a join's built operand by their keys' places in their
/// span.
#[derive(Debug, Default)]
pub(super) struct Dense {
    /// The least key.
    least: i64,
    /// Its parts, each the keys whose places share their high bits: one,
    /// or as many as a table built by many threads has (RULES.md D1080).
    parts: Vec<DensePart>,
    /// How far a place is turned to give its part.
    within: u32,
    /// The bits of a place that place it within its part.
    mask: usize,
    /// The built rows of a null key whose nulls match, in the order built.
    nulls: Vec<u32>,
}

/// The rows of the keys of a direct table whose places share their high
/// bits (RULES.md D1080): every row of a table one thread built.
#[derive(Debug, Default)]
pub(in crate::exec) struct DensePart {
    /// Of each place of the part and one past it, where its key's rows
    /// begin among `rows`.
    starts: Vec<u32>,
    /// The built rows, those of each key after the key before's, each in
    /// the order built.
    rows: Vec<u32>,
}

/// \[DESIGN\] (RULES.md D1080) What the numbers of some of a build's
/// keys say of a direct table: the least and the greatest, how many, and
/// whether each is a whole number below 2^51 in magnitude. Those of two
/// runs of rows say together what the numbers of both would.
#[derive(Debug, Clone, Copy)]
pub(in crate::exec) struct Span {
    least: i64,
    most: i64,
    keyed: usize,
    whole: bool,
}

impl Span {
    /// Of the keys whose `numbers` are given, the bits of each one's
    /// nearest `f64`, but those `misses` marks and those of a null.
    pub(in crate::exec) fn of(numbers: &[u64], misses: &[bool]) -> Span {
        let mut span = Span { least: i64::MAX, most: i64::MIN, keyed: 0, whole: true };
        for (&bits, _) in numbers.iter().zip(misses).filter(|&(&bits, m)| !*m && bits != NULL_NUMBER) {
            let Some(key) = whole(bits) else {
                span.whole = false;
                continue;
            };
            span = Span { least: span.least.min(key), most: span.most.max(key), keyed: span.keyed.saturating_add(1), whole: span.whole };
        }
        span
    }

    /// Of its keys and `other`'s.
    #[must_use]
    pub(in crate::exec) fn with(self, other: Span) -> Span {
        Span {
            least: self.least.min(other.least),
            most: self.most.max(other.most),
            keyed: self.keyed.saturating_add(other.keyed),
            whole: self.whole && other.whole,
        }
    }

    /// The least key and the places of the span, if its keys make a
    /// direct table: each a whole number, their span at most `DENSE_SPAN`
    /// times their count.
    pub(super) fn dense(&self) -> Option<(i64, usize)> {
        let width = usize::try_from(self.most.checked_sub(self.least)?).ok()?.checked_add(1)?;
        if !self.whole || self.keyed == 0 || width > self.keyed.saturating_mul(DENSE_SPAN) {
            return None;
        }
        Some((self.least, width))
    }
}

/// The place in a span from `least` of the key whose number's bits are
/// `bits`; none of a null's, or of a number not whole or below `least`.
pub(super) fn place_of(bits: u64, least: i64) -> Option<usize> {
    if bits == NULL_NUMBER {
        return None;
    }
    whole(bits).and_then(|key| key.checked_sub(least)).and_then(|p| usize::try_from(p).ok())
}

impl DensePart {
    /// The part of `places` places of the rows `built` gives each time
    /// it is asked, each with its key's place in the span, `count` of
    /// them, `at` of a place its place among the part's: put by a
    /// counting sort, the rows of a key in the order given.
    pub(super) fn of<I: Iterator<Item = (usize, u64)>>(
        built: impl Fn() -> I,
        (places, count): (usize, usize),
        at: impl Fn(u64) -> usize,
    ) -> DensePart {
        let mut ends = vec![0_usize; places.saturating_add(1)];
        for (_, place) in built() {
            if let Some(end) = ends.get_mut(at(place).saturating_add(1)) {
                *end = end.saturating_add(1);
            }
        }
        summed(&mut ends);
        let mut next = ends.clone();
        let mut rows = vec![0_u32; count];
        for (r, place) in built() {
            let Some(to) = next.get_mut(at(place)) else { continue };
            if let Some(row) = rows.get_mut(*to) {
                *row = u32::try_from(r).unwrap_or(u32::MAX);
            }
            *to = to.saturating_add(1);
        }
        DensePart { starts: ends.into_iter().map(|e| u32::try_from(e).unwrap_or(u32::MAX)).collect(), rows }
    }
}

impl Dense {
    /// The table of the rows whose keys' `numbers` are given, the bits of
    /// each one's nearest `f64`, each row's number its place among them,
    /// but those `misses` marks, and those `NULL_NUMBER` gives kept apart;
    /// none unless each other is a whole number below 2^51 in magnitude
    /// and their span is at most `DENSE_SPAN` times their count, of rows a
    /// `u32` numbers.
    pub(super) fn of(numbers: &[u64], misses: &[bool]) -> Option<Dense> {
        u32::try_from(numbers.len()).ok()?;
        let (least, width) = Span::of(numbers, misses).dense()?;
        let held = || numbers.iter().zip(misses).enumerate().filter(|(_, (_, m))| !**m);
        let nulls = held().filter(|(_, (bits, _))| **bits == NULL_NUMBER).filter_map(|(r, _)| u32::try_from(r).ok()).collect();
        let built = || held().filter_map(|(r, (&bits, _))| place_of(bits, least).map(|p| (r, u64::try_from(p).unwrap_or(u64::MAX))));
        let part = DensePart::of(built, (width, built().count()), |place| usize::try_from(place).unwrap_or(usize::MAX));
        let within = usize::BITS.saturating_sub(width.saturating_sub(1).leading_zeros());
        Some(Dense::of_parts(least, vec![part], within, nulls))
    }

    /// \[DESIGN\] (RULES.md D1080) The table of `parts`, in order, each
    /// the keys whose places from `least` share the bits above their
    /// `within` lowest; the rows of a null key `nulls`.
    pub(super) fn of_parts(least: i64, parts: Vec<DensePart>, within: u32, nulls: Vec<u32>) -> Dense {
        Dense { least, parts, within, mask: super::parted::low_bits(within), nulls }
    }

    /// How many parts it was made in.
    #[cfg(test)]
    pub(super) fn parts(&self) -> usize {
        self.parts.len()
    }

    /// The rows of the key whose number's bits are `bits`, in the order
    /// they were built; none of a number not whole or out of the span;
    /// those of a null key, of `NULL_NUMBER`.
    pub(super) fn candidates(&self, bits: u64) -> &[u32] {
        if bits == NULL_NUMBER {
            return &self.nulls;
        }
        let Some(place) = place_of(bits, self.least) else { return &[] };
        self.at(place)
    }

    /// \[DESIGN\] (RULES.md D1083) The rows of the key that is the whole
    /// number `key`, in the order built: those `candidates` gives of its
    /// number's bits. A key past 2^51 in magnitude, which no built key is,
    /// lies out of the span, so it finds none here as there.
    pub(super) fn rows_of(&self, key: i64) -> &[u32] {
        let Some(place) = key.checked_sub(self.least).and_then(|p| usize::try_from(p).ok()) else { return &[] };
        self.at(place)
    }

    /// The rows of the key at `place` in the span; none past it.
    fn at(&self, place: usize) -> &[u32] {
        let Some(part) = self.parts.get(place.checked_shr(self.within).unwrap_or(0)) else { return &[] };
        let at = place & self.mask;
        let (Some(&from), Some(&to)) = (part.starts.get(at), part.starts.get(at.saturating_add(1))) else { return &[] };
        let span = usize::try_from(from).unwrap_or(usize::MAX)..usize::try_from(to).unwrap_or(0);
        part.rows.get(span).unwrap_or_default()
    }
}

/// The whole number whose `f64`'s bits are `bits`, if it is one below
/// 2^51 in magnitude: read from the bits of it added to `MAGIC`.
fn whole(bits: u64) -> Option<i64> {
    let f = f64::from_bits(bits);
    if f.fract() != 0.0_f64 || f.abs() >= EXACT {
        return None;
    }
    Some((f + MAGIC).to_bits().wrapping_sub(MAGIC.to_bits()).cast_signed())
}

// Unit tests: spans at their bound, negative and fractional keys, build
// order within a key (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
