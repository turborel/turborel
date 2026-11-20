//! \[DESIGN\] Approximate numbers as decimals (RULES.md D31, D353), after
//! ALP (Afroozeh, Kuffó and Boncz, "ALP: Adaptive Lossless floating-Point
//! Compression", SIGMOD 2023): of a chunk of approximate numbers, each the
//! integer n with n × 10^f × 10^-e the same number, bit for bit, kept as a
//! frame of reference of n bit-packed (`packed.rs`), and each other one,
//! an exception, kept as it is. Most numbers written as decimals of a few
//! digits, as prices or measures are, are such: 12.34 is 1234 × 10^-2.
//! The factors e and f, f at most e, are the pair of the first `SAMPLE`
//! values that leaves fewest exceptions, then takes fewest bits, then the
//! least e, then the least f. A value is read by two multiplications, as
//! the paper decodes, never a division; the encoder keeps as a decimal
//! only a value those two multiplications give back exactly, so a value
//! read is the one written.
//!
//! ```text
//! ALP:  e | FACTORED u8 | f u8 | exceptions u32 | each: position u32 |
//!       its bits u64 | bits u8 | least i64 | count × bits, bit-packed,
//!       each n less the least, an exception's and a null's 0
//! ```
//!
//! A chunk written before the factors were, its first byte without
//! `FACTORED` and no byte of f, is `e u8` and the rest as above, each
//! value n / 10^e, and is read so, by division (`Scale::Divided`).

mod decode;

use super::packed::{bits_for, pack, packed_len};
use super::{WORD, corrupt, read_word, word};
use crate::error::Result;

pub(super) use decode::Alp;

/// The bytes of an approximate number's field.
const FIELD: usize = 8;
/// The bytes of an exception: its position and its bits.
const EXCEPTION: usize = 12;
/// \[DESIGN\] The greatest exponent tried: 10^18, the greatest power of
/// ten below 2^63.
const MOST_E: usize = 18;
/// The exponents tried, 0 to `MOST_E`.
const EXPONENTS: usize = MOST_E + 1;
/// \[DESIGN\] The values the factors are chosen by, as ALP samples.
const SAMPLE: usize = 64;
/// The greatest n an `f64` holds exactly.
const EXACT: i64 = 1 << 53;
/// \[DESIGN\] (RULES.md D353) The bit of a chunk's first byte saying its
/// values are read by multiplication with the factors e and f, the byte
/// of f following: above every exponent, so a chunk written before it
/// reads as it was written.
const FACTORED: u8 = 0x80;

/// 10^e, for each e tried: each exact.
const POWERS: [f64; EXPONENTS] = [1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18];
/// 10^-e, for each e tried: each the nearest `f64`, as the paper's table.
const FRACTIONS: [f64; EXPONENTS] =
    [1e0, 1e-1, 1e-2, 1e-3, 1e-4, 1e-5, 1e-6, 1e-7, 1e-8, 1e-9, 1e-10, 1e-11, 1e-12, 1e-13, 1e-14, 1e-15, 1e-16, 1e-17, 1e-18];

/// A chunk's factors: its values are n × 10^f × 10^-e.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Factors {
    pub(super) e: u8,
    pub(super) f: u8,
}

/// How a chunk's n are made its values.
#[derive(Debug, Clone, Copy)]
pub(super) enum Scale {
    /// Multiplied by 10^f, then by 10^-e, as of a chunk with `FACTORED`.
    Factored(f64, f64),
    /// Divided by 10^e, as of a chunk written before the factors were.
    Divided(f64),
}

impl Scale {
    /// The value of `n`, an integer held exactly.
    pub(super) fn of(self, n: f64) -> f64 {
        match self {
            Scale::Factored(power, fraction) => n * power * fraction,
            Scale::Divided(power) => n / power,
        }
    }
}

impl Factors {
    /// Their scale; none if either is above `MOST_E`.
    fn scale(self) -> Option<Scale> {
        Some(Scale::Factored(*POWERS.get(usize::from(self.f))?, *FRACTIONS.get(usize::from(self.e))?))
    }
}

