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
//! Every hash is XXH3-64 by the database's seed (RULES.md D573): a word's
//! of its 8 bytes, little-endian, a text's of its bytes; keys of several
//! columns join each column's hash in order, as XXH3-64 of the 16 bytes
//! of the hash so far and the next. The kernels hash a vector of
//! integers, one of texts, and join a column's hashes, each exactly as
//! these hash one. The seed is drawn at random as the database opens, so
//! no client knows it. A key of bytes may still be made to collide
//! whatever the seed, so a table whose probe grows long hashes its keys
//! anew by SipHash-1-3 keyed by the seed (`keyed`), which no key collides
//! under unknown.
//!
//! [ISO/IEC 9075-2:2023 §4.3.2] (RULES.md D1183) A grouping's seed also
//! says which of its key's columns are grouped unpadded, under a collation
//! of NO PAD that orders strings as their octets do (`Seed::unpadded`). A
//! character string of such a column hashes whole, its trailing spaces
//! among its bytes, and a table made by the seed compares it so
//! (`keys.rs`), so `'a'` and `'a '` are two groups.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::Arc;

use crate::hash::xxhash::Seeded;
use crate::kernels::Kernels;
use crate::storage::{Batch, Cell, Column, Texts};
use crate::value::{Decimal, Value};

/// \[DESIGN\] The word a null's hash is of: the bits of the golden ratio,
/// as hash tables commonly take a constant of no structure.
const NULL_WORD: u64 = 0x9e37_79b9_7f4a_7c15;

/// \[DESIGN\] (RULES.md D357) A database's seed of its groupings' hashes,
/// drawn at random as it opens: XXH3's seed, with what depends on it
/// alone made once, and the keys of SipHash-1-3 for a table whose probe
/// grew long.
#[derive(Clone, Debug)]
pub(crate) struct Seed {
    seeded: Seeded,
    keyed: RandomState,
    /// Of a grouping's seed, which of its key's columns are grouped
    /// unpadded, if any is.
    unpadded: Option<Arc<[bool]>>,
}

impl Default for Seed {
    fn default() -> Seed {
        let keyed = RandomState::new();
        Seed { seeded: Seeded::new(keyed.hash_one(NULL_WORD)), keyed, unpadded: None }
    }
}

impl Seed {
    /// A seed of the word `word`, for tests whose hashes are known.
    #[cfg(test)]
    pub(crate) fn fixed(word: u64) -> Seed {
        Seed { seeded: Seeded::new(word), keyed: RandomState::new(), unpadded: None }
    }

    /// This seed for a grouping whose key's columns are grouped unpadded
    /// where `flags` says: itself if none is.
    pub(crate) fn unpadded(&self, flags: &[bool]) -> Seed {
        let unpadded = flags.contains(&true).then(|| Arc::from(flags));
        Seed { seeded: self.seeded.clone(), keyed: self.keyed.clone(), unpadded }
    }

    /// Whether the `place`th column of a key is grouped unpadded.
    pub(super) fn is_unpadded(&self, place: usize) -> bool {
        self.unpadded.as_ref().is_some_and(|flags| flags.get(place).copied().unwrap_or(false))
    }

    /// The hash of `cell`, the `place`th column of a key: a character
    /// string's whole if the column is grouped unpadded, else as `cell`.
    pub(super) fn key_cell(&self, place: usize, cell: Cell<'_>) -> u64 {
        if self.is_unpadded(place) { self.whole(cell) } else { self.cell(cell) }
    }

    /// The hash of `cell`, a character string's whole, its trailing
    /// spaces among its bytes; of any other value, as `cell`.
    fn whole(&self, cell: Cell<'_>) -> u64 {
        match cell {
            Cell::Text(text) => self.bytes(text.as_bytes()),
            Cell::Value(Value::Char(s)) => self.bytes(s.as_bytes()),
            Cell::Value(_) | Cell::Exact(..) | Cell::Approx(_) | Cell::Datetime(_) | Cell::Bool(_) | Cell::Null => self.cell(cell),
        }
    }

    /// The hash of a word `w`: XXH3-64 of its 8 bytes, little-endian.
    pub(super) fn word(&self, w: u64) -> u64 {
        self.seeded.words().hash(w)
    }

