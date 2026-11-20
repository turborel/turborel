//! \[DESIGN\] Kernels on vector instructions for turborel (RULES.md D354,
//! D460). The core forbids `unsafe` (R-SAFE-1), and intrinsics need it,
//! so their loops live in this crate: SSE2, AVX2 and AVX-512 on x86-64,
//! NEON on AArch64, the widest the processor has chosen when it runs.
//! Every kernel gives exactly what the core's portable loop gives, and the
//! tests hold each one available to it, on every input shape.
//!
//! A database takes them as its `Kernels` (`Database::set_kernels`):
//! `Simd::detect()` overrides the methods it has kernels for and leaves
//! the rest to the portable bodies. Each family of kernels is a module
//! with a small safe interface of its own: `xxh3` now, so CRC32C, Bloom
//! filters' probes and decoders can join beside it.
//!
//! Each `unsafe` block holds one call: a `#[target_feature]` function,
//! reached only after the feature was detected, or a load or store of a
//! vector within a reference's bytes.

use turborel::hash::xxhash::{ACC_NB, Stripe, Stripes};
use turborel::kernels::Kernels;

pub mod xxh3;

/// \[DESIGN\] An instruction set the kernels may run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isa {
    /// The core's portable loops, on any processor.
    Scalar,
    /// SSE2, which every x86-64 processor has.
    Sse2,
    /// AVX2.
    Avx2,
    /// AVX-512 Foundation.
    Avx512,
    /// NEON, which every AArch64 processor has.
    Neon,
}

/// The name of these kernels, as diagnostics show it.
pub const SIMD: &str = "SIMD";

impl Isa {
    /// Whether this processor has the set.
    pub fn available(self) -> bool {
        match self {
            Isa::Scalar => true,
            Isa::Sse2 => x86_has(X86::Sse2),
            Isa::Avx2 => x86_has(X86::Avx2),
            Isa::Avx512 => x86_has(X86::Avx512),
            Isa::Neon => neon_has(),
        }
    }

    /// Every set this processor has, the portable one first.
    pub fn all_available() -> Vec<Isa> {
        [Isa::Scalar, Isa::Sse2, Isa::Avx2, Isa::Avx512, Isa::Neon].into_iter().filter(|isa| isa.available()).collect()
    }

    /// The widest set this processor has.
    pub fn detect() -> Isa {
        Isa::all_available().last().copied().unwrap_or(Isa::Scalar)
    }
}

/// The x86 sets the kernels use.
#[derive(Debug, Clone, Copy)]
enum X86 {
    Sse2,
    Avx2,
    Avx512,
}

/// Whether this processor has `set`: never off x86-64.
#[cfg(target_arch = "x86_64")]
fn x86_has(set: X86) -> bool {
    match set {
        X86::Sse2 => std::arch::is_x86_feature_detected!("sse2"),
        X86::Avx2 => std::arch::is_x86_feature_detected!("avx2"),
        X86::Avx512 => std::arch::is_x86_feature_detected!("avx512f"),
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn x86_has(_set: X86) -> bool {
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

/// \[DESIGN\] The kernels of one instruction set, as a database's
/// `Kernels`.
#[derive(Debug, Clone, Copy)]
pub struct Simd {
    isa: Isa,
}

impl Simd {
    /// The kernels of the widest set this processor has.
    pub fn detect() -> Simd {
        Simd { isa: Isa::detect() }
    }

    /// The kernels of `isa`, if this processor has it.
    pub fn of(isa: Isa) -> Option<Simd> {
        isa.available().then_some(Simd { isa })
    }

    /// The set they run on.
    pub fn isa(&self) -> Isa {
        self.isa
    }
}

impl Kernels for Simd {
    fn name(&self) -> &'static str {
        SIMD
    }
    fn xxh3_accumulate(&self, acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
        xxh3::accumulate(self.isa, acc, stripes, secret);
    }
    fn xxh3_scramble(&self, acc: &mut [u64; ACC_NB], key: &Stripe) {
        xxh3::scramble(self.isa, acc, key);
    }
}

/// XXH3 of long inputs on these kernels: `hash::xxhash::xxh3_64_on(&simd, ...)`.
impl Stripes for Simd {
    fn accumulate(&self, acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
        xxh3::accumulate(self.isa, acc, stripes, secret);
    }
    fn scramble(&self, acc: &mut [u64; ACC_NB], key: &Stripe) {
        xxh3::scramble(self.isa, acc, key);
    }
}

// Unit tests: every kernel against the portable loop, and the kernels'
// contract (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
