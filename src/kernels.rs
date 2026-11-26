//! \[DESIGN\] The kernels an executor's batches run on (RULES.md D354): the
//! loops of every primitive over a vector, a selection of rows, a sum, a
//! gather, written once each, as Boncz, Zukowski and Nes, "MonetDB/X100"
//! (2005), §4, write a primitive per type, operator and operand shape, and
//! passing selection vectors between them in place of the rows copied.
//!
//! `Kernels` is an extension point (`db.rs`): the database holds one, the
//! built-in `Portable` unless a plug-in's is set (`Database::register_kernels`),
//! which may run the same primitives on wider instructions. Every method
//! has its portable body as its default, so a set of kernels that predates
//! a method still runs it, and every one must give what the portable body
//! gives, of any input: the contract tests in `turborel-test-support`
//! (`kernels.rs`) hold each implementation to it.
//!
//! A selection's rows are ascending, each below its vector's length; a
//! null's slot holds 0, and its row is left out of every selection.

pub mod portable;

use crate::hash::xxhash::{self, Seeded, Stripes as _};
use crate::storage::Mask;
use crate::value::CompOp;

/// The rows a kernel reads: the nulls to leave out, if any is null, and
/// the rows of a selection, or all.
pub type Given<'a> = (Option<&'a Mask>, Option<&'a [u32]>);

/// \[DESIGN\] Primitives over vectors, each a loop a batch's operator runs
/// in place of one a row (RULES.md D354). Each default is the portable
/// body (`kernels/portable`); an implementation gives the same results.
pub trait Kernels: Send + Sync {
    /// Its name, as diagnostics show it.
    fn name(&self) -> &'static str;

    /// Appends to `out`, ascending, the rows of `given`'s selection, or of
    /// all `values`, not null, whose value `v` satisfies `v op c` of
    /// `pred = (op, c)`.
    fn select_i64(&self, values: &[i64], pred: (CompOp, i64), given: Given<'_>, out: &mut Vec<u32>) {
        portable::select::select_i64(values, pred, given, out);
    }

    /// As `select_i64`, of approximate numbers, compared as `f64::partial_cmp`
    /// orders them, as a row's judgment compares two (`value::compare`).
    fn select_f64(&self, values: &[f64], pred: (CompOp, f64), given: Given<'_>, out: &mut Vec<u32>) {
        portable::select::select_f64(values, pred, given, out);
    }

    /// Appends to `out` the rows of `given`, not null, whose bit is set in
    /// `bits`, words of a `Mask`.
    fn select_mask(&self, bits: &[u64], given: Given<'_>, out: &mut Vec<u32>) {
        portable::select::select_mask(bits, given, out);
    }

    /// Appends to `out` the rows of `given`, not null, whose dictionary
    /// code's bit is set in `keep`, words of a `Mask` of the entries.
    fn select_codes(&self, codes: &[u32], keep: &[u64], given: Given<'_>, out: &mut Vec<u32>) {
        portable::select::select_codes(codes, keep, given, out);
    }

    /// The exact sum of `values` at `sel`, or of all; a null's slot holds
    /// 0, so it adds nothing.
    fn sum_i64(&self, values: &[i64], sel: Option<&[u32]>) -> i128 {
        portable::sum::sum_i64(values, sel)
    }

    /// The least and greatest of `values` of `given`, not null; none of no
    /// row.
    fn min_max_i64(&self, values: &[i64], given: Given<'_>) -> Option<(i64, i64)> {
        portable::sum::min_max_i64(values, given)
    }

    /// Appends to `out` the values at `sel`, in order; 0 for a row past the
    /// end.
    fn gather_i64(&self, values: &[i64], sel: &[u32], out: &mut Vec<i64>) {
        portable::gather::gather_i64(values, sel, out);
    }

    /// Appends to `out` the hash of each of `values` at `sel`, or of all,
    /// by `seed`, as a grouping's table hashes a word (RULES.md D356,
    /// D573): XXH3-64 of its 8 bytes, little-endian
    /// (`hash::xxhash::Words::hash`); that of 0 for a row past the end.
    fn hash_i64(&self, values: &[i64], seed: u64, sel: Option<&[u32]>, out: &mut Vec<u64>) {
        portable::hash::hash_i64(values, seed, sel, out);
    }

    /// Joins to each of `hashes`, a key's hash so far, the next column's of
    /// its row in `next`, by `seed`, as a grouping's table joins a key's
    /// columns (RULES.md D573): XXH3-64 of the 16 bytes of the two,
    /// little-endian, the hash so far first (`hash::xxhash::Words::pair`).
    /// Of `hashes` past `next`, none changes.
    fn combine_i64(&self, hashes: &mut [u64], next: &[u64], seed: u64) {
        portable::hash::combine_i64(hashes, next, seed);
    }

