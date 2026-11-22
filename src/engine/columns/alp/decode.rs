//! \[DESIGN\] Reading a chunk of approximate numbers as decimals (RULES.md
//! D353): the n of a range unpacked a block at a time with the frame's
//! least added (`packed/blocks.rs`), each made its value by its chunk's
//! scale, then the exceptions among them put in their places. An n of
//! magnitude below 2^51 is made an `f64` by adding its bits to those of
//! 2^52 + 2^51 and taking that number away, exact and with no branch.

use super::super::packed::{get, packed_len, unpack_words};
use super::super::{WORD, corrupt, read_word};
use super::{EXCEPTION, FIELD, Scale, exact, exceptions_of, scale_of};
use crate::error::Result;
use crate::kernels::Kernels;

/// The magnitude every n the kernel makes an `f64` by the magic number
/// is below: 2^51 (`Kernels::alp_decode`).
const MAGIC_SPAN: i128 = 1 << 51;

/// `v`'s bits, as a signed integer, as a vector of approximate numbers
/// holds them.
fn word_of(v: f64) -> i64 {
    i64::from_le_bytes(v.to_bits().to_le_bytes())
}

/// A chunk of approximate numbers as decimals, read in place.
#[derive(Debug, Clone, Copy)]
pub(in crate::engine::columns) struct Alp<'a> {
    scale: Scale,
    least: i64,
    bits: u32,
    /// Its exceptions, each a position and its bits, by position.
    exceptions: &'a [u8],
    deltas: &'a [u8],
}

impl<'a> Alp<'a> {
    /// The chunk of `count` values `rest` holds.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is too short, or of factors or differences too
    /// large.
    pub(in crate::engine::columns) fn parse(rest: &'a [u8], count: usize) -> Result<Alp<'a>> {
        let short = || corrupt("it is too short");
        let (scale, at) = scale_of(rest)?;
        let (exceptions, after) = exceptions_of(rest, at)?;
        let bits = u32::from(*rest.get(after).ok_or_else(short)?);
        if bits > u64::BITS {
            return Err(corrupt("its differences are wider than 64 bits"));
        }
        let least_at = after.saturating_add(1);
        let least = rest
            .get(least_at..least_at.saturating_add(FIELD))
            .and_then(|b| <[u8; FIELD]>::try_from(b).ok())
            .map(i64::from_le_bytes)
            .ok_or_else(short)?;
        let deltas_at = least_at.saturating_add(FIELD);
        let deltas_len = packed_len(count, bits).ok_or_else(|| corrupt("its values are too long"))?;
        let deltas = rest.get(deltas_at..deltas_at.saturating_add(deltas_len)).ok_or_else(short)?;
        Ok(Alp { scale, least, bits, exceptions, deltas })
    }

    /// Whether every n of the chunk, its least plus a difference of its
    /// bits, is of magnitude below 2^51, so made an `f64` by the kernel.
    fn magic_holds(&self) -> bool {
        let greatest = i128::from(self.least).saturating_add(i128::from(super::super::packed::mask(self.bits)));
        -MAGIC_SPAN < i128::from(self.least) && greatest < MAGIC_SPAN
    }

    /// The values at positions `from` to `to`, each its bits as a signed
    /// integer pushed to `out`: the n unpacked a block at a time, each made
    /// its value, then the exceptions among them put in their places.
    pub(in crate::engine::columns) fn words(&self, (from, to): (usize, usize), out: &mut Vec<i64>, kernels: &dyn Kernels) -> Option<()> {
        let start = out.len();
        let scale = self.scale;
        let ok = if self.magic_holds() {
            unpack_words(self.deltas, (self.bits, self.least), (from, to), out)?;
            kernels.alp_decode(out.get_mut(start..)?, scale);
            true
        } else {
            // n may be past 2^53, or its sum overflow, in a malformed
            // chunk: each difference added checked, each n made exactly.
            unpack_words(self.deltas, (self.bits, 0), (from, to), out)?;
            let mut ok = true;
            for w in out.get_mut(start..)? {
                let v = (*w >= 0).then(|| self.least.checked_add(*w)).flatten().and_then(exact).map(|n| scale.of(n));
                ok = ok && v.is_some();
                *w = word_of(v.unwrap_or(0.0_f64));
            }
            ok
        };
        self.patch((from, to), start, out)?;
        ok.then_some(())
    }

    /// Puts each exception among positions `from` to `to` in its place of
    /// `out`, whose value of `from` is at `start`.
    fn patch(&self, (from, to): (usize, usize), start: usize, out: &mut [i64]) -> Option<()> {
        let entries = self.exceptions.as_chunks::<EXCEPTION>().0;
        let position = |e: &[u8; EXCEPTION]| read_word(e, 0).unwrap_or(usize::MAX);
        for e in entries.iter().skip(entries.partition_point(|e| position(e) < from)).take_while(|e| position(e) < to) {
            let bits = <[u8; FIELD]>::try_from(e.get(WORD..)?).ok()?;
            *out.get_mut(start.checked_add(position(e).checked_sub(from)?)?)? = i64::from_le_bytes(bits);
        }
        Some(())
    }

    /// The `n`th value, as its field's bytes.
    pub(in crate::engine::columns) fn field(&self, n: usize) -> Option<[u8; FIELD]> {
        let entries = self.exceptions.as_chunks::<EXCEPTION>().0;
        let position = |e: &[u8; EXCEPTION]| read_word(e, 0).unwrap_or(usize::MAX);
        if let Ok(at) = entries.binary_search_by_key(&n, position) {
            return entries.get(at)?.get(WORD..)?.try_into().ok();
        }
        let delta = i64::try_from(get(self.deltas, self.bits, n)?).ok()?;
        let v = self.scale.of(exact(self.least.checked_add(delta)?)?);
        Some(v.to_bits().to_le_bytes())
    }
}