    /// XXH3's seed of its words' hashes, as the kernels take it
    /// (`Kernels::hash_i64`).
    pub(super) fn word_seed(&self) -> u64 {
        self.seeded.seed()
    }

    /// The hash of a null.
    pub(super) fn null(&self) -> u64 {
        self.word(NULL_WORD)
    }

    /// The XXH3 of `bytes`, seeded by the seed (RULES.md D357), as a
    /// parallel grouping parts its rows' keys.
    pub(crate) fn xxh3(&self, bytes: &[u8]) -> u64 {
        self.seeded.bytes(bytes)
    }

    /// The SipHash-1-3 of `bytes`, keyed by the seed (RULES.md D357).
    pub(super) fn keyed(&self, bytes: &[u8]) -> u64 {
        self.keyed.hash_one(bytes)
    }

    /// The hash of the bytes `bytes`: their XXH3-64.
    pub(super) fn bytes(&self, bytes: &[u8]) -> u64 {
        self.seeded.bytes(bytes)
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
        self.joined(self.joined(self.word(low), self.word(high)), self.word(u64::from(scale)))
    }

    /// A key's hash `so_far` with the next column's, `next`, joined: XXH3-64
    /// of their 16 bytes, little-endian, the hash so far first, so (a, b)
    /// and (b, a) hash apart.
    pub(in crate::exec) fn joined(&self, so_far: u64, next: u64) -> u64 {
        self.seeded.words().pair(so_far, next)
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

/// The hash of a key of `values`, by `seed`, as of the same values in a
/// batch's vectors.
pub(in crate::exec) fn hash_values(seed: &Seed, values: &[Value]) -> u64 {
    let mut hashes = values.iter().enumerate().map(|(place, v)| seed.key_cell(place, Cell::Value(v)));
    let first = hashes.next().unwrap_or_else(|| seed.null());
    hashes.fold(first, |so_far, next| seed.joined(so_far, next))
}

/// Writes to `out` the hash of each row given of `batch`, `rows` or all,
/// of its key of the columns at `at`, by `seed`, each column's hashed a
/// vector at a time, a vector of integers or texts by `kernels`, and
/// joined by them; of a key of no column, as `hash_values` hashes one.
pub(in crate::exec) fn hash_rows(
    batch: &Batch,
    (at, rows): (&[usize], Option<&[u32]>),
    (seed, kernels): (&Seed, &dyn Kernels),
    out: &mut Vec<u64>,
) {
    hash_places(batch, (at.iter().copied().enumerate(), rows), (seed, kernels), out);
}

/// \[DESIGN\] (RULES.md D1113, D1313) As `hash_rows`, of the columns of
/// the key at `lanes` alone, each hashed as the column of the key it is,
/// so a row's hash is the one `Keys::hash_of` gives of its group's key of
/// those lanes, and its partition by them the one its group would take.
pub(in crate::exec) fn hash_lanes(
    batch: &Batch,
    (at, lanes, rows): (&[usize], &[usize], Option<&[u32]>),
    (seed, kernels): (&Seed, &dyn Kernels),
    out: &mut Vec<u64>,
) {
    let places = lanes.iter().filter_map(|&lane| at.get(lane).map(|&c| (lane, c)));
    hash_places(batch, (places, rows), (seed, kernels), out);
}

/// Writes to `out` the hash of each row given of `batch`, `rows` or all,
/// of the columns `places` names, each the place it has in the key and
/// where it lies in the batch, joined in the order given; of none, a
/// null's hash a row.
fn hash_places(
    batch: &Batch,
    (places, rows): (impl Iterator<Item = (usize, usize)>, Option<&[u32]>),
    (seed, kernels): (&Seed, &dyn Kernels),
    out: &mut Vec<u64>,
) {
    out.clear();
    let (mut column_hashes, mut first) = (Vec::new(), true);
    for (place, c) in places {
        let target = if first { &mut *out } else { &mut column_hashes };
        hash_column((batch, c, rows), (seed, kernels, seed.is_unpadded(place)), target);
        if !first {
            kernels.combine_i64(out, &column_hashes, seed.seeded.seed());
        }
        first = false;
    }
    if first {
        out.resize(rows.map_or(batch.rows, <[u32]>::len), seed.null());
    }
}

/// How a column of a key is hashed: by a seed and kernels, and whether
/// its character strings whole, as a column grouped unpadded.
type How<'a> = (&'a Seed, &'a dyn Kernels, bool);

/// Writes to `out`, emptied first, the hash of each row given, `rows` or
/// all, of the column at `c` of `batch`: an integer's of scale 0 and a
/// text's by the kernels, every other's as its cell's.
fn hash_column((batch, c, rows): (&Batch, usize, Option<&[u32]>), (seed, kernels, unpadded): How<'_>, out: &mut Vec<u64>) {
    out.clear();
    let given = rows.map_or(batch.rows, <[u32]>::len);
    let places = || (0..given).map(|n| crate::storage::sel::nth(rows, n));
    if let Some(Column::Ints(ints)) = batch.columns.get(c)
        && ints.scale == 0
    {
        kernels.hash_i64(&ints.units, seed.seeded.seed(), rows, out);
        if let Some(nulls) = &ints.nulls {
            out.iter_mut().zip(places()).filter(|&(_, r)| nulls.get(r)).for_each(|(h, _)| *h = seed.null());
        }
        return;
    }
    if let Some(Column::Texts(texts)) = batch.columns.get(c) {
        return hash_texts((texts, rows), (seed, kernels, unpadded), out);
    }
    if unpadded {
        // Whatever vector holds a string, it hashes whole.
        out.extend(places().map(|r| seed.whole(batch.cell(c, r))));
        return;
    }
    out.extend(places().map(|r| seed.cell(batch.cell(c, r))));
}

/// Appends to `into` the hash of each text at `sel`, or of all, of the
/// texts `given`, their octets and where each ends, by the kernel: whole
/// of a column grouped unpadded, else less its trailing spaces.
fn texts_by_kernel(given: (&[u8], &[usize]), sel: Option<&[u32]>, (seed, kernels, unpadded): How<'_>, into: &mut Vec<u64>) {
    if unpadded {
        kernels.hash_texts_unpadded(given, &seed.seeded, sel, into);
    } else {
        kernels.hash_texts(given, &seed.seeded, sel, into);
    }
}

/// \[DESIGN\] (RULES.md D1330) Appends to `out` the hash of each entry
/// of `entries`, a dictionary's texts held as their own, as the `place`th
/// column of a key hashes one, by the kernel in one pass, and a null's
/// after them, as a memo of codes places a null past its dictionary's
/// codes (`batch/groups/coded.rs`).
pub(in crate::exec) fn hash_entries(entries: &Texts, place: usize, (seed, kernels): (&Seed, &dyn Kernels), out: &mut Vec<u64>) {
    texts_by_kernel((entries.text.as_bytes(), &entries.ends), None, (seed, kernels, seed.is_unpadded(place)), out);
    out.push(seed.null());
}

/// Writes to `out` the hash of each text given of `texts`, `rows` or all,
/// as `Seed::text` hashes one, by the kernel: of texts coded by a store's
/// dictionaries, each entry hashed once and each row's taken by its code;
/// a null's the null's hash.
fn hash_texts((texts, rows): (&Texts, Option<&[u32]>), how: How<'_>, out: &mut Vec<u64>) {
    let (seed, _, unpadded) = how;
    let one = |text: &str| if unpadded { seed.bytes(text.as_bytes()) } else { seed.text(text) };
    let given = rows.map_or(texts.len(), <[u32]>::len);
    let places = || (0..given).map(|n| crate::storage::sel::nth(rows, n));
    match &texts.coded {
        Some(coded) => {
            // Each entry hashed once, if they are no more than the rows;
            // else each row's entry alone.
            let entry = |r: usize| coded.codes.get(r).and_then(|&e| usize::try_from(e).ok());
            if coded.entries.len() <= given {
                let mut entries = Vec::new();
                texts_by_kernel((coded.entries.text.as_bytes(), &coded.entries.ends), None, how, &mut entries);
                out.extend(places().map(|r| entry(r).and_then(|e| entries.get(e)).copied().unwrap_or(0)));
            } else {
                out.extend(places().map(|r| one(entry(r).and_then(|e| coded.entries.get(e)).unwrap_or(""))));
            }
        }
        None => texts_by_kernel((texts.text.as_bytes(), &texts.ends), rows, how, out),
    }
    if let Some(nulls) = &texts.nulls {
        out.iter_mut().zip(places()).filter(|&(_, r)| nulls.get(r)).for_each(|(h, _)| *h = seed.null());
    }
}
