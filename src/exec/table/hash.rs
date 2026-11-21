//! \[DESIGN\] (RULES.md D356, D357) The hashes a grouping's table finds
//! its groups by. A key's hash is of its value as grouping compares it,
//! whatever vector holds it: an exact number's of its normalized units and
//! scale, so 1.0 and 1 hash alike; an approximate number's of its bits, -0
//! as 0; a datetime's of its instant in UTC; a character string's of its
//! text less its trailing spaces; a null's a constant; any other value's of
//! the bytes `spill::put_norm` gives of it. So a key hashes alike of a
//! vector of integers and of values, and the tables of a grouping's
//! morsels agree on every key's partition.
//!
//! A word is mixed by MurmurHash3's finalizer (`hash::murmur::fmix64`)
//! after the database's seed is added, a text 8 bytes at a time; keys of
//! several columns join each column's hash in order. The seed is drawn at
//! random as the database opens, so no client knows it. A key of bytes
//! may still be made to collide whatever the seed, so a table whose probe
//! grows long hashes its keys anew by SipHash-1-3 keyed by the seed
//! (`keyed`), which no key collides under unknown.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;

use crate::hash::murmur::fmix64;
use crate::kernels::Kernels;
use crate::storage::{Batch, Cell, Column};
use crate::value::{Decimal, Value};

/// \[DESIGN\] The word a null's hash is of: the bits of the golden ratio,
/// as hash tables commonly take a constant of no structure.
const NULL_WORD: u64 = 0x9e37_79b9_7f4a_7c15;

/// \[DESIGN\] What a text's hash starts from beside its length, and
/// multiplies each word by: MurmurHash3's first multiplier.
const TEXT_MULTIPLIER: u64 = 0x87c3_7b91_1142_53d5;

/// \[DESIGN\] How far a text's hash turns after each word: MurmurHash3's
/// turn of its first lane.
const TEXT_TURN: u32 = 31;

/// \[DESIGN\] How far a key's hash so far turns before the next column's
/// joins it, so (a, b) and (b, a) hash apart.
const JOIN_TURN: u32 = 23;

/// Bytes of a word.
const WORD: usize = 8;

/// \[DESIGN\] (RULES.md D357) A database's seed of its groupings' hashes,
/// drawn at random as it opens: a word for the hash of words, and the keys
/// of SipHash-1-3 for a table whose probe grew long.
#[derive(Clone, Debug)]
pub(crate) struct Seed {
    word: u64,
    keyed: RandomState,
}

impl Default for Seed {
    fn default() -> Seed {
        let keyed = RandomState::new();
        Seed { word: keyed.hash_one(NULL_WORD), keyed }
    }
}

impl Seed {
    /// A seed of the word `word`, for tests whose hashes are known.
    #[cfg(test)]
    pub(crate) fn fixed(word: u64) -> Seed {
        Seed { word, keyed: RandomState::new() }
    }

    /// The hash of a word `w`.
    pub(super) fn word(&self, w: u64) -> u64 {
        fmix64(self.word ^ w)
    }

    /// The hash of a null.
    pub(super) fn null(&self) -> u64 {
        self.word(NULL_WORD)
    }

    /// The XXH3 of `bytes`, seeded by the seed's word (RULES.md D357), as
    /// a parallel grouping parts its rows' keys.
    pub(crate) fn xxh3(&self, bytes: &[u8]) -> u64 {
        crate::hash::xxhash::xxh3_64_with_seed(bytes, self.word)
    }

    /// The SipHash-1-3 of `bytes`, keyed by the seed (RULES.md D357).
    pub(super) fn keyed(&self, bytes: &[u8]) -> u64 {
        self.keyed.hash_one(bytes)
    }

    /// The hash of the bytes `bytes`, 8 at a time.
    pub(super) fn bytes(&self, bytes: &[u8]) -> u64 {
        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let mut h = self.word ^ length.wrapping_mul(TEXT_MULTIPLIER);
        let (words, rest) = bytes.as_chunks::<WORD>();
        for w in words {
            h = (h ^ u64::from_le_bytes(*w)).wrapping_mul(TEXT_MULTIPLIER).rotate_left(TEXT_TURN);
        }
        fmix64(h ^ word_of(rest))
    }

    /// The hash of a character string's text `text`, its trailing spaces
    /// left out, as strings compare padded [X3.135-1986 §5.11 GR5].
    pub(super) fn text(&self, text: &str) -> u64 {
        self.bytes(text.trim_end_matches(' ').as_bytes())
    }

    /// The hash of the exact number of `units` at `scale`: of its normalized
    /// form, an integer within 64 bits by its word.
    pub(super) fn exact(&self, units: i128, scale: u32) -> u64 {
        let (units, scale) = Decimal::new(units, scale).normalized();
        if let (0, Ok(integer)) = (scale, i64::try_from(units)) {
            return self.word(integer.cast_unsigned());
        }
        let bits = units.cast_unsigned();
        let (low, high) = (u64::try_from(bits & u128::from(u64::MAX)).unwrap_or(0), u64::try_from(bits >> 64_u32).unwrap_or(0));
        joined(joined(self.word(low), self.word(high)), self.word(u64::from(scale)))
    }

