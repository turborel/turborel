//! \[DESIGN\] Approximate numbers as decimals (RULES.md D31), after ALP
//! (Afroozeh, Kuffó and Boncz, "ALP: Adaptive Lossless floating-Point
//! Compression", SIGMOD 2023): of a chunk of approximate numbers, each the
//! integer n with n / 10^e the same number, bit for bit, kept as a frame of
//! reference of n bit-packed (`packed.rs`), and each other one, an
//! exception, kept as it is. Most numbers written as decimals of a few
//! digits, as prices or measures are, are such: 12.34 is 1234 / 10^2. The
//! exponent e is the one of the first `SAMPLE` values that leaves fewest
//! exceptions, then that takes fewest bits, then the least. Dividing by an
//! exact power of ten rounds correctly, so a value read is the one written.
//! Split from encoded.rs (RULES.md R-SIZE-2).
//!
//! ```text
//! ALP:  e u8 | exceptions u32 | each: position u32 | its bits u64 |
//!       bits u8 | least i64 | count × bits, bit-packed, each n less the
//!       least, an exception's and a null's 0
//! ```

use super::packed::{bits_for, get, pack, packed_len};
use super::{WORD, corrupt, read_word, word};
use crate::error::Result;

/// The bytes of an approximate number's field.
const FIELD: usize = 8;
/// The bytes of an exception: its position and its bits.
const EXCEPTION: usize = 12;
/// \[DESIGN\] The greatest exponent tried: 10^18, the greatest power of
/// ten below 2^63.
const MOST_E: usize = 18;
/// \[DESIGN\] The values the exponent is chosen by, as ALP samples.
const SAMPLE: usize = 64;
/// The greatest n an `f64` holds exactly, so n / 10^e is correctly rounded.
const EXACT: i64 = 1 << 53;

/// 10^e, for each e tried: each exact.
const POWERS: [f64; MOST_E + 1] = [1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18];

/// `n` as an `f64`, exactly, if its magnitude is at most 2^53.
fn exact(n: i64) -> Option<f64> {
    if !(-EXACT..=EXACT).contains(&n) {
        return None;
    }
    let (high, low) = (i32::try_from(n >> 32_u32).ok()?, u32::try_from(n & 0xffff_ffff).ok()?);
    Some(f64::from(high) * 4_294_967_296.0_f64 + f64::from(low))
}

/// `x` rounded to the nearest integer, if its magnitude is at most 2^53.
fn rounded(x: f64) -> Option<i64> {
    let r = x.round();
    if !r.is_finite() || r.abs() > 9_007_199_254_740_992.0_f64 {
        return None;
    }
    let bits = r.to_bits();
    let biased = i32::try_from((bits >> 52_u32) & 0x7ff).ok()?;
    if biased == 0_i32 {
        return Some(0);
    }
    // r is mantissa × 2^(biased - 1075), an integer, so the shift is exact:
    // left only of 2^53 itself.
    let mantissa = i64::try_from((bits & 0x000f_ffff_ffff_ffff) | (1_u64 << 52_u32)).ok()?;
    let magnitude = if biased > 1075_i32 {
        mantissa.checked_shl(u32::try_from(biased.checked_sub(1075)?).ok()?)?
    } else {
        mantissa.checked_shr(u32::try_from(1075_i32.checked_sub(biased)?).ok()?)?
    };
    Some(if r.is_sign_negative() { magnitude.checked_neg()? } else { magnitude })
}

/// The n of `v` at exponent `e`, if n / 10^e is `v`, bit for bit.
fn encoded(v: f64, e: usize) -> Option<i64> {
    let power = *POWERS.get(e)?;
    let n = rounded(v * power)?;
    (exact(n)? / power).to_bits().eq(&v.to_bits()).then_some(n)
}

/// The value of field `f`, an approximate number's bits.
fn number(f: &[u8]) -> Option<f64> {
    <[u8; FIELD]>::try_from(f).ok().map(|b| f64::from_bits(u64::from_le_bytes(b)))
}

/// Of an exponent, the values it holds, and their least and greatest n.
#[derive(Debug, Clone, Copy, Default)]
struct Trial {
    fit: usize,
    range: Option<(i64, i64)>,
}

impl Trial {
    fn add(&mut self, n: i64) {
        self.fit = self.fit.saturating_add(1);
        self.range = Some(self.range.map_or((n, n), |(lo, hi)| (lo.min(n), hi.max(n))));
    }

    fn bits(&self) -> u32 {
        self.range.map_or(0, |(lo, hi)| bits_for(u64::try_from(i128::from(hi).saturating_sub(i128::from(lo))).unwrap_or(u64::MAX)))
    }
}

/// The exponents tried of a chunk's values as they are added, then the
/// one chosen by the first `SAMPLE`.
#[derive(Debug, Clone)]
pub(super) struct Trials {
    by_e: [Trial; MOST_E + 1],
    /// The values not null added.
    seen: usize,
    chosen: Option<usize>,
}

impl Default for Trials {
    fn default() -> Self {
        Trials { by_e: [Trial::default(); MOST_E + 1], seen: 0, chosen: None }
    }
}

impl Trials {
    /// Adds the value of field `f`, not null.
    pub(super) fn add(&mut self, f: &[u8]) {
        let Some(v) = number(f) else { return };
        self.seen = self.seen.saturating_add(1);
        if let Some(e) = self.chosen {
            if let (Some(n), Some(trial)) = (encoded(v, e), self.by_e.get_mut(e)) {
                trial.add(n);
            }
            return;
        }
        for (e, trial) in self.by_e.iter_mut().enumerate() {
            if let Some(n) = encoded(v, e) {
                trial.add(n);
            }
        }
        if self.seen >= SAMPLE {
            self.chosen = self.best_e();
        }
    }

