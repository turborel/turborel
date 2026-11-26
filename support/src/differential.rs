//! Differential tests of a set of kernels against the portable bodies
//! (RULES.md D570, R-TEST-10): inputs made by a fixed-seed generator,
//! biased to the edges, of lengths around the vector widths, a word of
//! bits and a chunk of 2,048 rows, from unaligned starts, of every width
//! and operator, NaN, -0.0, the infinities and the extreme integers among
//! them, nulls of none, few, half and all rows, selections or none;
//! vectors of every element format, FLOAT16's every kind of bit pattern
//! and INT8's extremes among them, and CRCs of every short length and some
//! long ones. Each output is compared with the portable body's bit for
//! bit, a NaN distance with a NaN. A failure names
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

use turborel::kernels::{AlpScale, DistanceTerm, Kernels, Packed, Portable};
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
    let density = rng.pick(&[0, 1, 20, 25, 30, 90, 100, 101]);
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
        hashes(kernels, rng, (seed, len));
        decoding(kernels, rng, seed);
        distances(kernels, rng, seed);
        crc(kernels, rng, seed);
        blooms(kernels, rng, (seed, len));
        finds(kernels, rng, seed);
    }
}

/// Checks the Bloom filter probe of `kernels` (RULES.md D1087): of a
/// filter of any count of blocks, at times with words past its last, of
/// a density that holds few hashes or most, and of hashes of every length
/// and alignment, the places the portable probe gives.
fn blooms(kernels: &dyn Kernels, rng: &mut Rng, (seed, len): (u64, usize)) {
    use turborel::kernels::portable::bloom::{BLOCK_WORDS, bloom_probe};
    let blocks = [0, 1, 3, 64, 1_000][rng.below(5) as usize];
    let dense = rng.below(4);
    let words: Vec<u64> =
        (0..blocks * BLOCK_WORDS + rng.below(3) as usize).map(|_| (0..dense).fold(rng.next(), |w, _| w | rng.next())).collect();
    let (hashes, skip) = unaligned(rng, len, Rng::next);
    let hashes = &hashes[skip..];
    let want = appended(7, |out| bloom_probe(&words, hashes, out));
    assert_eq!(appended(7, |out| kernels.bloom_probe(&words, hashes, out)), want, "bloom_probe of {blocks} blocks, seed {seed}");
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
    // Sums and extremes of picks out of order, repeated and past the end.
    assert_eq!(kernels.sum_i64(ints, Some(&picks)), Portable.sum_i64(ints, Some(&picks)), "sum_i64 picked, seed {seed}");
    let picked = (nulls.as_ref(), Some(&picks[..]));
    assert_eq!(kernels.min_max_i64(ints, picked), Portable.min_max_i64(ints, picked), "min_max_i64 picked, seed {seed}");
    // A window of rows picked densely, from a row of any block, sometimes
    // past the end: either side of the share a masked run takes.
    let from = rng.below(len as u64 + 1) as u32;
    let share = rng.pick(&[3, 4, 5]);
    let window: Vec<u32> = (from..from + rng.below(600) as u32).filter(|_| rng.below(share) == 0).collect();
    assert_eq!(kernels.sum_i64(ints, Some(&window)), Portable.sum_i64(ints, Some(&window)), "sum_i64 window, seed {seed}");
    let windowed = (nulls.as_ref(), Some(&window[..]));
    assert_eq!(kernels.min_max_i64(ints, windowed), Portable.min_max_i64(ints, windowed), "min_max_i64 window, seed {seed}");
    let within: Vec<u32> = picks.iter().copied().filter(|&r| (r as usize) < len).collect();
    let held = (nulls.as_ref(), Some(&within[..]));
    assert_eq!(kernels.min_max_i64(ints, held), Portable.min_max_i64(ints, held), "min_max_i64 within, seed {seed}");
    let hash_seed = rng.next();
    let want = appended(7, |out| Portable.hash_i64(ints, hash_seed, sel.as_deref(), out));
    assert_eq!(appended(7, |out| kernels.hash_i64(ints, hash_seed, sel.as_deref(), out)), want, "hash_i64, seed {seed}");
}

