//! Unit tests of the vector kernels: each instruction set this processor
//! has keeps the kernels' contract, its results those of the portable
//! loops bit for bit (RULES.md R-TEST-1, R-TEST-2).

use super::*;

/// Verifies: [DESIGN] RULES.md D354, D460: the kernels of every
/// instruction set this processor has keep the contract of `Kernels`, XXH3's
/// loops giving what the portable ones give of every stripe count, secret
/// and length.
#[test]
fn every_available_set_keeps_the_contract() {
    for isa in Isa::all_available() {
        let simd = Simd::of(isa).unwrap();
        assert_eq!(simd.isa(), isa);
        turborel_test_support::kernels::contract(&simd);
    }
}

/// Verifies: [DESIGN] RULES.md D460: the portable set is always there, the
/// widest available is chosen, and a set the processor lacks is refused.
#[test]
fn sets_are_detected() {
    let available = Isa::all_available();
    assert_eq!(available[0], Isa::Scalar);
    assert_eq!(Isa::detect(), *available.last().unwrap());
    assert_eq!(Simd::detect().isa(), Isa::detect());
    assert_eq!(Simd::detect().name(), SIMD);
    for isa in [Isa::Sse2, Isa::Avx2, Isa::Avx512, Isa::Neon] {
        assert_eq!(Simd::of(isa).is_some(), available.contains(&isa), "{isa:?}");
    }
    #[cfg(target_arch = "x86_64")]
    assert!(available.contains(&Isa::Sse2));
    #[cfg(target_arch = "aarch64")]
    assert!(available.contains(&Isa::Neon));
}

/// Verifies: [DESIGN] RULES.md D460: each set's loops, called directly,
/// give the portable loops' accumulators over a whole 1 MiB input hashed
/// block by block, and a set the processor lacks gives them too.
#[test]
fn each_set_hashes_a_large_input_as_the_portable_loops() {
    use turborel::hash::xxhash::{self, xxh3_64, xxh3_128};
    let input: Vec<u8> = (0..(1_u32 << 20_u32) + 13).map(|n| n.wrapping_mul(2_654_435_761).to_be_bytes()[0]).collect();
    for isa in [Isa::Scalar, Isa::Sse2, Isa::Avx2, Isa::Avx512, Isa::Neon] {
        let simd = Simd { isa };
        assert_eq!(xxhash::xxh3_64_on(&simd, &input, 0), xxh3_64(&input), "{isa:?}");
        assert_eq!(xxhash::xxh3_128_on(&simd, &input, 0), xxh3_128(&input), "{isa:?}");
    }
}
