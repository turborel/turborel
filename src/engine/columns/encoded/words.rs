//! \[DESIGN\] A chunk's values of 8 bytes as integers (RULES.md D31,
//! D352), an exact number's units or an approximate number's bits: a
//! frame of reference's differences unpacked a block at a time with its
//! least added (`packed/blocks.rs`), a whole byte width read with a loop
//! of its own, a dictionary's codes unpacked so and gathered from its
//! entries read once, runs, decimals and fixed fields each as theirs.
//! Fields of 16 bytes, an exact number's units of a type of more digits
//! than 64 bits always hold, are integers too where 64 bits hold each
//! (RULES.md D1287).

use super::super::packed::{get, unpack_words};
use super::{Body, FRAMED, WORD, between, integer};
use crate::kernels::Kernels;

/// \[DESIGN\] (RULES.md D1287) The bytes of a field of an exact number of
/// a type of more digits than 64 bits always hold: its units in 128 bits
/// (`value/types/exact.rs`). No other type whose values are read as
/// integers writes a field so wide, as approximate numbers and datetimes
/// of no time zone take 8 bytes, so a field of this width read here is
/// such a number's.
const WIDE: usize = 16;

/// \[DESIGN\] (RULES.md D1287) The units a field holds, as an integer: of
/// 8 bytes, as they are; of 16, if 64 bits hold them. None of units past
/// 64 bits, which must not be cut to them, or of another width.
fn narrowed(field: &[u8]) -> Option<i64> {
    match <[u8; WIDE]>::try_from(field) {
        Ok(wide) => i64::try_from(i128::from_le_bytes(wide)).ok(),
        Err(_) => integer(field),
    }
}

/// \[DESIGN\] (RULES.md D1287) The one value of a chunk whose field is
/// `field`, as an integer: 0 of an empty field, every value null; none of
/// a field 64 bits do not hold (`narrowed`), which read as 0 would be a
/// wrong number.
pub(super) fn constant(field: &[u8]) -> Option<i64> {
    if field.is_empty() { Some(0) } else { narrowed(field) }
}

/// \[DESIGN\] (RULES.md D1287) The fields of 16 bytes of `given`, each its
/// low 8 bytes as an integer pushed to `out`: none if the high 8 bytes of
/// one are not its sign alone, so 64 bits do not hold it. Judged of all
/// at the end, with no branch a field, so the loop is vector code.
fn narrowed_all(given: &[u8], out: &mut Vec<i64>) -> Option<()> {
    let mut fit = true;
    out.extend(given.as_chunks::<FRAMED>().0.as_chunks::<2>().0.iter().map(|[low, high]| {
        let units = i64::from_le_bytes(*low);
        fit &= i64::from_le_bytes(*high) == units.wrapping_shr(SIGN_SHIFT);
        units
    }));
    fit.then_some(())
}

/// The bits a 64-bit integer is shifted right by to leave its sign in
/// every bit: all but one.
const SIGN_SHIFT: u32 = i64::BITS - 1;

