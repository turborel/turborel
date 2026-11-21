//! Differential tests of a set of kernels against the portable bodies
//! (RULES.md D570, R-TEST-10): inputs made by a fixed-seed generator,
//! biased to the edges, of lengths around the vector widths, a word of
//! bits and a chunk of 2,048 rows, from unaligned starts, of every width
//! and operator, NaN, -0.0, the infinities and the extreme integers among
//! them, nulls of none, few, half and all rows, selections or none; each
//! output compared with the portable body's bit for bit. A failure names
//! its seed, which `cargo xtask kernels --from SEED --seeds 1` runs alone.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::integer_division,
    clippy::default_numeric_fallback,
    clippy::missing_panics_doc,
    clippy::shadow_unrelated,
    clippy::should_implement_trait,
    clippy::cast_precision_loss,
    reason = "test code: a panic is a test failure, its fixtures literal (RULES.md R-TEST-4)"
)]

use std::ops::Range;

use turborel::kernels::{AlpScale, Kernels, Portable};
use turborel::storage::Mask;
use turborel::value::CompOp;

/// The seeds a test runs: `KERNEL_SEEDS` of them from `KERNEL_SEEDS_FROM`,
/// as `cargo xtask kernels` sets them, else `default` from 0.
pub fn seeds(default: u64) -> Range<u64> {
    let read = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok());
    let from = read("KERNEL_SEEDS_FROM").unwrap_or(0);
    from..from + read("KERNEL_SEEDS").unwrap_or(default)
}

/// A xorshift64* generator, its state never 0.
pub struct Rng(u64);

impl Rng {
    /// The generator of `seed`.
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// The next word.
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number below `n`, of `n` above 0.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// One of `items`.
    pub fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
}

/// Every comparison operator.
const OPS: [CompOp; 6] = [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Gt, CompOp::Le, CompOp::Ge];

/// A length: around the vector widths and a word, around a chunk, or
/// short.
fn length(rng: &mut Rng) -> usize {
    match rng.below(4) {
        0 => rng.below(20) as usize,
        1 => rng.below(300) as usize,
        2 => 2048 - 70 + rng.below(140) as usize,
        _ => 4096 + rng.below(300) as usize,
    }
}

/// An integer, of the extremes, small, or any.
fn int(rng: &mut Rng) -> i64 {
    match rng.below(6) {
        0 => rng.pick(&[i64::MIN, i64::MAX, i64::MIN + 1, i64::MAX - 1, 0, -1, 1]),
        1 | 2 => rng.below(41) as i64 - 20,
        3 => (rng.next() >> rng.below(64)) as i64,
        _ => rng.next() as i64,
    }
}

/// An approximate number, of NaN, the zeros, the infinities, small, or
/// any bits.
fn float(rng: &mut Rng) -> f64 {
    match rng.below(6) {
        0 => rng.pick(&[f64::NAN, -0.0, 0.0, f64::INFINITY, f64::NEG_INFINITY, f64::MIN, f64::MAX]),
        1 | 2 => rng.below(41) as f64 - 20.0,
        3 => (rng.below(2001) as f64 - 1000.0) / 8.0,
        _ => f64::from_bits(rng.next()),
    }
}

/// A vector of `len` made by `make`, from an unaligned start: the vector
/// is the tail of one longer by 0 to 7.
fn unaligned<T: Clone>(rng: &mut Rng, len: usize, make: impl Fn(&mut Rng) -> T) -> (Vec<T>, usize) {
    let skip = rng.below(8) as usize;
    ((0..len + skip).map(|_| make(rng)).collect(), skip)
}

/// Nulls of `len` rows, of none, few, half or all, or no mask.
fn nulls(rng: &mut Rng, len: usize) -> Option<Mask> {
    let density = match rng.below(5) {
        0 => return None,
        1 => 0,
        2 => 1,
        3 => 50,
        _ => 100,
    };
    Some(Mask::from_bools(&(0..len).map(|_| rng.below(100) < density).collect::<Vec<_>>()))
}

/// A selection of rows below `len`, ascending, or none.
fn selection(rng: &mut Rng, len: usize) -> Option<Vec<u32>> {
    let density = rng.pick(&[0, 1, 30, 90, 100, 101]);
    (density <= 100).then(|| (0..len as u32).filter(|_| rng.below(100) < density).collect())
}

/// Checks `kernels` against the portable bodies on the inputs of each of
/// `seeds`.
pub fn differential(kernels: &dyn Kernels, seeds: Range<u64>) {
    for seed in seeds {
        let rng = &mut Rng::new(seed);
        let len = length(rng);
        selections(kernels, rng, (seed, len));
        aggregates(kernels, rng, (seed, len));
        decoding(kernels, rng, seed);
    }
}

/// `out` after `run` appended to a vector holding one word, as each
/// kernel appends.
fn appended<T: Copy>(first: T, run: impl FnOnce(&mut Vec<T>)) -> Vec<T> {
    let mut out = vec![first];
    run(&mut out);
    out
}