    /// The exponent that leaves fewest exceptions, then takes fewest bits,
    /// then is least.
    fn best_e(&self) -> Option<usize> {
        self.by_e.iter().enumerate().max_by_key(|(e, t)| (t.fit, std::cmp::Reverse(t.bits()), std::cmp::Reverse(*e))).map(|(e, _)| e)
    }

    /// The exponent, least n and bits of each difference, and the bytes of
    /// the body, of `count` values; none if every value is an exception.
    pub(super) fn best(&self, count: usize) -> Option<((u32, i64, u32), usize)> {
        let e = self.chosen.or_else(|| self.best_e())?;
        let trial = self.by_e.get(e)?;
        let (least, _) = trial.range?;
        let exceptions = self.seen.saturating_sub(trial.fit);
        let bits = trial.bits();
        let bytes = (1 + WORD + 1 + FIELD)
            .saturating_add(exceptions.saturating_mul(EXCEPTION))
            .saturating_add(packed_len(count, bits).unwrap_or(usize::MAX));
        Some(((u32::try_from(e).ok()?, least, bits), bytes))
    }
}

/// Appends `fields`, approximate numbers, as decimals of exponent `e`, each
/// n less `least` in `bits`, the others exceptions.
pub(super) fn encode(fields: &[Option<Vec<u8>>], (e, least, bits): (u32, i64, u32), out: &mut Vec<u8>) -> Result<()> {
    let e_at = usize::try_from(e).map_err(|_| corrupt("its exponent is too large"))?;
    let mut exceptions = Vec::new();
    let mut deltas = Vec::with_capacity(fields.len());
    for (at, field) in fields.iter().enumerate() {
        let Some(f) = field else {
            deltas.push(0);
            continue;
        };
        let delta =
            number(f).and_then(|v| encoded(v, e_at)).and_then(|n| u64::try_from(i128::from(n).saturating_sub(i128::from(least))).ok());
        if delta.is_none() {
            exceptions.push((at, f.as_slice()));
        }
        deltas.push(delta.unwrap_or(0));
    }
    out.push(u8::try_from(e).map_err(|_| corrupt("its exponent is too large"))?);
    out.extend_from_slice(&word(exceptions.len())?);
    for (at, f) in exceptions {
        out.extend_from_slice(&word(at)?);
        out.extend_from_slice(f);
    }
    out.push(u8::try_from(bits).unwrap_or(u8::MAX));
    out.extend_from_slice(&least.to_le_bytes());
    pack(deltas.into_iter(), bits, out);
    Ok(())
}

/// A chunk of approximate numbers as decimals, read in place.
#[derive(Debug, Clone, Copy)]
pub(super) struct Alp<'a> {
    power: f64,
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
    /// SQLCODE -916 if it is too short, or of an exponent or differences
    /// too large.
    pub(super) fn parse(rest: &'a [u8], count: usize) -> Result<Alp<'a>> {
        let short = || corrupt("it is too short");
        let power = *POWERS.get(usize::from(*rest.first().ok_or_else(short)?)).ok_or_else(|| corrupt("its exponent is too large"))?;
        let many = read_word(rest, 1)?;
        let at = 1 + WORD;
        let len = many.checked_mul(EXCEPTION).ok_or_else(|| corrupt("its exceptions are too many"))?;
        let exceptions = rest.get(at..at.saturating_add(len)).ok_or_else(short)?;
        let after = at.saturating_add(len);
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
        Ok(Alp { power, least, bits, exceptions, deltas })
    }

    /// The `n`th value, as its field's bytes.
    pub(super) fn field(&self, n: usize) -> Option<[u8; FIELD]> {
        let entries = self.exceptions.as_chunks::<EXCEPTION>().0;
        let position = |e: &[u8; EXCEPTION]| read_word(e, 0).unwrap_or(usize::MAX);
        if let Ok(at) = entries.binary_search_by_key(&n, position) {
            return entries.get(at)?.get(WORD..)?.try_into().ok();
        }
        let delta = i64::try_from(get(self.deltas, self.bits, n)?).ok()?;
        let v = exact(self.least.checked_add(delta)?)? / self.power;
        Some(v.to_bits().to_le_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D31, ALP: integers convert to and from
    /// approximate numbers exactly up to 2^53, negative ones too, and no
    /// further; a decimal of few digits is n / 10^e, a third one of 16,
    /// and -0, a number of 17 digits, an infinity are not of any
    /// exponent.
    #[test]
    fn numbers_are_decimals_exactly() {
        for n in [0_i64, 1, -1, 1234, -987_654_321, EXACT, -EXACT, (1_i64 << 40_i32) + 7] {
            let f = exact(n).unwrap();
            assert_eq!(rounded(f), Some(n), "{n}");
        }
        assert_eq!(exact(EXACT + 1), None);
        assert_eq!(rounded(9.1e15), None, "past 2^53");
        assert_eq!(rounded(-2.5), Some(-3), "away from zero, as f64::round");
        assert_eq!(encoded(12.34, 2), Some(1234));
        assert_eq!(encoded(12.34, 1), None);
        assert_eq!(encoded(-0.5, 1), Some(-5));
        assert_eq!(encoded(-0.0, 0), None, "-0 is kept as it is");
        assert_eq!(
            (0..=MOST_E).find_map(|e| encoded(1.0 / 3.0, e).map(|n| (e, n))),
            Some((16, 3_333_333_333_333_333)),
            "a third, of 16 digits"
        );
        assert_eq!((0..=MOST_E).find_map(|e| encoded(0.1 + 0.2, e)), None, "0.30000000000000004 needs 17");
        assert_eq!(encoded(f64::INFINITY, 0), None);
    }
}
