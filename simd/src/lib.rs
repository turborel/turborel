//! \[DESIGN\] Kernels on vector instructions for turborel (RULES.md D354,
//! D460). The core forbids `unsafe` (R-SAFE-1), and intrinsics need it,
//! so the kernels that run on them live in this crate: SSE2, AVX2 and
//! AVX-512 on x86-64 (`x86/`), NEON on AArch64 (`aarch64/`), the widest
//! the processor has chosen as it runs. Every kernel gives exactly what
//! the core's portable body gives, and the tests hold each level the
//! processor has to the kernels' contract.
//!
//! `turboreld` and the client register `Simd::detect()` as a database's
//! kernels (`Database::register_kernels`); EXECUTOR.KERNELS = 'PORTABLE'
//! sets them aside. A method these kernels have no body for is the
//! portable one, `Kernels`' default.
//!
//! A `Simd` exists only of a level the processor has: `detect` and
//! `with_level` are its only makers. Each `unsafe` call of a
//! `#[target_feature]` function rests on that, and each load or store of
//! a vector on the bytes of a reference it reads or writes, a SAFETY
//! comment on each block.

use turborel::hash::xxhash::{ACC_NB, Scalar, Stripe, Stripes, keyed};
use turborel::kernels::Kernels;

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "x86_64")]
mod x86;

/// The name of these kernels, as diagnostics and EXECUTOR.KERNELS show it.
pub const SIMD: &str = "SIMD";

/// \[DESIGN\] An instruction set the kernels run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// The core's portable bodies, on any processor.
    Portable,
    /// SSE2, which every x86-64 processor has.
    Sse2,
    /// AVX2.
    Avx2,
    /// AVX-512 Foundation.
    Avx512,
    /// NEON, which every AArch64 processor has.
    Neon,
}

impl Level {
    /// Whether this processor has the level.
    pub fn available(self) -> bool {
        match self {
            Level::Portable => true,
            Level::Sse2 => x86_has(X86::Sse2),
            Level::Avx2 => x86_has(X86::Avx2),
            Level::Avx512 => x86_has(X86::Avx512),
            Level::Neon => neon_has(),
        }
    }
}

/// The x86 levels.
#[derive(Debug, Clone, Copy)]
enum X86 {
    Sse2,
    Avx2,
    Avx512,
}

