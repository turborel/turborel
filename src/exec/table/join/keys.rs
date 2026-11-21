//! \[DESIGN\] (RULES.md D368, D369) The hashes of a join's keys, of a
//! batch's rows a vector at a time. A key's hash is of its bucket
//! (`value::key_bucket`), which every two values a join matches share: a
//! number's of its nearest `f64`'s bits, -0 as 0, so an exact number of
//! any scale and an approximate one that match hash alike; a character
//! string's of its text less its trailing spaces; a datetime's of its
//! instant; a truth value's of itself; a null's, on a key whose nulls
//! match, a constant; any other value's of its bucket's bytes. A row whose
//! key has no bucket, a null on a key whose nulls match nothing, or a
//! large object, matches no row and is marked a miss.
//!
//! A vector of integers is hashed without a value made: an integer of
//! fewer than 51 bits is its `f64` exactly by adding it to 1.5 × 2^52 in
//! the bits of a double, and one of a scale up to 22 is divided by 10 to
//! it, both exact, so the quotient is correctly rounded, the nearest `f64`
//! a value's digits give. A vector of texts coded by a dictionary hashes
//! each entry once.
//!
//! Words are mixed by the hash of the grouping's table (`table/hash.rs`),
//! seeded at random as the database opens; a text by SipHash-1-3 keyed by
//! that seed instead, once a table's probes found many rows of one hash
//! and another key (RULES.md D369), as a text may be made to collide
//! under the hash of words whatever the seed.

use super::super::hash::{Seed, canonical, joined};
use crate::error::Result;
use crate::storage::{Batch, Column, Ints, Texts};
use crate::value::{Bucket, Datetime, Decimal, Value, key_bucket};

/// \[DESIGN\] 1.5 × 2^52, to which an integer of fewer than 51 bits is
/// added in a double's bits to give its `f64` exactly.
const MAGIC: f64 = 6_755_399_441_055_744.0_f64;

/// \[DESIGN\] An integer below this in magnitude is its `f64` exactly by
/// `MAGIC`: 2^51.
const MAGIC_RANGE: i128 = 1_i128 << 51_u32;

/// \[DESIGN\] The powers of 10 an `f64` holds exactly: up to 10^22.
const POWERS: [f64; 23] = [
    1e0_f64, 1e1_f64, 1e2_f64, 1e3_f64, 1e4_f64, 1e5_f64, 1e6_f64, 1e7_f64, 1e8_f64, 1e9_f64, 1e10_f64, 1e11_f64, 1e12_f64, 1e13_f64,
    1e14_f64, 1e15_f64, 1e16_f64, 1e17_f64, 1e18_f64, 1e19_f64, 1e20_f64, 1e21_f64, 1e22_f64,
];

/// How a join hashes its keys: by its database's seed, and whether a text
/// is hashed by SipHash keyed by it (RULES.md D369).
#[derive(Clone, Copy)]
pub(in crate::exec) struct Hashing<'s> {
    pub(in crate::exec) seed: &'s Seed,
    pub(in crate::exec) keyed: bool,
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
                self.hashes.iter_mut().zip(&self.column).for_each(|(h, &next)| *h = joined(*h, next));
            }
        }
        Ok(())
    }
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
        how.seed.null()
    };
    let places = (0..count).map(|n| nth(rows, n));
    match column {
        Column::Ints(ints) => out.extend(places.zip(misses.iter_mut()).map(|(r, m)| ints_hash(ints, r, how).unwrap_or_else(|| null(m)))),
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

/// The hash of row `r` of `ints`, none if it is null.
fn ints_hash(ints: &Ints, r: usize, how: Hashing<'_>) -> Option<u64> {
    let units = *ints.units.get(r).filter(|_| !ints.is_null(r))?;
    Some(how.number(nearest(i128::from(units), ints.scale)))
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
        Value::Exact(d) => Ok(Some(how.number(nearest(d.units, d.scale)))),
        Value::Approx(f) => Ok(Some(how.number(*f))),
        Value::Char(s) => Ok(Some(how.text(s))),
        Value::Datetime(d) => Ok(Some(instant(how, *d))),
        Value::Bool(b) => Ok(Some(how.word(u64::from(*b)))),
        Value::Null
        | Value::DecFloat(_)
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

    /// The hash of a number whose nearest `f64` is `f`.
    fn number(self, f: f64) -> u64 {
        self.seed.word(canonical(f))
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
            Bucket::Number(bits) => self.word(*bits),
            Bucket::Char(s) => self.text(s),
            Bucket::Instant(utc) => self.word(utc.cast_unsigned()),
            Bucket::Bool(t) => self.word(u64::from(*t)),
            Bucket::Null => self.seed.null(),
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
        hash = Some(hash.map_or(h, |so_far| joined(so_far, h)));
    }
    Ok(hash)
}
