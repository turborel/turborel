//! \[DESIGN\] (RULES.md D368, D369, D1288) The hashes of a join's keys, of
//! a batch's rows a vector at a time. A key's hash is of a word every two
//! values a join matches share. A number's word is the number itself if
//! it is a whole number 64 bits hold (`whole_word`), and else the bits of
//! its nearest `f64`, -0 as 0. Two numbers match by their algebraic value
//! (`value::compare_numbers`), so an exact number of any scale, a decimal
//! floating-point number and an approximate one that match have one word,
//! and whole numbers apart have words apart, where the nearest `f64` is
//! one for 1,024 of them near 2^63. A character string's hash is of its
//! text less its trailing spaces; a datetime's of its instant; a truth
//! value's of itself; a null's, on a key whose nulls match, a constant;
//! any other value's of its bucket's bytes (`value::key_bucket`). A row
//! whose key has no bucket, a null on a key whose nulls match nothing, or
//! a large object, matches no row and is marked a miss.
//!
//! A vector of integers is hashed without a value made: an integer of
//! fewer than 51 bits is its `f64` exactly by adding it to 1.5 × 2^52 in
//! the bits of a double, a wider one is its own word, and one of a scale
//! up to 22 below 2^51 is divided by 10 to it, both exact, so the quotient
//! is correctly rounded, the nearest `f64` a value's digits give. A vector
//! of texts coded by a dictionary hashes each entry once.
//!
//! Words are mixed by the hash of the grouping's table (`table/hash.rs`),
//! seeded at random as the database opens; a text by SipHash-1-3 keyed by
//! that seed instead, once a table's probes found many rows of one hash
//! and another key (RULES.md D369), as a text may be made to collide
//! under the hash of words whatever the seed.

use super::super::hash::{Seed, canonical};
use super::direct::NULL_NUMBER;
use crate::error::Result;
use crate::storage::{Batch, Column, Ints, Texts};
use crate::value::{Bucket, Datetime, DecFloat, Decimal, MAX_EXACT_PRECISION, Value, key_bucket, pow10};

/// \[DESIGN\] 1.5 × 2^52, to which an integer of fewer than 51 bits is
/// added in a double's bits to give its `f64` exactly.
const MAGIC: f64 = 6_755_399_441_055_744.0_f64;

/// \[DESIGN\] An integer below this in magnitude is its `f64` exactly by
/// `MAGIC`: 2^51.
const MAGIC_RANGE: i128 = 1_i128 << 51_u32;

/// \[DESIGN\] As `MAGIC_RANGE`, of a unit's magnitude.
const MAGIC_UNITS: u64 = 1_u64 << 51_u32;

/// \[DESIGN\] As `MAGIC_RANGE`, as an `f64`.
const MAGIC_RANGE_F64: f64 = 2_251_799_813_685_248.0_f64;

/// \[DESIGN\] (RULES.md D1288) The bits of an `f64`'s mantissa, which its
/// exponent's 11 bits and its sign follow (IEEE 754 binary64).
const MANTISSA_BITS: u32 = 52;

/// \[DESIGN\] (RULES.md D1288) The exponent's bits of an `f64` of 2^51 or
/// more and below 2^52, the least a whole number read from an `f64`'s
/// bits has (`whole_f64`): 1023, an exponent of 0, and 51.
const LEAST_EXPONENT: u64 = 1074;

/// \[DESIGN\] (RULES.md D1288) 2^63, the greatest magnitude of a whole
/// number 64 bits hold, as an `f64`.
const WORD_RANGE: f64 = f64::from_bits(WORD_RANGE_BITS);

/// The bits of 2^63 as an `f64`: an exponent of 63 over 1023, no mantissa.
const WORD_RANGE_BITS: u64 = 0x43E0_0000_0000_0000;

/// \[DESIGN\] The powers of 10 an `f64` holds exactly: up to 10^22.
const POWERS: [f64; 23] = [
    1e0_f64, 1e1_f64, 1e2_f64, 1e3_f64, 1e4_f64, 1e5_f64, 1e6_f64, 1e7_f64, 1e8_f64, 1e9_f64, 1e10_f64, 1e11_f64, 1e12_f64, 1e13_f64,
    1e14_f64, 1e15_f64, 1e16_f64, 1e17_f64, 1e18_f64, 1e19_f64, 1e20_f64, 1e21_f64, 1e22_f64,
];

