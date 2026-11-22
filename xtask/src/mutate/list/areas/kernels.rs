//! The mutations of the kernels' use and their vector bodies (RULES.md
//! R-TEST-9, D570): a sum of integers that keeps its nulls' units, one
//! that adds by the kernels past the bound where a partial sum could
//! overflow, a gathering that drops its rows' nulls, and the vector
//! extremes of a dense selection that take rows not picked.

use super::super::{Mutation, Package};

/// The set functions over integers.
const INTS_FILE: &str = "src/functions/ints.rs";
/// The unit test of set functions fed by the kernels.
const FED: &str = "functions::tests::set_functions_fed_by_kernels_total_as_each";

/// Mechanisms whose removal must fail the kernels' requirements.
pub const KERNELS: [Mutation; 4] = [
    Mutation {
        name: "kernels-sum-nulls-kept",
        package: Package::Core,
        breaks: "a sum of integers by the kernels keeps the units under its nulls",
        file: INTS_FILE,
        original: "        Some(nulls) => nulled_sum(&ints.units, nulls.words()),",
        replacement: "        Some(_) => 0,",
        integration: &[],
        unit: &[FED],
    },
    Mutation {
        name: "kernels-sum-unbounded",
        package: Package::Core,
        breaks: "a sum started near the 128-bit bound is added by the kernels, so a partial sum that overflows is not raised",
        file: INTS_FILE,
        original: "if start.unsigned_abs() >> SAFE_BITS != 0 || !few {",
        replacement: "if !few {",
        integration: &[],
        unit: &["functions::tests::a_sum_near_the_bound_overflows_as_each"],
    },
    Mutation {
        name: "kernels-gathered-nulls-dropped",
        package: Package::Core,
        breaks: "integers gathered by the kernels lose their rows' nulls, a null read as 0",
        file: "src/storage/batch/gathered.rs",
        original: "if r == NO_ROW || ints.is_null(index(r)) {",
        replacement: "if r == NO_ROW {",
        integration: &[],
        unit: &["storage::batch::gathered::tests::integers_gathered_by_the_kernels_keep_their_nulls"],
    },
    Mutation {
        name: "simd-picks-dropped",
        package: Package::Simd,
        breaks: "the extremes of a dense selection take rows not picked",
        file: "simd/src/x86/sum.rs",
        original: "    all & picked & !null",
        replacement: "    all & !null",
        integration: &[],
        unit: &["tests::every_available_set_gives_the_portable_results"],
    },
];