/// Checks the selections of `kernels`.
fn selections(kernels: &dyn Kernels, rng: &mut Rng, (seed, len): (u64, usize)) {
    let (ints, skip) = unaligned(rng, len, int);
    let ints = &ints[skip..];
    let (floats, skip) = unaligned(rng, len, float);
    let floats = &floats[skip..];
    let nulls = nulls(rng, len);
    let sel = selection(rng, len);
    let given = (nulls.as_ref(), sel.as_deref());
    let op = rng.pick(&OPS);
    let c = if len > 0 && rng.below(2) == 0 { ints[rng.below(len as u64) as usize] } else { int(rng) };
    let want = appended(9, |out| Portable.select_i64(ints, (op, c), given, out));
    assert_eq!(appended(9, |out| kernels.select_i64(ints, (op, c), given, out)), want, "select_i64 {op:?} {c}, seed {seed}");
    let f = if len > 0 && rng.below(2) == 0 { floats[rng.below(len as u64) as usize] } else { float(rng) };
    let want = appended(9, |out| Portable.select_f64(floats, (op, f), given, out));
    assert_eq!(appended(9, |out| kernels.select_f64(floats, (op, f), given, out)), want, "select_f64 {op:?} {f}, seed {seed}");
    let bits: Vec<u64> = (0..len.div_ceil(64) + rng.below(2) as usize).map(|_| rng.next() & rng.next()).collect();
    let want = appended(9, |out| Portable.select_mask(&bits, given, out));
    assert_eq!(appended(9, |out| kernels.select_mask(&bits, given, out)), want, "select_mask, seed {seed}");
    let entries = rng.below(300) as u32 + 1;
    let codes: Vec<u32> = (0..len).map(|_| rng.below(u64::from(entries)) as u32).collect();
    let keep: Vec<u64> = (0..(entries as usize).div_ceil(64).saturating_sub(rng.below(2) as usize)).map(|_| rng.next()).collect();
    let want = appended(9, |out| Portable.select_codes(&codes, &keep, given, out));
    assert_eq!(appended(9, |out| kernels.select_codes(&codes, &keep, given, out)), want, "select_codes, seed {seed}");
}

/// Checks the sums, extremes, gathers and hashes of `kernels`.
fn aggregates(kernels: &dyn Kernels, rng: &mut Rng, (seed, len): (u64, usize)) {
    let (ints, skip) = unaligned(rng, len, int);
    let ints = &ints[skip..];
    let nulls = nulls(rng, len);
    let sel = selection(rng, len);
    let given = (nulls.as_ref(), sel.as_deref());
    assert_eq!(kernels.sum_i64(ints, sel.as_deref()), Portable.sum_i64(ints, sel.as_deref()), "sum_i64, seed {seed}");
    assert_eq!(kernels.min_max_i64(ints, given), Portable.min_max_i64(ints, given), "min_max_i64, seed {seed}");
    // Rows past the end among those gathered: each gives 0.
    let picks: Vec<u32> = (0..rng.below(300)).map(|_| rng.below(len as u64 + 20) as u32).collect();
    let want = appended(5, |out| Portable.gather_i64(ints, &picks, out));
    assert_eq!(appended(5, |out| kernels.gather_i64(ints, &picks, out)), want, "gather_i64, seed {seed}");
    let hash_seed = rng.next();
    let want = appended(7, |out| Portable.hash_i64(ints, hash_seed, sel.as_deref(), out));
    assert_eq!(appended(7, |out| kernels.hash_i64(ints, hash_seed, sel.as_deref(), out)), want, "hash_i64, seed {seed}");
}

/// Checks the decoding of `kernels`: unpacking, decimals and normal keys.
fn decoding(kernels: &dyn Kernels, rng: &mut Rng, seed: u64) {
    let bits = rng.below(65) as u32;
    let count = rng.below(400) as usize;
    let bytes: Vec<u8> = (0..(count * bits as usize).div_ceil(8) + rng.below(9) as usize).map(|_| rng.next() as u8).collect();
    let (from, to) = {
        let (a, b) = (rng.below(count as u64 + 2) as usize, rng.below(count as u64 + 2) as usize);
        (a.min(b), a.max(b))
    };
    let least = int(rng);
    let (mut want, mut got) = (vec![1], vec![1]);
    let made = Portable.unpack(&bytes, (bits, least), (from, to), &mut want);
    assert_eq!(kernels.unpack(&bytes, (bits, least), (from, to), &mut got), made, "unpack {bits} bits {from}..{to}, seed {seed}");
    if made.is_some() {
        assert_eq!(got, want, "unpack {bits} bits {from}..{to}, seed {seed}");
    }
    let span = 1_i64 << rng.below(52);
    let count = rng.below(300) as usize;
    let (words, skip) = unaligned(rng, count, |r| if r.below(20) == 0 { int(r) } else { int(r) % span });
    let power = 10_f64.powi(rng.below(19) as i32);
    let scale = if rng.below(2) == 0 { AlpScale::Factored(power, 10_f64.powi(-(rng.below(19) as i32))) } else { AlpScale::Divided(power) };
    let (mut want, mut got) = (words[skip..].to_vec(), words[skip..].to_vec());
    Portable.alp_decode(&mut want, scale);
    kernels.alp_decode(&mut got, scale);
    assert_eq!(got, want, "alp_decode {scale:?}, seed {seed}");
    normal_keys(kernels, rng, seed);
}

/// Checks the normal keys of `kernels`, of every field width and place.
fn normal_keys(kernels: &dyn Kernels, rng: &mut Rng, seed: u64) {
    let len = rng.below(100) as usize;
    let values: Vec<i64> = (0..len).map(|_| int(rng)).collect();
    let rows: Vec<u32> = (0..rng.below(100)).map(|_| rng.below(len as u64 + 2) as u32).collect();
    let base = if rng.below(2) == 0 { i64::MIN } else { int(rng) };
    let (bytes, invert) = (rng.below(9) as usize, rng.below(2) == 0);
    let (width, at) = (rng.below(20) as usize + 1, rng.below(12) as usize);
    let blank = vec![0xA5_u8; rows.len() * width];
    let (mut want, mut got) = (blank.clone(), blank);
    let made = Portable.normalize_i64((&values, &rows), (base, bytes, invert), (width, at), &mut want);
    assert_eq!(kernels.normalize_i64((&values, &rows), (base, bytes, invert), (width, at), &mut got), made, "normalize_i64, seed {seed}");
    assert_eq!(got, want, "normalize_i64 keys, seed {seed}");
}