/// How a join hashes its keys: by its database's seed, and whether a text
/// is hashed by SipHash keyed by it (RULES.md D369); and whether a number
/// is given as its word, the bits of its `f64` if it is a whole number
/// below 2^51, to a direct table, not hashed (RULES.md D601); and the
/// kernels a vector of integers is
/// hashed on, if given (RULES.md D1098).
#[derive(Clone, Copy)]
pub(in crate::exec) struct Hashing<'s> {
    pub(in crate::exec) seed: &'s Seed,
    pub(in crate::exec) keyed: bool,
    pub(in crate::exec) direct: bool,
    pub(in crate::exec) kernels: Option<&'s dyn crate::kernels::Kernels>,
}

/// The hashes of the keys of a batch's rows, and of each, whether it is a
/// miss, its key matching nothing.
#[derive(Debug, Default)]
pub(in crate::exec) struct KeyHashes {
    pub(in crate::exec) hashes: Vec<u64>,
    pub(in crate::exec) misses: Vec<bool>,
    /// A column's hashes, before they are joined to the key's.
    column: Vec<u64>,
}

impl KeyHashes {
    /// Hashes the keys of the rows of `batch` at `rows`, or of all, of
    /// its columns at `places`, of each key whether a null matches a null
    /// as `nulls` says.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value nests deeper than a type may.
    pub(in crate::exec) fn of_batch(
        &mut self,
        (batch, rows): (&Batch, Option<&[usize]>),
        (places, nulls): (&[usize], &[bool]),
        how: Hashing<'_>,
    ) -> Result<()> {
        let count = rows.map_or(batch.rows, <[usize]>::len);
        self.hashes.clear();
        self.misses.clear();
        // A batch of no rows may hold no vectors, as one joined of none.
        if count == 0 {
            return Ok(());
        }
        self.misses.resize(count, false);
        // A key of no column, of a join on none: every row of one hash.
        if places.is_empty() {
            self.hashes.resize(count, how.seed.null());
        }
        for (n, (&at, &matches_null)) in places.iter().zip(nulls).enumerate() {
            let target = if n == 0 { &mut self.hashes } else { &mut self.column };
            target.clear();
            let column = batch.columns.get(at).ok_or_else(|| crate::exec::broken("a join key its batch lacks"))?;
            hash_column((column, rows, count), matches_null, how, (target, &mut self.misses))?;
            if n > 0 {
                self.hashes.iter_mut().zip(&self.column).for_each(|(h, &next)| *h = how.seed.joined(*h, next));
            }
        }
        Ok(())
    }
}

/// \[DESIGN\] (RULES.md D1080) The hashes of the keys whose `numbers` were
/// given as to a direct table (`Hashing::direct`), as a hashed table
/// hashes them by `seed`: a number's word hashed, a null's the hash of a
/// null, so a build that asked its keys' numbers first, to see whether
/// they are dense, hashes none of its vectors again.
pub(in crate::exec) fn numbers_hashed(numbers: &[u64], seed: &Seed) -> Vec<u64> {
    numbers.iter().map(|&bits| if bits == NULL_NUMBER { seed.null() } else { seed.word(bits) }).collect()
}

/// The place in a vector of the `n`th row given, `rows` or all.
fn nth(rows: Option<&[usize]>, n: usize) -> usize {
    rows.map_or(n, |r| r.get(n).copied().unwrap_or(usize::MAX))
}

/// Appends to `out` the hash of each of `count` rows given of `column`,
/// `rows` or all, marking in `misses` each whose value matches nothing.
///
/// # Errors
///
/// As `KeyHashes::of_batch`.
fn hash_column(
    (column, rows, count): (&Column, Option<&[usize]>, usize),
    nulls: bool,
    how: Hashing<'_>,
    (out, misses): (&mut Vec<u64>, &mut [bool]),
) -> Result<()> {
    let null = |miss: &mut bool| {
        *miss = *miss || !nulls;
        how.null()
    };
    let places = (0..count).map(|n| nth(rows, n));
    match column {
        Column::Ints(ints) => {
            if ints.scale == 0 {
                integers_hashed((ints, rows, count), how, out);
                let base = out.len().saturating_sub(count);
                nulls_marked((ints, rows), how, nulls, (out.get_mut(base..).unwrap_or_default(), misses));
            } else {
                out.extend(places.zip(misses.iter_mut()).map(|(r, m)| ints_hash(ints, r, how).unwrap_or_else(|| null(m))));
            }
        }
        Column::Floats(f) => out.extend(places.zip(misses.iter_mut()).map(|(r, m)| f.get(r).map_or_else(|| null(m), |x| how.number(x)))),
        Column::Texts(texts) => texts_hashed((texts, places), how, (out, misses), &null),
        Column::Datetimes(d) => {
            out.extend(places.zip(misses.iter_mut()).map(|(r, m)| d.get(r).map_or_else(|| null(m), |x| instant(how, x))));
        }
        Column::Bools(b) => {
            out.extend(places.zip(misses.iter_mut()).map(|(r, m)| b.get(r).map_or_else(|| null(m), |x| how.word(u64::from(x)))));
        }
        Column::Nulls(_) => out.extend(misses.iter_mut().map(null)),
        Column::Values(values) => {
            for (r, m) in places.zip(misses.iter_mut()) {
                out.push(value_hash(values.get(r).unwrap_or(&Value::Null), nulls, how)?.unwrap_or_else(|| null(m)));
            }
        }
        Column::Late(late) => {
            for (r, m) in places.zip(misses.iter_mut()) {
                out.push(value_hash(late.values().get(r).unwrap_or(&Value::Null), nulls, how)?.unwrap_or_else(|| null(m)));
            }
        }
    }
    Ok(())
}

