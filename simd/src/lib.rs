//! \[DESIGN\] Kernels on vector instructions for turborel (RULES.md D354,
//! D460, D570). The core forbids `unsafe` (R-SAFE-1), and intrinsics need
//! it, so the kernels that run on them live in this crate: SSE2, AVX2 and
//! AVX-512 on x86-64 (`x86/`), NEON on AArch64 (`aarch64/`), the widest
//! the processor has chosen as it runs, and CRC-32C on the instructions
//! of each that carry its register (`crc.rs`). Every kernel gives exactly what
//! the core's portable body gives, and the tests hold each level the
//! processor has to the kernels' contract and to the portable bodies, of
//! random inputs of every length and alignment.
//!
//! `turboreld` registers `Simd::detect()` as a database's
//! kernels (`Database::register_kernels`); EXECUTOR.KERNELS = 'PORTABLE'
//! sets them aside. A method these kernels have no body for is the
//! portable one, `Kernels`' default.
//!
//! A `Simd` exists only of a level the processor has: `detect` and
//! `with_level` are its only makers, and the instruction set it holds
//! (`arch::Isa`) is made only by `arch::isa`, of a level detected. Each
//! `unsafe` call of a `#[target_feature]` function rests on that, and each
//! load or store of a vector on the bytes of a reference it reads or
//! writes, a SAFETY comment on each block.

use turborel::hash::xxhash::{ACC_NB, Seeded, Stripe, Stripes};
use turborel::kernels::{AlpScale, DistanceTerm, Given, Kernels, Packed, portable};
use turborel::storage::mask::index;
use turborel::value::CompOp;

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
use aarch64 as arch;
#[cfg(target_arch = "x86_64")]
mod x86;
#[cfg(target_arch = "x86_64")]
use x86 as arch;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
mod generic;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
use generic as arch;

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod crc;
mod rows;

/// The name of these kernels, as diagnostics and EXECUTOR.KERNELS show it.
pub const SIMD: &str = "SIMD";

/// \[DESIGN\] An instruction set the kernels run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// The core's portable bodies, on any processor.
    Portable,
    /// SSE2, which every x86-64 processor has.
    Sse2,
    /// AVX2, with what every processor of it has beside it (x86-64-v3:
    /// BMI1, BMI2, POPCNT, LZCNT, FMA, F16C, SSE4.2).
    Avx2,
    /// AVX-512 Foundation, BW, DQ, VL and CD, on top of AVX2's (x86-64-v4).
    Avx512,
    /// NEON, which every AArch64 processor has.
    Neon,
}

impl Level {
    /// Whether this processor has the level.
    pub fn available(self) -> bool {
        arch::isa(self).is_some()
    }
}

/// \[DESIGN\] The kernels of one level the processor has, as a database's
/// `Kernels`.
#[derive(Debug, Clone, Copy)]
pub struct Simd {
    level: Level,
    /// The level's instructions, made only of a level detected.
    isa: arch::Isa,
}

impl Simd {
    /// The kernels of the widest level this processor has.
    pub fn detect() -> Simd {
        let widest = Simd::levels().last().copied().unwrap_or(Level::Portable);
        Simd::with_level(widest).unwrap_or(Simd { level: Level::Portable, isa: arch::PORTABLE })
    }

    /// The kernels of `level`; none if this processor lacks it.
    pub fn with_level(level: Level) -> Option<Simd> {
        arch::isa(level).map(|isa| Simd { level, isa })
    }

    /// Every level this processor has, the portable one first.
    pub fn levels() -> Vec<Level> {
        [Level::Portable, Level::Sse2, Level::Avx2, Level::Avx512, Level::Neon].into_iter().filter(|l| l.available()).collect()
    }

    /// The level they run on.
    pub fn level(&self) -> Level {
        self.level
    }
}

/// The bytes past which XXH3 hashes an input by its stripes, at most of
/// which its short and mid paths hash it with no stripe.
const LONG_PAST: usize = 240;

