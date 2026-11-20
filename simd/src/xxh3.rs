//! \[DESIGN\] XXH3's long inputs' loops (RULES.md D460), as `xxhash.h`'s
//! `XXH3_accumulate_512_sse2`, `_avx2`, `_avx512` and `_neon` and their
//! `XXH3_scrambleAcc_*` write them: the accumulators kept in vector
//! registers across a block's stripes. Each gives exactly what the core's
//! `hash::xxhash::Scalar` gives, which is what runs of a set the processor
//! lacks.
//!
//! Ported from xxHash, whose notice it keeps, as its license asks:
//!
//! ```text
//! xxHash - Extremely Fast Hash algorithm
//! Copyright (C) 2012-2023 Yann Collet
//!
//! BSD 2-Clause License (https://www.opensource.org/licenses/bsd-license.php)
//!
//! Redistribution and use in source and binary forms, with or without
//! modification, are permitted provided that the following conditions are
//! met:
//!
//!    * Redistributions of source code must retain the above copyright
//!      notice, this list of conditions and the following disclaimer.
//!    * Redistributions in binary form must reproduce the above
//!      copyright notice, this list of conditions and the following disclaimer
//!      in the documentation and/or other materials provided with the
//!      distribution.
//!
//! THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
//! "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
//! LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
//! A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
//! OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
//! SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
//! LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
//! DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
//! THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
//! (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
//! OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
//!
//! You can contact the author at:
//!   - xxHash homepage: https://www.xxhash.com
//!   - xxHash source repository: https://github.com/Cyan4973/xxHash
//! ```

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use turborel::hash::xxhash::keyed;
use turborel::hash::xxhash::{ACC_NB, Scalar, Stripe, Stripes};

use crate::Isa;

#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(target_arch = "x86_64")]
mod x86;

/// Accumulates `stripes` into `acc` on `isa`, as `Stripes::accumulate`.
pub fn accumulate(isa: Isa, acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    match isa {
        Isa::Sse2 => accumulate_x86(X86Set::Sse2, acc, stripes, secret),
        Isa::Avx2 => accumulate_x86(X86Set::Avx2, acc, stripes, secret),
        Isa::Avx512 => accumulate_x86(X86Set::Avx512, acc, stripes, secret),
        Isa::Neon => accumulate_neon(acc, stripes, secret),
        Isa::Scalar => Scalar.accumulate(acc, stripes, secret),
    }
}

/// Scrambles `acc` by `key` on `isa`, as `Stripes::scramble`.
pub fn scramble(isa: Isa, acc: &mut [u64; ACC_NB], key: &Stripe) {
    match isa {
        Isa::Sse2 => scramble_x86(X86Set::Sse2, acc, key),
        Isa::Avx2 => scramble_x86(X86Set::Avx2, acc, key),
        Isa::Avx512 => scramble_x86(X86Set::Avx512, acc, key),
        Isa::Neon => scramble_neon(acc, key),
        Isa::Scalar => Scalar.scramble(acc, key),
    }
}

/// The x86 sets with kernels.
#[derive(Debug, Clone, Copy)]
enum X86Set {
    Sse2,
    Avx2,
    Avx512,
}

#[cfg(target_arch = "x86_64")]
fn accumulate_x86(set: X86Set, acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    let stripes = keyed(stripes, secret);
    match set {
        X86Set::Sse2 => x86::accumulate_sse2(acc, stripes, secret),
        X86Set::Avx2 => x86::accumulate_avx2(acc, stripes, secret),
        X86Set::Avx512 => x86::accumulate_avx512(acc, stripes, secret),
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn accumulate_x86(_set: X86Set, acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    Scalar.accumulate(acc, stripes, secret);
}

#[cfg(target_arch = "x86_64")]
fn scramble_x86(set: X86Set, acc: &mut [u64; ACC_NB], key: &Stripe) {
    match set {
        X86Set::Sse2 => x86::scramble_sse2(acc, key),
        X86Set::Avx2 => x86::scramble_avx2(acc, key),
        X86Set::Avx512 => x86::scramble_avx512(acc, key),
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn scramble_x86(_set: X86Set, acc: &mut [u64; ACC_NB], key: &Stripe) {
    Scalar.scramble(acc, key);
}

#[cfg(target_arch = "aarch64")]
fn accumulate_neon(acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    neon::accumulate(acc, keyed(stripes, secret), secret);
}

#[cfg(not(target_arch = "aarch64"))]
fn accumulate_neon(acc: &mut [u64; ACC_NB], stripes: &[Stripe], secret: &[u8]) {
    Scalar.accumulate(acc, stripes, secret);
}

#[cfg(target_arch = "aarch64")]
fn scramble_neon(acc: &mut [u64; ACC_NB], key: &Stripe) {
    neon::scramble(acc, key);
}

#[cfg(not(target_arch = "aarch64"))]
fn scramble_neon(acc: &mut [u64; ACC_NB], key: &Stripe) {
    Scalar.scramble(acc, key);
}