/// \[DESIGN\] (RULES.md D1288) The word a whole number `n` keys by. Below
/// 2^51 in magnitude it is the bits of its `f64`, which is `n` exactly by
/// `MAGIC`, as a direct table reads it (`direct.rs`). A wider one is `n`
/// itself, its bits crossed with a null's number, so no whole number's
/// word is a null's, as the one that would be is 0, which is below 2^51.
/// Whole numbers apart on one side of 2^51 have words apart.
#[inline]
fn whole_word(n: i64) -> u64 {
    if n.unsigned_abs() < MAGIC_UNITS {
        (f64::from_bits(MAGIC.to_bits().wrapping_add(n.cast_unsigned())) - MAGIC).to_bits()
    } else {
        n.cast_unsigned() ^ NULL_NUMBER
    }
}

/// \[DESIGN\] (RULES.md D1288) The whole number the exact number of
/// `units` at `scale` is, if it is one that 64 bits hold.
fn whole_of(units: i128, scale: u32) -> Option<i64> {
    if scale == 0 {
        return i64::try_from(units).ok();
    }
    let power = pow10(scale)?;
    if units.checked_rem(power)? != 0 {
        return None;
    }
    i64::try_from(units.checked_div(power)?).ok()
}

/// \[DESIGN\] (RULES.md D1288) The whole number the approximate number
/// `f` is, if its magnitude is from 2^51 to 2^63 and 64 bits hold it:
/// read from its bits, its mantissa with the bit above it, moved by its
/// exponent. One of 2^52 or more has no fraction, and one below it has a
/// half if its last bit is set. A whole number below 2^51 keys by its
/// bits as it is.
fn whole_f64(f: f64) -> Option<i64> {
    if !(MAGIC_RANGE_F64..=WORD_RANGE).contains(&f.abs()) {
        return None;
    }
    // Of the magnitude, so no sign is above the exponent's bits.
    let bits = f.abs().to_bits();
    let above = 1_u64 << MANTISSA_BITS;
    let mantissa = (bits & above.wrapping_sub(1)) | above;
    let magnitude = match u32::try_from((bits >> MANTISSA_BITS).checked_sub(LEAST_EXPONENT)?).ok()?.checked_sub(1) {
        Some(up) => mantissa.checked_shl(up)?,
        None => (mantissa & 1 == 0).then_some(mantissa >> 1_u32)?,
    };
    if f.is_sign_negative() { 0_i64.checked_sub_unsigned(magnitude) } else { i64::try_from(magnitude).ok() }
}

/// \[DESIGN\] (RULES.md D1288) The word the exact number of `units` at
/// `scale` keys by: of units below 2^51 the bits of its `f64`, exact or
/// correctly rounded (`nearest`), which of a whole number are its word; of
/// a whole number 64 bits hold, its word; of any other, a fraction or
/// past 64 bits, the bits of its nearest `f64`.
fn exact_word(units: i128, scale: u32) -> u64 {
    if units.unsigned_abs() < MAGIC_RANGE.unsigned_abs() {
        return canonical(nearest(units, scale));
    }
    whole_of(units, scale).map_or_else(|| canonical(nearest(units, scale)), whole_word)
}

/// \[DESIGN\] (RULES.md D1288) The word the approximate number `f` keys
/// by: of a whole number 64 bits hold, its word, as an exact number equal
/// to it keys; of any other its bits, -0 as 0.
fn approx_word(f: f64) -> u64 {
    whole_f64(f).map_or_else(|| canonical(f), whole_word)
}

