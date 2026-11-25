//! \[DESIGN\] A frame of reference with a common divisor (RULES.md
//! D1098): each value of 8 bytes as a signed integer, less the chunk's
//! least, divided by the greatest number that divides every such
//! difference, then bit-packed. Values a fixed step apart take the bits
//! of their count of steps, not of their span: a DATE, kept as its
//! instant's microseconds, is a whole number of days past the least, and
//! takes the bits of the days its chunk spans; a timestamp of whole
//! seconds takes 20 bits fewer than its microseconds.
//!
//! ```text
//! STEPPED:     bits u8 | least i64 | step u64 | count × bits, bit-packed,
//!              each value less the least, divided by the step, a null's
//!              zeroed
//! ```
//!
//! A difference is below 2^64 and is the step times its quotient, so the
//! product and the sum wrap to the value written, whatever its sign: the
//! least and greatest integers of 64 bits in one chunk are read back as
//! written. The sizer offers it only where the step is 2 or more, and it
//! is taken only where it is smaller than every other encoding.

use super::super::packed::{get, pack, packed_len};
use super::words::unframed;
use super::{FRAMED, corrupt, integer, slice};
use crate::error::Result;
use crate::kernels::Kernels;

/// The bytes before the packed quotients: the bits, the least, the step.
pub(super) const HEAD: usize = 1 + FRAMED + FRAMED;

/// The greatest number dividing both `a` and `b`; the other if one is 0.
pub(super) fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a.checked_rem(b).unwrap_or(0));
    }
    a
}

/// A chunk's values of a fixed step apart, read in place.
pub(in crate::engine::columns) struct Stepped<'a> {
    least: i64,
    step: u64,
    bits: u32,
    quotients: &'a [u8],
}

/// The value of quotient `q` of a chunk of `least` and `step`.
fn value_of(least: i64, step: u64, q: u64) -> i64 {
    least.wrapping_add_unsigned(q.wrapping_mul(step))
}

/// Appends `fields`, of 8 bytes each, as steps of `step` from `least`,
/// each count of steps in `bits`, bit-packed.
pub(super) fn encode(fields: &[Option<Vec<u8>>], (least, step, bits): (i64, u64, u32), out: &mut Vec<u8>) {
    out.push(u8::try_from(bits).unwrap_or(u8::MAX));
    out.extend_from_slice(&least.to_le_bytes());
    out.extend_from_slice(&step.to_le_bytes());
    let quotients = fields.iter().map(|field| {
        let n = field.as_deref().and_then(integer).unwrap_or(least);
        n.abs_diff(least).checked_div(step).unwrap_or(0)
    });
    pack(quotients, bits, out);
}

impl<'a> Stepped<'a> {
    /// The chunk of `count` values `rest` holds.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is too short, its quotients wider than 64 bits,
    /// or its step 0.
    pub(super) fn parse(rest: &'a [u8], count: usize) -> Result<Stepped<'a>> {
        let bits = u32::from(*rest.first().ok_or_else(|| corrupt("it is too short"))?);
        if bits > u64::BITS {
            return Err(corrupt("its steps are wider than 64 bits"));
        }
        let least = integer(slice(rest, 1, FRAMED)?).ok_or_else(|| corrupt("it is too short"))?;
        let step = integer(slice(rest, 1 + FRAMED, FRAMED)?).map(i64::cast_unsigned).filter(|&s| s != 0);
        let step = step.ok_or_else(|| corrupt("its step is none"))?;
        let quotients = slice(rest, HEAD, packed_len(count, bits).ok_or_else(|| corrupt("its values are too long"))?)?;
        Ok(Stepped { least, step, bits, quotients })
    }

    /// Its `n`th value, none past its end.
    pub(in crate::engine::columns) fn int_at(&self, n: usize) -> Option<i64> {
        get(self.quotients, self.bits, n).map(|q| value_of(self.least, self.step, q))
    }

    /// Its `n`th value as the 8 bytes of its field.
    pub(super) fn field(&self, n: usize) -> Option<[u8; FRAMED]> {
        self.int_at(n).map(i64::to_le_bytes)
    }

    /// The values at positions `from` to `to` pushed to `out`: the
    /// quotients unpacked as a frame's differences are, then each made
    /// its value in place.
    pub(in crate::engine::columns) fn ints(&self, (from, to): (usize, usize), out: &mut Vec<i64>, kernels: &dyn Kernels) -> Option<()> {
        let start = out.len();
        unframed(0, (self.bits, self.quotients), (from, to), (out, kernels))?;
        for slot in out.get_mut(start..)? {
            *slot = value_of(self.least, self.step, slot.cast_unsigned());
        }
        Some(())
    }
}