impl Kernels for Simd {
    fn name(&self) -> &'static str {
        SIMD
    }
    fn select_i64(&self, values: &[i64], pred: (CompOp, i64), given: Given<'_>, out: &mut Vec<u32>) {
        arch::select_i64(self.isa, values, pred, given, out);
    }
    fn select_f64(&self, values: &[f64], pred: (CompOp, f64), given: Given<'_>, out: &mut Vec<u32>) {
        arch::select_f64(self.isa, values, pred, given, out);
    }
    fn select_mask(&self, bits: &[u64], given: Given<'_>, out: &mut Vec<u32>) {
        arch::select_mask(self.isa, bits, given, out);
    }
    fn select_codes(&self, codes: &[u32], keep: &[u64], given: Given<'_>, out: &mut Vec<u32>) {
        arch::select_codes(self.isa, codes, keep, given, out);
    }
    fn sum_i64(&self, values: &[i64], sel: Option<&[u32]>) -> i128 {
        arch::sum_i64(self.isa, values, sel)
    }
    fn min_max_i64(&self, values: &[i64], given: Given<'_>) -> Option<(i64, i64)> {
        arch::min_max_i64(self.isa, values, given)
    }
    fn gather_i64(&self, values: &[i64], sel: &[u32], out: &mut Vec<i64>) {
        arch::gather_i64(self.isa, values, sel, out);
    }
    fn hash_i64(&self, values: &[i64], seed: u64, sel: Option<&[u32]>, out: &mut Vec<u64>) {
        arch::hash_i64(self.isa, values, seed, sel, out);
    }
    fn combine_i64(&self, hashes: &mut [u64], next: &[u64], seed: u64) {
        arch::combine_i64(self.isa, hashes, next, seed);
    }
    fn hash_texts(&self, given: (&[u8], &[usize]), seeded: &Seeded, sel: Option<&[u32]>, out: &mut Vec<u64>) {
        // Each text one at a time, as the portable body hashes it; a long
        // one's stripes by these kernels.
        let hashed = |n: usize| {
            let text = portable::hash::text_at(given, n);
            if text.len() > LONG_PAST { seeded.bytes_on(self, text) } else { seeded.bytes(text) }
        };
        match sel {
            Some(sel) => out.extend(sel.iter().map(|&r| hashed(index(r)))),
            None => out.extend((0..given.1.len()).map(hashed)),
        }
    }
    fn alp_decode(&self, words: &mut [i64], scale: AlpScale) {
        arch::alp_decode(self.isa, words, scale);
    }
    fn normalize_i64(&self, given: (&[i64], &[u32]), shape: (i64, usize, bool), place: (usize, usize), keys: &mut [u8]) -> Option<()> {
        arch::normalize_i64(self.isa, given, shape, place, keys)
    }
    fn xxh3_accumulate(&self, acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
        arch::xxh3_accumulate(self.isa, acc, stripes, secret);
    }
    fn xxh3_scramble(&self, acc: &mut [u64; ACC_NB], key: &Stripe) {
        arch::xxh3_scramble(self.isa, acc, key);
    }
    fn distance_f32(&self, a: &[f32], b: &[f32], term: DistanceTerm) -> f32 {
        arch::distance_f32(self.isa, a, b, term)
    }
    fn cosine_f32(&self, a: &[f32], b: &[f32]) -> (f32, f32, f32) {
        arch::cosine_f32(self.isa, a, b)
    }
    fn distance_packed(&self, a: &[u8], b: &[u8], shape: (Packed, DistanceTerm)) -> f32 {
        arch::distance_packed(self.isa, a, b, shape)
    }
    fn distance_i8(&self, a: &[u8], b: &[u8], term: DistanceTerm) -> i64 {
        arch::distance_i8(self.isa, a, b, term)
    }
    fn crc32c(&self, register: u32, data: &[u8]) -> u32 {
        arch::crc32c(self.isa, register, data)
    }
}

/// XXH3 of long inputs on these kernels: `hash::xxhash::xxh3_64_on(&simd, ...)`.
impl Stripes for Simd {
    fn accumulate(&self, acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
        self.xxh3_accumulate(acc, stripes, secret);
    }
    fn scramble(&self, acc: &mut [u64; ACC_NB], key: &Stripe) {
        self.xxh3_scramble(acc, key);
    }
}

// Unit tests: every level against the portable bodies, and the kernels'
// contract (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