/// \[DESIGN\] (RULES.md D1288) The exact number equal to the decimal
/// floating-point number `d`, if it has no more digits after the point
/// than an exact number may and 128 bits hold its units. Every whole
/// number 64 bits hold is so, and keys as the exact number equal to it;
/// any other keys by its nearest `f64`, as an exact number that is no
/// whole number of 64 bits does.
fn exact_of(d: &DecFloat) -> Option<Decimal> {
    let after = i64::from(d.exponent()).checked_neg()?.max(0);
    d.to_decimal(u32::try_from(after).ok().filter(|s| *s <= MAX_EXACT_PRECISION)?)
}

/// \[DESIGN\] (RULES.md D1098, D1288) Appends to `out` the hash of each
/// of `count` rows given of `ints`, of scale 0, `rows` or all, a null's as
/// of its place's unit: each of its word (`whole_word`), made by a loop
/// with no call and the kernels' hash of words where they are given; to a
/// direct table, the words themselves.
fn integers_hashed((ints, rows, count): (&Ints, Option<&[usize]>, usize), how: Hashing<'_>, out: &mut Vec<u64>) {
    let words = (0..count).map(|n| whole_word(ints.units.get(nth(rows, n)).copied().unwrap_or(0)));
    if how.direct {
        out.extend(words);
    } else if let Some(kernels) = how.kernels {
        let words: Vec<i64> = words.map(u64::cast_signed).collect();
        kernels.hash_i64(&words, how.seed.word_seed(), None, out);
    } else {
        out.extend(words.map(|w| how.seed.word(w)));
    }
}

/// Of the hashes `out` of rows given of `ints`, `rows` or all, each of a
/// null made a null's, marked a miss in `misses` unless nulls match.
fn nulls_marked((ints, rows): (&Ints, Option<&[usize]>), how: Hashing<'_>, nulls: bool, (out, misses): (&mut [u64], &mut [bool])) {
    if ints.nulls.is_none() {
        return;
    }
    for (n, (h, m)) in out.iter_mut().zip(misses.iter_mut()).enumerate() {
        if ints.is_null(nth(rows, n)) {
            *h = how.null();
            *m = *m || !nulls;
        }
    }
}

/// The hash of row `r` of `ints`, none if it is null.
fn ints_hash(ints: &Ints, r: usize, how: Hashing<'_>) -> Option<u64> {
    let units = *ints.units.get(r).filter(|_| !ints.is_null(r))?;
    Some(how.exact(i128::from(units), ints.scale))
}

/// The hash of datetime `d`: of its instant.
fn instant(how: Hashing<'_>, d: Datetime) -> u64 {
    how.word(d.utc.cast_unsigned())
}

/// Appends to `out` the hash of the text of each row of `texts` at
/// `places`, a null's as `null` says: of a vector coded by a dictionary,
/// each entry hashed once.
fn texts_hashed(
    (texts, places): (&Texts, impl Iterator<Item = usize>),
    how: Hashing<'_>,
    (out, misses): (&mut Vec<u64>, &mut [bool]),
    null: &dyn Fn(&mut bool) -> u64,
) {
    let Some(coded) = &texts.coded else {
        out.extend(places.zip(misses.iter_mut()).map(|(r, m)| texts.get(r).map_or_else(|| null(m), |t| how.text(t))));
        return;
    };
    let mut entries: Vec<Option<u64>> = vec![None; coded.entries.len()];
    for (r, m) in places.zip(misses.iter_mut()) {
        let code = coded.codes.get(r).and_then(|&c| usize::try_from(c).ok()).filter(|_| !texts.is_null(r));
        let Some((code, slot)) = code.and_then(|c| entries.get_mut(c).map(|s| (c, s))) else {
            out.push(null(m));
            continue;
        };
        let h = *slot.get_or_insert_with(|| coded.entries.get(code).map_or_else(|| how.seed.null(), |t| how.text(t)));
        out.push(h);
    }
}