/// `n` as an `f64`, exactly, if its magnitude is at most 2^53.
pub(super) fn exact(n: i64) -> Option<f64> {
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

/// The n of `v` of factors `factors`, if n × 10^f × 10^-e is `v`, bit for
/// bit, multiplied as the decoder multiplies. n is v × 10^e × 10^-f
/// rounded, as the paper encodes.
fn encoded(v: f64, factors: Factors) -> Option<i64> {
    let (power, fraction) = (*POWERS.get(usize::from(factors.e))?, *FRACTIONS.get(usize::from(factors.f))?);
    let n = rounded(v * power * fraction)?;
    (factors.scale()?.of(exact(n)?).to_bits() == v.to_bits()).then_some(n)
}

/// The value of field `f`, an approximate number's bits.
fn number(f: &[u8]) -> Option<f64> {
    <[u8; FIELD]>::try_from(f).ok().map(|b| f64::from_bits(u64::from_le_bytes(b)))
}

/// Of a pair of factors, the values it holds, and their least and
/// greatest n.
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

/// Every pair of factors tried, e from 0 to `MOST_E` and f from 0 to e.
fn pairs() -> impl Iterator<Item = Factors> {
    (0..=MOST_E)
        .flat_map(|e| (0..=e).map(move |f| (e, f)))
        .filter_map(|(e, f)| Some(Factors { e: u8::try_from(e).ok()?, f: u8::try_from(f).ok()? }))
}

/// The pairs of factors tried of a chunk's values as they are added, then
/// the one chosen by the first `SAMPLE`.
#[derive(Debug, Clone)]
pub(super) struct Trials {
    by_pair: Vec<(Factors, Trial)>,
    /// The values not null added.
    seen: usize,
    chosen: Option<usize>,
}

impl Default for Trials {
    fn default() -> Self {
        Trials { by_pair: pairs().map(|p| (p, Trial::default())).collect(), seen: 0, chosen: None }
    }
}

impl Trials {
    /// Adds the value of field `f`, not null.
    pub(super) fn add(&mut self, f: &[u8]) {
        let Some(v) = number(f) else { return };
        self.seen = self.seen.saturating_add(1);
        if let Some(at) = self.chosen {
            if let Some((factors, trial)) = self.by_pair.get_mut(at)
                && let Some(n) = encoded(v, *factors)
            {
                trial.add(n);
            }
            return;
        }
        for (factors, trial) in &mut self.by_pair {
            if let Some(n) = encoded(v, *factors) {
                trial.add(n);
            }
        }
        if self.seen >= SAMPLE {
            self.chosen = self.best_pair();
        }
    }

    /// The pair that leaves fewest exceptions, then takes fewest bits,
    /// then is of the least e, then of the least f.
    fn best_pair(&self) -> Option<usize> {
        self.by_pair
            .iter()
            .enumerate()
            .max_by_key(|(_, (p, t))| (t.fit, std::cmp::Reverse(t.bits()), std::cmp::Reverse(p.e), std::cmp::Reverse(p.f)))
            .map(|(at, _)| at)
    }

    /// The factors, least n and bits of each difference, and the bytes of
    /// the body, of `count` values; none if every value is an exception.
    pub(super) fn best(&self, count: usize) -> Option<((Factors, i64, u32), usize)> {
        let (factors, trial) = self.by_pair.get(self.chosen.or_else(|| self.best_pair())?)?;
        let (least, _) = trial.range?;
        let exceptions = self.seen.saturating_sub(trial.fit);
        let bits = trial.bits();
        let bytes = (1 + 1 + WORD + 1 + FIELD)
            .saturating_add(exceptions.saturating_mul(EXCEPTION))
            .saturating_add(packed_len(count, bits).unwrap_or(usize::MAX));
        Some(((*factors, least, bits), bytes))
    }
}

/// Appends `fields`, approximate numbers, as decimals of factors
/// `factors`, each n less `least` in `bits`, the others exceptions.
pub(super) fn encode(fields: &[Option<Vec<u8>>], (factors, least, bits): (Factors, i64, u32), out: &mut Vec<u8>) -> Result<()> {
    if usize::from(factors.e) > MOST_E || factors.f > factors.e {
        return Err(corrupt("its exponent is too large"));
    }
    let mut exceptions = Vec::new();
    let mut deltas = Vec::with_capacity(fields.len());
    for (at, field) in fields.iter().enumerate() {
        let Some(f) = field else {
            deltas.push(0);
            continue;
        };
        let delta =
            number(f).and_then(|v| encoded(v, factors)).and_then(|n| u64::try_from(i128::from(n).saturating_sub(i128::from(least))).ok());
        if delta.is_none() {
            exceptions.push((at, f.as_slice()));
        }
        deltas.push(delta.unwrap_or(0));
    }
    out.push(factors.e | FACTORED);
    out.push(factors.f);
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

/// The scale a chunk's first bytes give, and where its count of
/// exceptions is: of factors if its first byte has `FACTORED`, else of
/// division, as written before the factors were.
fn scale_of(rest: &[u8]) -> Result<(Scale, usize)> {
    let short = || corrupt("it is too short");
    let large = || corrupt("its exponent is too large");
    let first = *rest.first().ok_or_else(short)?;
    if first & FACTORED == 0 {
        return Ok((Scale::Divided(*POWERS.get(usize::from(first)).ok_or_else(large)?), 1));
    }
    let factors = Factors { e: first & !FACTORED, f: *rest.get(1).ok_or_else(short)? };
    if factors.f > factors.e {
        return Err(large());
    }
    Ok((factors.scale().ok_or_else(large)?, 2))
}

/// The exceptions of a chunk whose count of them is at `at`, and where
/// what follows them is.
fn exceptions_of(rest: &[u8], at: usize) -> Result<(&[u8], usize)> {
    let many = read_word(rest, at)?;
    let from = at.saturating_add(WORD);
    let len = many.checked_mul(EXCEPTION).ok_or_else(|| corrupt("its exceptions are too many"))?;
    Ok((rest.get(from..from.saturating_add(len)).ok_or_else(|| corrupt("it is too short"))?, from.saturating_add(len)))
}

#[cfg(test)]
mod tests;