    /// The hash of the approximate number `f`, -0 as 0, as they compare.
    pub(super) fn approx(&self, f: f64) -> u64 {
        self.word(canonical(f))
    }

    /// The hash of `v`, as of the same value in any vector.
    pub(super) fn value(&self, v: &Value) -> u64 {
        match v {
            Value::Null => self.null(),
            Value::Exact(d) => self.exact(d.units, d.scale),
            Value::Approx(f) => self.approx(*f),
            Value::Char(s) => self.text(s),
            Value::Datetime(d) => self.word(d.utc.cast_unsigned()),
            Value::Bool(b) => self.word(u64::from(*b)),
            Value::DecFloat(_)
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
            | Value::Xml(_) => {
                let mut key = Vec::new();
                crate::spill::put_norm(v, &mut key);
                self.bytes(&key)
            }
        }
    }

    /// The hash of `cell`, as of its value.
    pub(super) fn cell(&self, cell: Cell<'_>) -> u64 {
        match cell {
            Cell::Value(v) => self.value(v),
            Cell::Exact(units, scale) => self.exact(i128::from(units), scale),
            Cell::Approx(f) => self.approx(f),
            Cell::Text(text) => self.text(text),
            Cell::Datetime(d) => self.word(d.utc.cast_unsigned()),
            Cell::Bool(b) => self.word(u64::from(b)),
            Cell::Null => self.null(),
        }
    }
}

/// The bits an approximate number keys by: -0 as 0.
pub(super) fn canonical(f: f64) -> u64 {
    if f == 0.0_f64 { 0.0_f64.to_bits() } else { f.to_bits() }
}

/// The little-endian word of up to 8 `bytes`, the missing high bytes zero.
fn word_of(bytes: &[u8]) -> u64 {
    let mut le = [0_u8; WORD];
    le.iter_mut().zip(bytes).for_each(|(to, from)| *to = *from);
    u64::from_le_bytes(le)
}

/// A key's hash `so_far` with the next column's, `next`, joined.
pub(super) fn joined(so_far: u64, next: u64) -> u64 {
    fmix64(so_far.rotate_left(JOIN_TURN) ^ next)
}

/// The hash of a key of `values`, by `seed`, as of the same values in a
/// batch's vectors.
pub(in crate::exec) fn hash_values(seed: &Seed, values: &[Value]) -> u64 {
    let mut hashes = values.iter().map(|v| seed.value(v));
    let first = hashes.next().unwrap_or_else(|| seed.null());
    hashes.fold(first, joined)
}

/// Writes to `out` the hash of each row given of `batch`, `rows` or all,
/// of its key of the columns at `at`, by `seed`, each column's hashed a
/// vector at a time, a vector of integers by `kernels`; of a key of no
/// column, as `hash_values` hashes one.
pub(in crate::exec) fn hash_rows(
    batch: &Batch,
    (at, rows): (&[usize], Option<&[u32]>),
    (seed, kernels): (&Seed, &dyn Kernels),
    out: &mut Vec<u64>,
) {
    out.clear();
    if at.is_empty() {
        out.resize(rows.map_or(batch.rows, <[u32]>::len), seed.null());
    }
    let mut column_hashes = Vec::new();
    for (place, &c) in at.iter().enumerate() {
        let target = if place == 0 { &mut *out } else { &mut column_hashes };
        hash_column((batch, c, rows), (seed, kernels), target);
        if place > 0 {
            out.iter_mut().zip(&column_hashes).for_each(|(h, &next)| *h = joined(*h, next));
        }
    }
}

/// Writes to `out`, emptied first, the hash of each row given, `rows` or
/// all, of the column at `c` of `batch`: an integer's of scale 0 by the
/// kernel, every other's as its cell's.
fn hash_column((batch, c, rows): (&Batch, usize, Option<&[u32]>), (seed, kernels): (&Seed, &dyn Kernels), out: &mut Vec<u64>) {
    out.clear();
    let given = rows.map_or(batch.rows, <[u32]>::len);
    let places = || (0..given).map(|n| crate::storage::sel::nth(rows, n));
    if let Some(Column::Ints(ints)) = batch.columns.get(c)
        && ints.scale == 0
    {
        kernels.hash_i64(&ints.units, seed.word, rows, out);
        if let Some(nulls) = &ints.nulls {
            out.iter_mut().zip(places()).filter(|&(_, r)| nulls.get(r)).for_each(|(h, _)| *h = seed.null());
        }
        return;
    }
    out.extend(places().map(|r| seed.cell(batch.cell(c, r))));
}