/// The nearest `f64` to the exact number of `units` at `scale`, as
/// `Decimal::nearest` gives it: exactly by `MAGIC` and `POWERS` where they
/// hold it, else by the value's bucket.
pub(in crate::exec) fn nearest(units: i128, scale: u32) -> f64 {
    let power = usize::try_from(scale).ok().and_then(|s| POWERS.get(s));
    let small = i64::try_from(units).ok().filter(|_| units.unsigned_abs() < MAGIC_RANGE.unsigned_abs());
    if let (Some(&power), Some(whole)) = (power, small) {
        let exact = f64::from_bits(MAGIC.to_bits().wrapping_add(whole.cast_unsigned())) - MAGIC;
        return exact / power;
    }
    match crate::value::bucket(&Value::Exact(Decimal::new(units, scale))) {
        Some(Bucket::Number(bits)) => f64::from_bits(bits),
        Some(_) | None => f64::NAN,
    }
}

/// The hash of `v` on a key whose nulls match as `nulls` says, as of its
/// bucket; none if it has none.
///
/// # Errors
///
/// As `KeyHashes::of_batch`.
pub(in crate::exec) fn value_hash(v: &Value, nulls: bool, how: Hashing<'_>) -> Result<Option<u64>> {
    match v {
        Value::Exact(d) => Ok(Some(how.exact(d.units, d.scale))),
        Value::Approx(f) => Ok(Some(how.number(*f))),
        Value::DecFloat(d) => match exact_of(d) {
            Some(exact) => Ok(Some(how.exact(exact.units, exact.scale))),
            None => Ok(key_bucket(v, nulls)?.map(|b| how.bucket(&b))),
        },
        Value::Char(s) => Ok(Some(how.text(s))),
        Value::Datetime(d) => Ok(Some(instant(how, *d))),
        Value::Bool(b) => Ok(Some(how.word(u64::from(*b)))),
        Value::Null
        | Value::Binary(_)
        | Value::Lob(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_)
        | Value::Json(_)
        | Value::Extension(_)
        | Value::Xml(_) => Ok(key_bucket(v, nulls)?.map(|b| how.bucket(&b))),
    }
}

impl Hashing<'_> {
    /// The hash of a word `w`.
    fn word(self, w: u64) -> u64 {
        self.seed.word(w)
    }

    /// The hash of the approximate number `f`, of its word
    /// (`approx_word`); given to a direct table, the word itself (RULES.md
    /// D601).
    fn number(self, f: f64) -> u64 {
        if self.direct { approx_word(f) } else { self.seed.word(approx_word(f)) }
    }

    /// The hash of the exact number of `units` at `scale`, of its word
    /// (`exact_word`); given to a direct table, the word itself.
    fn exact(self, units: i128, scale: u32) -> u64 {
        if self.direct { exact_word(units, scale) } else { self.seed.word(exact_word(units, scale)) }
    }

    /// The hash of a null on a key whose nulls match; given to a direct
    /// table, `NULL_NUMBER` (RULES.md D601).
    fn null(self) -> u64 {
        if self.direct { NULL_NUMBER } else { self.seed.null() }
    }

    /// The hash of a character string's text `text`, its trailing spaces
    /// left out.
    fn text(self, text: &str) -> u64 {
        let trimmed = text.trim_end_matches(' ').as_bytes();
        if self.keyed { self.seed.keyed(trimmed) } else { self.seed.bytes(trimmed) }
    }

    /// The hash of bucket `b`, as of a value of it.
    fn bucket(self, b: &Bucket) -> u64 {
        match b {
            Bucket::Number(bits) => self.number(f64::from_bits(*bits)),
            Bucket::Char(s) => self.text(s),
            Bucket::Instant(utc) => self.word(utc.cast_unsigned()),
            Bucket::Bool(t) => self.word(u64::from(*t)),
            Bucket::Null => self.null(),
            Bucket::Binary(_) | Bucket::Span(_) | Bucket::Extension(_) | Bucket::Composite(_) | Bucket::Bag(_) => {
                let bytes = crate::spill::bucket_key(std::slice::from_ref(b));
                if self.keyed { self.seed.keyed(&bytes) } else { self.seed.bytes(&bytes) }
            }
        }
    }
}

/// The hash of a key of `values`, of each whether a null matches a null as
/// `nulls` says, as `KeyHashes::of_batch` hashes it of a batch's vectors;
/// none if a value has no bucket.
///
/// # Errors
///
/// As `KeyHashes::of_batch`.
pub(in crate::exec) fn values_hash<'v>(values: impl Iterator<Item = &'v Value>, nulls: &[bool], how: Hashing<'_>) -> Result<Option<u64>> {
    let mut hash = None;
    for (v, &matches_null) in values.zip(nulls) {
        let Some(h) = value_hash(v, matches_null, how)? else { return Ok(None) };
        hash = Some(hash.map_or(h, |so_far| how.seed.joined(so_far, h)));
    }
    Ok(hash)
}