/// Whether this processor has `level`: never off x86-64.
#[cfg(target_arch = "x86_64")]
fn x86_has(level: X86) -> bool {
    match level {
        X86::Sse2 => std::arch::is_x86_feature_detected!("sse2"),
        X86::Avx2 => std::arch::is_x86_feature_detected!("avx2"),
        X86::Avx512 => std::arch::is_x86_feature_detected!("avx512f"),
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn x86_has(_level: X86) -> bool {
    false
}

/// Whether this processor has NEON: never off AArch64.
#[cfg(target_arch = "aarch64")]
fn neon_has() -> bool {
    std::arch::is_aarch64_feature_detected!("neon")
}

#[cfg(not(target_arch = "aarch64"))]
fn neon_has() -> bool {
    false
}

/// \[DESIGN\] The kernels of one level the processor has, as a database's
/// `Kernels`.
#[derive(Debug, Clone, Copy)]
pub struct Simd {
    level: Level,
}

impl Simd {
    /// The kernels of the widest level this processor has.
    pub fn detect() -> Simd {
        Simd { level: Simd::levels().last().copied().unwrap_or(Level::Portable) }
    }

    /// The kernels of `level`; none if this processor lacks it.
    pub fn with_level(level: Level) -> Option<Simd> {
        level.available().then_some(Simd { level })
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

impl Kernels for Simd {
    fn name(&self) -> &'static str {
        SIMD
    }
    fn xxh3_accumulate(&self, acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
        let stripes = keyed(stripes, secret);
        match self.level {
            Level::Portable => Scalar.accumulate(acc, stripes, secret),
            Level::Sse2 => on_x86(X86::Sse2, Op::Accumulate(acc, stripes, secret)),
            Level::Avx2 => on_x86(X86::Avx2, Op::Accumulate(acc, stripes, secret)),
            Level::Avx512 => on_x86(X86::Avx512, Op::Accumulate(acc, stripes, secret)),
            Level::Neon => on_neon(Op::Accumulate(acc, stripes, secret)),
        }
    }
    fn xxh3_scramble(&self, acc: &mut [u64; ACC_NB], key: &Stripe) {
        match self.level {
            Level::Portable => Scalar.scramble(acc, key),
            Level::Sse2 => on_x86(X86::Sse2, Op::Scramble(acc, key)),
            Level::Avx2 => on_x86(X86::Avx2, Op::Scramble(acc, key)),
            Level::Avx512 => on_x86(X86::Avx512, Op::Scramble(acc, key)),
            Level::Neon => on_neon(Op::Scramble(acc, key)),
        }
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

/// One of XXH3's loops, with what it works on.
enum Op<'a> {
    /// `Kernels::xxh3_accumulate` of stripes whose keys the secret holds.
    Accumulate(&'a mut [u64; ACC_NB], &'a [Stripe], &'a [u8]),
    /// `Kernels::xxh3_scramble`.
    Scramble(&'a mut [u64; ACC_NB], &'a Stripe),
}

/// Runs `op` on the x86 `level`, which a `Simd` of it proves the processor
/// has.
#[cfg(target_arch = "x86_64")]
fn on_x86(level: X86, op: Op<'_>) {
    match (level, op) {
        // SAFETY: a `Simd` of `level` exists only where the processor has it
        // (`Simd::detect`, `Simd::with_level`), all a `#[target_feature]`
        // function of it asks; the stripes are those whose keys the secret
        // holds (`keyed`), all the function reads of it.
        (X86::Sse2, Op::Accumulate(acc, stripes, secret)) => unsafe { x86::sse2::xxh3_accumulate(acc, stripes, secret) },
        // SAFETY: as above.
        (X86::Avx2, Op::Accumulate(acc, stripes, secret)) => unsafe { x86::avx2::xxh3_accumulate(acc, stripes, secret) },
        // SAFETY: as above.
        (X86::Avx512, Op::Accumulate(acc, stripes, secret)) => unsafe { x86::avx512::xxh3_accumulate(acc, stripes, secret) },
        // SAFETY: as above; a scramble reads no more than its key.
        (X86::Sse2, Op::Scramble(acc, key)) => unsafe { x86::sse2::xxh3_scramble(acc, key) },
        // SAFETY: as above.
        (X86::Avx2, Op::Scramble(acc, key)) => unsafe { x86::avx2::xxh3_scramble(acc, key) },
        // SAFETY: as above.
        (X86::Avx512, Op::Scramble(acc, key)) => unsafe { x86::avx512::xxh3_scramble(acc, key) },
    }
}

/// No x86 level is had off x86-64, so no `Simd` of one is made: the
/// portable loops, for the match to be whole.
#[cfg(not(target_arch = "x86_64"))]
fn on_x86(_level: X86, op: Op<'_>) {
    portable(op);
}

/// Runs `op` on NEON, which a `Simd` of it proves the processor has.
#[cfg(target_arch = "aarch64")]
fn on_neon(op: Op<'_>) {
    match op {
        // SAFETY: a `Simd` of NEON exists only where the processor has it
        // (`Simd::detect`, `Simd::with_level`), all the function asks.
        Op::Accumulate(acc, stripes, secret) => unsafe { aarch64::neon::xxh3_accumulate(acc, stripes, secret) },
        // SAFETY: as above.
        Op::Scramble(acc, key) => unsafe { aarch64::neon::xxh3_scramble(acc, key) },
    }
}

/// No NEON off AArch64, so no `Simd` of it is made: the portable loops.
#[cfg(not(target_arch = "aarch64"))]
fn on_neon(op: Op<'_>) {
    portable(op);
}

/// `op` on the core's portable loops.
fn portable(op: Op<'_>) {
    match op {
        Op::Accumulate(acc, stripes, secret) => Scalar.accumulate(acc, stripes, secret),
        Op::Scramble(acc, key) => Scalar.scramble(acc, key),
    }
}

// Unit tests: every level against the portable loops, and the kernels'
// contract (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
