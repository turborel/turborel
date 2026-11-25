//! \[DESIGN\] A chunk's values of 8 bytes as integers (RULES.md D31,
//! D352), an exact number's units or an approximate number's bits: a
//! frame of reference's differences unpacked a block at a time with its
//! least added (`packed/blocks.rs`), a whole byte width read with a loop
//! of its own, a dictionary's codes unpacked so and gathered from its
//! entries read once, runs, decimals and fixed fields each as theirs.

use super::super::packed::{get, unpack_words};
use super::{Body, FRAMED, WORD, between, integer};
use crate::kernels::Kernels;

impl Body<'_> {
    /// \[DESIGN\] The values at positions `from` to `to`, each 8 bytes, an
    /// exact number's units or an approximate number's bits, as integers
    /// pushed to `out`, a null's of no meaning (`Ints`, `Floats`): none if
    /// the fields are not of 8 bytes, or texts coded. Of a dictionary, its
    /// entries as integers are `entries` if given (`prepared.rs`), else
    /// read.
    pub(in crate::engine::columns) fn ints(
        &self,
        (from, to): (usize, usize),
        out: &mut Vec<i64>,
        (entries, kernels): (Option<&[i64]>, &dyn Kernels),
    ) -> Option<()> {
        let count = to.checked_sub(from)?;
        match self {
            Body::Frame { least, bits, deltas } => unframed(*least, (*bits, deltas), (from, to), (out, kernels)),
            Body::Stepped(stepped) => stepped.ints((from, to), out, kernels),
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
            Body::Constant { field } => {
                // None if every value is null, its field empty.
                out.extend(std::iter::repeat_n(integer(field).unwrap_or(0), count));
                Some(())
            }
            Body::Fixed { .. } | Body::Varying { .. } | Body::Fsst(_) => None,
        }
    }

    /// \[DESIGN\] (RULES.md D360) The value at position `n`, of 8 bytes,
    /// as `ints` gives it, read alone: of a frame of reference or a
    /// dictionary, its bits alone unpacked; of runs, its run found. For
    /// positions too few of a span to unpack it whole (`packed::DENSE_SPAN`).
    pub(in crate::engine::columns) fn int_at(&self, n: usize, entries: Option<&[i64]>) -> Option<i64> {
        match self {
            Body::Frame { least, bits, deltas } => get(deltas, *bits, n).map(|d| least.wrapping_add(i64::from_le_bytes(d.to_le_bytes()))),
            Body::Stepped(stepped) => stepped.int_at(n),
            Body::Runs(runs) => runs.field(n).map(i64::from_le_bytes),
            Body::Alp(alp) => alp.field(n).map(i64::from_le_bytes),
            Body::Dictionary { bits, codes, .. } => {
                let code = usize::try_from(get(codes, *bits, n)?).ok()?;
                match entries {
                    Some(words) => words.get(code).copied(),
                    None => self.entry_words()?.get(code).copied(),
                }
            }
            Body::Fixed { width: FRAMED, values } => {
                values.get(n.checked_mul(FRAMED)?..n.checked_add(1)?.checked_mul(FRAMED)?).and_then(integer)
            }
            // None if every value is null, its field empty.
            Body::Constant { field } => Some(integer(field).unwrap_or(0)),
            Body::Fixed { .. } | Body::Varying { .. } | Body::Fsst(_) => None,
        }
    }

    /// Of a dictionary, each entry's 8 bytes as an integer, in order: none
    /// if it is not a dictionary, or an entry is not of 8 bytes.
    pub(in crate::engine::columns) fn entry_words(&self) -> Option<Vec<i64>> {
        let Body::Dictionary { offsets, entries, .. } = self else { return None };
        let count = offsets.len().checked_div(WORD)?.checked_sub(1)?;
        (0..count).map(|e| between(offsets, e).and_then(|(a, b)| entries.get(a..b)).and_then(integer)).collect()
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
pub(super) fn unframed(
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