    /// Appends to `out` the hash of each text at `sel`, or of all, of the
    /// texts of `text` ending at `ends`, by `seeded`, as a grouping's
    /// table hashes a character string (RULES.md D573): XXH3-64 of its
    /// bytes less its trailing spaces (`hash::xxhash::Seeded::bytes`); of
    /// a text past the end, that of no byte.
    fn hash_texts(&self, (text, ends): (&[u8], &[usize]), seeded: &Seeded, sel: Option<&[u32]>, out: &mut Vec<u64>) {
        portable::hash::hash_texts((text, ends), seeded, sel, out);
    }

    /// As `hash_texts`, of each text whole, its trailing spaces among its
    /// bytes, as a grouping's table hashes a character string under a
    /// collation of NO PAD that orders strings as their octets do
    /// (RULES.md D1183).
    fn hash_texts_unpadded(&self, (text, ends): (&[u8], &[usize]), seeded: &Seeded, sel: Option<&[u32]>, out: &mut Vec<u64>) {
        portable::hash::hash_texts_unpadded((text, ends), seeded, sel, out);
    }

    /// Appends to `out` the place of each of `hashes` a blocked Bloom
    /// filter of `words` may hold, in order (RULES.md D369): its block of
    /// 8 words picked by a hash's high bits, four of its bits by its low
    /// ones (`portable::bloom`).
    fn bloom_probe(&self, words: &[u64], hashes: &[u64], out: &mut Vec<u32>) {
        portable::bloom::bloom_probe(words, hashes, out);
    }

    /// Appends to `out` the values from the `from`th to the `to`th of
    /// `bits` bits packed in `bytes`, as a chunk's frame of reference
    /// keeps them (RULES.md D352), each plus `least`, wrapping: none if
    /// `bytes` hold fewer values, or `bits` is above 64. The whole blocks
    /// of 64 values are what wider instructions unpack at once.
    fn unpack(&self, bytes: &[u8], (bits, least): (u32, i64), (from, to): (usize, usize), out: &mut Vec<i64>) -> Option<()> {
        portable::unpack::unpack(bytes, (bits, least), (from, to), out)
    }
    /// Makes each of `words`, an n of a chunk of decimals of magnitude
    /// below 2^51, the bits, as a signed integer, of its approximate
    /// number by `scale` (RULES.md D353): n made an `f64` exactly by the
    /// magic number 2^52 + 2^51, then multiplied by the factors or divided
    /// by the power. An n of 2^51 or more gives what the same bit
    /// operations give of it, of no use.
    fn alp_decode(&self, words: &mut [i64], scale: AlpScale) {
        portable::alp::alp_decode(words, scale);
    }

    /// XXH3's accumulation of `stripes` into `acc`, the `n`th keyed by the
    /// 64 bytes of `secret` at `8 n` (`hash::xxhash::Stripes::accumulate`,
    /// RULES.md D460): the loop of a long input's hash that wider
    /// instructions run.
    fn xxh3_accumulate(&self, acc: &mut [u64; xxhash::ACC_NB], stripes: &[xxhash::Stripe], secret: &[u8]) {
        xxhash::Scalar.accumulate(acc, stripes, secret);
    }
    /// XXH3's scramble of `acc` by `key`, the secret's last 64 bytes
    /// (`hash::xxhash::Stripes::scramble`).
    fn xxh3_scramble(&self, acc: &mut [u64; xxhash::ACC_NB], key: &xxhash::Stripe) {
        xxhash::Scalar.scramble(acc, key);
    }

    /// Writes into `keys`, one key of `width` bytes after another, at byte
    /// `at` of each, the value of `values` at each of `rows`: its distance
    /// above `base` as `bytes` big-endian bytes, each inverted if `invert`,
    /// as a sort's normal keys hold an integer (RULES.md D364, D380). None
    /// if a value is below `base` or past `bytes`, a row past the end, or
    /// the field past a key.
    fn normalize_i64(
        &self,
        (values, rows): (&[i64], &[u32]),
        (base, bytes, invert): (i64, usize, bool),
        (width, at): (usize, usize),
        keys: &mut [u8],
    ) -> Option<()> {
        portable::normal::normalize_i64((values, rows), (base, bytes, invert), (width, at), keys)
    }

