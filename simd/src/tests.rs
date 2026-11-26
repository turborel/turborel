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
    for level in Simd::levels() {
        let simd = Simd::with_level(level).unwrap();
        assert_eq!(simd.level(), level);
        turborel_test_support::kernels::contract(&simd);
    }
}

/// Verifies: [DESIGN] RULES.md D570: the kernels of every instruction set
/// this processor has give what the portable bodies give, bit for bit, of
/// random inputs of every length, alignment, width and operator, nulls and
/// selections (`KERNEL_SEEDS` seeds from `KERNEL_SEEDS_FROM` if set, as
/// `cargo xtask kernels` sets them).
#[test]
fn every_available_set_gives_the_portable_results() {
    for level in Simd::levels() {
        let simd = Simd::with_level(level).unwrap();
        turborel_test_support::differential::differential(&simd, turborel_test_support::differential::seeds(400));
    }
}

/// Verifies: [DESIGN] RULES.md D460, D570: a `Simd` is made only of a
/// level the processor has, whose name it keeps.
#[test]
fn a_level_lacked_makes_no_kernels() {
    for level in [Level::Sse2, Level::Avx2, Level::Avx512, Level::Neon] {
        assert_eq!(Simd::with_level(level).map(|s| s.level()), level.available().then_some(level), "{level:?}");
    }
}

/// Verifies: [DESIGN] RULES.md D460: the portable set is always there, the
/// widest available is chosen, and a set the processor lacks is refused.
#[test]
fn levels_are_detected() {
    let available = Simd::levels();
    assert_eq!(available[0], Level::Portable);
    assert_eq!(Simd::detect().level(), *available.last().unwrap());
    assert_eq!(Simd::detect().name(), SIMD);
    for level in [Level::Sse2, Level::Avx2, Level::Avx512, Level::Neon] {
        assert_eq!(Simd::with_level(level).is_some(), available.contains(&level), "{level:?}");
    }
    #[cfg(target_arch = "x86_64")]
    assert!(available.contains(&Level::Sse2));
    #[cfg(target_arch = "aarch64")]
    assert!(available.contains(&Level::Neon));
}

/// Verifies: [DESIGN] RULES.md D460: each level's loops give the portable
/// loops' hash of a whole 1 MiB input, block by block.
#[test]
fn each_set_hashes_a_large_input_as_the_portable_loops() {
    use turborel::hash::xxhash::{self, xxh3_64, xxh3_128};
    let input: Vec<u8> = (0..(1_u32 << 20_u32) + 13).map(|n| n.wrapping_mul(2_654_435_761).to_be_bytes()[0]).collect();
    for level in Simd::levels() {
        let simd = Simd::with_level(level).unwrap();
        assert_eq!(xxhash::xxh3_64_on(&simd, &input, 0), xxh3_64(&input), "{level:?}");
        assert_eq!(xxhash::xxh3_128_on(&simd, &input, 0), xxh3_128(&input), "{level:?}");
    }
}

/// Verifies: [DESIGN] RULES.md D1095: a prefetch of every instruction set
/// this processor has asks only of the values of its slice, a place past
/// the end passed by, of every length, and changes no value.
#[test]
fn a_prefetch_names_only_its_slice() {
    let (words, values) = (vec![7_u64; 100], vec![9_u32; 3]);
    for level in Simd::levels() {
        let simd = Simd::with_level(level).unwrap();
        simd.prefetch_u64(&words, &[0, 50, 99, 100, u32::MAX]);
        simd.prefetch_u32(&values, &[2, 3, u32::MAX]);
        simd.prefetch_u64(&[], &[0]);
        simd.prefetch_i64(&[1, 2], &[1, 2]);
        assert!(words.iter().all(|&w| w == 7) && values.iter().all(|&v| v == 9), "{level:?}");
    }
}