/// Checks the hashes of `kernels`: of integers, the joins of a key's
/// columns, and of texts short, mid and long, trailing spaces among them,
/// of a zero seed and not.
fn hashes(kernels: &dyn Kernels, rng: &mut Rng, (seed, len): (u64, usize)) {
    let hash_seed = if rng.below(8) == 0 { 0 } else { rng.next() };
    let next: Vec<u64> = (0..len + rng.below(3) as usize).map(|_| rng.next()).collect();
    let mut want: Vec<u64> = (0..len).map(|_| rng.next()).collect();
    let mut got = want.clone();
    Portable.combine_i64(&mut want, &next, hash_seed);
    kernels.combine_i64(&mut got, &next, hash_seed);
    assert_eq!(got, want, "combine_i64, seed {seed}");
    let (mut text, mut ends) = (Vec::new(), Vec::new());
    for _ in 0..rng.below(200) {
        let long = [4, 16, 128, 240, 1100][rng.below(5) as usize];
        text.extend((0..rng.below(long + 1)).map(|_| if rng.below(4) == 0 { b' ' } else { b'a' + rng.below(26) as u8 }));
        ends.push(text.len());
    }
    let seeded = turborel::hash::xxhash::Seeded::new(hash_seed);
    let picks: Vec<u32> = (0..rng.below(100)).map(|_| rng.below(ends.len() as u64 + 5) as u32).collect();
    for sel in [None, Some(&picks[..])] {
        let want = appended(4, |out| Portable.hash_texts((&text, &ends), &seeded, sel, out));
        assert_eq!(appended(4, |out| kernels.hash_texts((&text, &ends), &seeded, sel, out)), want, "hash_texts, seed {seed}");
    }
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

/// A FLOAT32 element, of NaN, the zeros, the infinities, the least
/// subnormal, any bits, or a small number most often.
fn single(rng: &mut Rng) -> f32 {
    match rng.below(24) {
        0 => rng.pick(&[f32::NAN, -0.0, 0.0, f32::INFINITY, f32::NEG_INFINITY, f32::from_bits(1), f32::MAX]),
        1 => f32::from_bits(rng.next() as u32),
        _ => (rng.below(1 << 24) as f32 / (1 << 20) as f32 - 8.0) * if rng.below(2) == 0 { 1.0 } else { 1e-3 },
    }
}

/// A FLOAT16 element's bits, of NaN quiet and signalling, the zeros, the
/// infinities, subnormals, the greatest, or any bits.
fn half_bits(rng: &mut Rng) -> u16 {
    match rng.below(4) {
        0 => rng.pick(&[0x7E00, 0x7C01, 0xFE00, 0x0000, 0x8000, 0x7C00, 0xFC00, 0x0001, 0x83FF, 0x7BFF, 0xFBFF, 0x3C00]),
        _ => rng.next() as u16,
    }
}

/// An INT8 element, of the extremes, or any byte.
fn byte(rng: &mut Rng) -> u8 {
    if rng.below(4) == 0 { rng.pick(&[0x80, 0x7F, 0x00, 0xFF, 0x81]) } else { rng.next() as u8 }
}

/// Whether two distances are the same, bit for bit or both NaN, whose
/// payload no sum fixes.
fn same(got: f32, want: f32) -> bool {
    got.to_bits() == want.to_bits() || (got.is_nan() && want.is_nan())
}

/// The elements of a vector and its pair, most often of one length, at
/// times of another.
fn lengths(rng: &mut Rng) -> (usize, usize) {
    let len = if rng.below(8) == 0 { rng.pick(&[768, 1536, 2048]) } else { rng.below(200) as usize };
    (len, if rng.below(10) == 0 { rng.below(200) as usize } else { len })
}

/// `make`'s bytes of `len` elements from an unaligned start, past a
/// longer vector's first 0 to 7 bytes.
fn packed(rng: &mut Rng, len: usize, make: impl Fn(&mut Rng) -> Vec<u8>) -> (Vec<u8>, usize) {
    let skip = rng.below(8) as usize;
    let mut bytes: Vec<u8> = (0..skip).map(|_| rng.next() as u8).collect();
    for _ in 0..len {
        bytes.extend(make(rng));
    }
    (bytes, skip)
}

/// Checks the distance sums of `kernels`, of every term and format.
fn distances(kernels: &dyn Kernels, rng: &mut Rng, seed: u64) {
    let (len_a, len_b) = lengths(rng);
    let (a, skip_a) = unaligned(rng, len_a, single);
    let (b, skip_b) = unaligned(rng, len_b, single);
    let (left, right) = (&a[skip_a..], &b[skip_b..]);
    for term in DistanceTerm::ALL {
        let (got, want) = (kernels.distance_f32(left, right, term), Portable.distance_f32(left, right, term));
        assert!(same(got, want), "distance_f32 {term:?} {len_a}/{len_b}: {got} against {want}, seed {seed}");
    }
    let (got, want) = (kernels.cosine_f32(left, right), Portable.cosine_f32(left, right));
    assert!(
        same(got.0, want.0) && same(got.1, want.1) && same(got.2, want.2),
        "cosine_f32 {len_a}/{len_b}: {got:?} against {want:?}, seed {seed}"
    );
    let (single_a, skip_a) = packed(rng, len_a, |g| single(g).to_le_bytes().to_vec());
    let (single_b, skip_b) = packed(rng, len_b, |g| single(g).to_le_bytes().to_vec());
    let (half_a, half_skip_a) = packed(rng, len_a, |g| half_bits(g).to_le_bytes().to_vec());
    let (half_b, half_skip_b) = packed(rng, len_b, |g| half_bits(g).to_le_bytes().to_vec());
    let (bytes_a, bytes_skip_a) = packed(rng, len_a, |g| vec![byte(g)]);
    let (bytes_b, bytes_skip_b) = packed(rng, len_b, |g| vec![byte(g)]);
    for term in DistanceTerm::ALL {
        for (format, xs, ys) in
            [(Packed::Single, &single_a[skip_a..], &single_b[skip_b..]), (Packed::Half, &half_a[half_skip_a..], &half_b[half_skip_b..])]
        {
            let (got, want) = (kernels.distance_packed(xs, ys, (format, term)), Portable.distance_packed(xs, ys, (format, term)));
            assert!(same(got, want), "distance_packed {format:?} {term:?} {len_a}/{len_b}: {got} against {want}, seed {seed}");
        }
        let (xs, ys) = (&bytes_a[bytes_skip_a..], &bytes_b[bytes_skip_b..]);
        assert_eq!(
            kernels.distance_i8(xs, ys, term),
            Portable.distance_i8(xs, ys, term),
            "distance_i8 {term:?} {len_a}/{len_b}, seed {seed}"
        );
    }
}

/// Checks CRC-32C's register of `kernels`, of every length to 300 most often,
/// at times one past many rounds of the long streams, from an unaligned
/// start and any register.
fn crc(kernels: &dyn Kernels, rng: &mut Rng, seed: u64) {
    let len = if rng.below(8) == 0 { rng.below(80_000) as usize } else { rng.below(300) as usize };
    let (data, skip) = unaligned(rng, len, |r| r.next() as u8);
    let register = if rng.below(2) == 0 { u32::MAX } else { rng.next() as u32 };
    let data = &data[skip..];
    assert_eq!(kernels.crc32c(register, data), Portable.crc32c(register, data), "crc32c of {len} bytes, seed {seed}");
}

/// Checks the substring search of `kernels` (RULES.md D1121): of a
/// haystack of two to four letters, of every length to 300 most often, at
/// times longer, from an unaligned start, and a needle of 0 to 80 bytes
/// taken from it or not.
fn finds(kernels: &dyn Kernels, rng: &mut Rng, seed: u64) {
    let len = if rng.below(8) == 0 { rng.below(5000) as usize } else { rng.below(300) as usize };
    let letters = 2 + rng.below(3);
    let (data, skip) = unaligned(rng, len, |r| b'a' + r.below(letters) as u8);
    let haystack = &data[skip..];
    let width = rng.below(81) as usize;
    let needle: Vec<u8> = if rng.below(2) == 0 && width <= haystack.len() {
        let at = rng.below((haystack.len() - width + 1) as u64) as usize;
        haystack[at..at + width].to_vec()
    } else {
        (0..width).map(|_| b'a' + rng.below(letters) as u8).collect()
    };
    assert_eq!(
        kernels.find_bytes(haystack, &needle),
        Portable.find_bytes(haystack, &needle),
        "find_bytes of {width} bytes in {len}, seed {seed}"
    );
}