    /// Appends to `out` the words of a mask of the `keys.len() / width`
    /// keys of `width` bytes each, a sort's normal keys in order: bit `r`
    /// set, for `r` from 1, if bytes `from` up to `to` of key `r` differ
    /// from those of key `r - 1`, as a window finds where its partitions
    /// and peer groups begin (RULES.md D560). Bit 0 is never set, nor any
    /// past the last key; of a width of 0, or a span of no bytes or past a
    /// key, none is.
    fn boundaries(&self, keys: &[u8], width: usize, span: (usize, usize), out: &mut Vec<u64>) {
        portable::boundaries::boundaries(keys, width, span, out);
    }

    /// The sum over each pair of `f32`s of `a` and `b` of `term`, in
    /// sixteen lanes over whole chunks, each lane summed alone, then the
    /// lanes in order and the rest one pair at a time (RULES.md D571). A
    /// dense vector index measures by it, the same at every instruction set.
    fn distance_f32(&self, a: &[f32], b: &[f32], term: DistanceTerm) -> f32 {
        portable::distance::distance_f32(a, b, term)
    }
    /// `a·a`, `b·b` and `a·b` of `f32`s in one pass, each as
    /// `distance_f32` sums a dot product: COSINE's three sums.
    fn cosine_f32(&self, a: &[f32], b: &[f32]) -> (f32, f32, f32) {
        portable::distance::cosine_f32(a, b)
    }
    /// As `distance_f32`, of two vectors of FLOAT32 or FLOAT16 elements
    /// packed little-endian, each read as an `f32`.
    fn distance_packed(&self, a: &[u8], b: &[u8], shape: (Packed, DistanceTerm)) -> f32 {
        portable::distance::distance_packed(a, b, shape)
    }
    /// As `distance_f32`, of two vectors of INT8 elements, in `i32` lanes
    /// added in an `i64`.
    fn distance_i8(&self, a: &[u8], b: &[u8], term: DistanceTerm) -> i64 {
        portable::distance::distance_i8(a, b, term)
    }

    /// CRC-32C's register after `data`, from `register`: CRC-32/ISCSI's,
    /// reflected, its starting value and final exclusive or left to the
    /// caller (RULES.md D572).
    fn crc32c(&self, register: u32, data: &[u8]) -> u32 {
        portable::crc::crc32c(register, data)
    }
}

/// \[DESIGN\] The term a distance kernel sums over each pair of elements
/// (RULES.md D571).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistanceTerm {
    /// `p * q`.
    Dot,
    /// `(p - q)^2`.
    Square,
    /// `|p - q|`.
    Gap,
}

impl DistanceTerm {
    /// Every term.
    pub const ALL: [DistanceTerm; 3] = [DistanceTerm::Dot, DistanceTerm::Square, DistanceTerm::Gap];
}

/// \[DESIGN\] A floating element format a distance kernel reads packed
/// little-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packed {
    /// FLOAT32.
    Single,
    /// FLOAT16.
    Half,
}

/// A database's kernels run XXH3's long inputs' loops: XXH3 of a vector's
/// long values hashes by `hash::xxhash::xxh3_64_on(kernels, ...)`.
impl xxhash::Stripes for dyn Kernels + '_ {
    fn accumulate(&self, acc: &mut [u64; xxhash::ACC_NB], stripes: &[xxhash::Stripe], secret: &[u8]) {
        self.xxh3_accumulate(acc, stripes, secret);
    }
    fn scramble(&self, acc: &mut [u64; xxhash::ACC_NB], key: &xxhash::Stripe) {
        self.xxh3_scramble(acc, key);
    }
}

/// \[DESIGN\] How a chunk of decimals' n are made its approximate numbers
/// (RULES.md D353): multiplied by 10^f, then by 10^-e, or divided by
/// 10^e, as of a chunk written before the factors were.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlpScale {
    /// Multiplied by the first, then by the second.
    Factored(f64, f64),
    /// Divided by it.
    Divided(f64),
}

impl AlpScale {
    /// The value of `n`, an integer held exactly.
    #[inline]
    pub fn of(self, n: f64) -> f64 {
        match self {
            AlpScale::Factored(power, fraction) => n * power * fraction,
            AlpScale::Divided(power) => n / power,
        }
    }
}

/// \[DESIGN\] The built-in kernels: every portable body, as written.
#[derive(Debug, Clone, Copy, Default)]
pub struct Portable;

/// The name of the built-in kernels.
pub const PORTABLE: &str = "PORTABLE";

impl Kernels for Portable {
    fn name(&self) -> &'static str {
        PORTABLE
    }
}