impl Body<'_> {
    /// \[DESIGN\] The values at positions `from` to `to`, each 8 bytes, an
    /// exact number's units or an approximate number's bits, or 16 bytes
    /// of units 64 bits hold (RULES.md D1287), as integers pushed to
    /// `out`, a null's of no meaning (`Ints`, `Floats`): none if the
    /// fields are of another width, a field of 16 bytes passes 64 bits, or
    /// texts are coded. Of a dictionary, its entries as integers are
    /// `entries` if given (`prepared.rs`), else read.
    pub(in crate::engine::columns) fn ints(
        &self,
        (from, to): (usize, usize),
        out: &mut Vec<i64>,
        (entries, kernels): (Option<&[i64]>, &dyn Kernels),
    ) -> Option<()> {
        let count = to.checked_sub(from)?;
        match self {
            Body::Frame { least, bits, deltas } => unframed(*least, (*bits, deltas), (from, to), (out, kernels)),
            Body::Runs(runs) => runs.ints((from, to), out),
            Body::Alp(alp) => alp.words((from, to), out, kernels),
            Body::Dictionary { bits, codes, .. } => match entries {
                Some(words) => gathered(words, (*bits, codes), (from, to), out),
                None => gathered(&self.entry_words()?, (*bits, codes), (from, to), out),
            },
            Body::Fixed { width: FRAMED, values } => {
                let given = values.get(from.checked_mul(FRAMED)?..to.checked_mul(FRAMED)?)?;
                out.extend(given.as_chunks::<FRAMED>().0.iter().map(|&w| i64::from_le_bytes(w)));
                Some(())
            }
            Body::Fixed { width: WIDE, values } => narrowed_all(values.get(from.checked_mul(WIDE)?..to.checked_mul(WIDE)?)?, out),
            Body::Constant { field } => {
                out.extend(std::iter::repeat_n(constant(field)?, count));
                Some(())
            }
            Body::Fixed { .. } | Body::Varying { .. } | Body::Fsst(_) => None,
        }
    }

    /// \[DESIGN\] (RULES.md D360) The value at position `n`, of 8 bytes
    /// or of 16 that 64 bits hold, as `ints` gives it, read alone: of a frame of reference or a
    /// dictionary, its bits alone unpacked; of runs, its run found. For
    /// positions too few of a span to unpack it whole (`packed::DENSE_SPAN`).
    pub(in crate::engine::columns) fn int_at(&self, n: usize, entries: Option<&[i64]>) -> Option<i64> {
        match self {
            Body::Frame { least, bits, deltas } => get(deltas, *bits, n).map(|d| least.wrapping_add(i64::from_le_bytes(d.to_le_bytes()))),
            Body::Runs(runs) => runs.field(n).map(i64::from_le_bytes),
            Body::Alp(alp) => alp.field(n).map(i64::from_le_bytes),
            Body::Dictionary { bits, codes, .. } => {
                let code = usize::try_from(get(codes, *bits, n)?).ok()?;
                match entries {
                    Some(words) => words.get(code).copied(),
                    None => self.entry_words()?.get(code).copied(),
                }
            }
            // None of a width that is not an integer's (`narrowed`).
            Body::Fixed { width, values } => values.get(n.checked_mul(*width)?..n.checked_add(1)?.checked_mul(*width)?).and_then(narrowed),
            Body::Constant { field } => constant(field),
            Body::Varying { .. } | Body::Fsst(_) => None,
        }
    }

    /// Of a dictionary, each entry as an integer (`narrowed`), in order:
    /// none if it is not a dictionary, or an entry is not an integer's
    /// field, or passes 64 bits.
    pub(in crate::engine::columns) fn entry_words(&self) -> Option<Vec<i64>> {
        let Body::Dictionary { offsets, entries, .. } = self else { return None };
        let count = offsets.len().checked_div(WORD)?.checked_sub(1)?;
        (0..count).map(|e| between(offsets, e).and_then(|(a, b)| entries.get(a..b)).and_then(narrowed)).collect()
    }
}

/// The values at positions `from` to `to` of a dictionary whose codes of
/// `bits` are `codes`, each its entry of `words`, pushed to `out`: the
/// codes unpacked a block at a time, then each made its entry in place;
/// none if a code is of no entry.
fn gathered(words: &[i64], (bits, codes): (u32, &[u8]), (from, to): (usize, usize), out: &mut Vec<i64>) -> Option<()> {
    let start = out.len();
    unpack_words(codes, (bits, 0), (from, to), out)?;
    let mut ok = true;
    for slot in out.get_mut(start..)? {
        let word = usize::try_from(*slot).ok().and_then(|c| words.get(c)).copied();
        ok = ok && word.is_some();
        *slot = word.unwrap_or(0);
    }
    ok.then_some(())
}

/// The values at positions `from` to `to` of frame of reference `least`
/// whose differences, of `bits` each, are `deltas`, pushed to `out`. A
/// difference is less than 2^64, and the value it gives was an integer of
/// 64 bits, so adding it wrapping gives that value.
fn unframed(
    least: i64,
    (bits, deltas): (u32, &[u8]),
    (from, to): (usize, usize),
    (out, kernels): (&mut Vec<i64>, &dyn Kernels),
) -> Option<()> {
    let plus = |d: u64| least.wrapping_add(i64::from_le_bytes(d.to_le_bytes()));
    // Of the widths of 1, 2, 4 and 8 whole bytes, read as such, a loop
    // the compiler makes vector code of; of others, a block at a time.
    let bytes = match bits {
        0 => {
            out.extend(std::iter::repeat_n(least, to.checked_sub(from)?));
            return Some(());
        }
        8 | 16 | 32 | 64 => usize::try_from(bits.checked_div(8)?).ok()?,
        _ => return kernels.unpack(deltas, (bits, least), (from, to), out),
    };
    let given = deltas.get(from.checked_mul(bytes)?..to.checked_mul(bytes)?)?;
    match bytes {
        1 => out.extend(given.iter().map(|&b| plus(u64::from(b)))),
        2 => out.extend(given.as_chunks::<2>().0.iter().map(|&d| plus(u64::from(u16::from_le_bytes(d))))),
        4 => out.extend(given.as_chunks::<4>().0.iter().map(|&d| plus(u64::from(u32::from_le_bytes(d))))),
        _ => out.extend(given.as_chunks::<FRAMED>().0.iter().map(|&d| plus(u64::from_le_bytes(d)))),
    }
    Some(())
}
