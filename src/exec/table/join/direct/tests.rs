//! Unit tests of a direct table: its span at the bound, keys negative,
//! fractional, out of the span and past 2^51, and a key's rows in build
//! order (RULES.md D601, R-TEST-1).

use super::*;

/// The bits of the number `f`.
fn bits(f: f64) -> u64 {
    f.to_bits()
}

/// The rows a probe of `f` finds of `table`.
fn found(table: &Dense, f: f64) -> Vec<u32> {
    table.candidates(bits(f)).to_vec()
}

/// Verifies: [DESIGN] RULES.md D601: a span of exactly `DENSE_SPAN` times
/// the rows makes a direct table, one wider none; the rows a miss marks
/// count for neither.
#[test]
fn a_span_of_four_times_the_rows_is_direct() {
    let at_bound: Vec<u64> = [0.0_f64, 7.0_f64].into_iter().map(bits).collect();
    assert!(Dense::of(&at_bound, &[false, false]).is_some(), "span 8 of 2 rows");
    let past: Vec<u64> = [0.0_f64, 8.0_f64].into_iter().map(bits).collect();
    assert!(Dense::of(&past, &[false, false]).is_none(), "span 9 of 2 rows");
    let missed: Vec<u64> = [0.0_f64, 7.0_f64, 1e9_f64].into_iter().map(bits).collect();
    assert!(Dense::of(&missed, &[false, false, true]).is_some(), "a miss is no key");
    assert!(Dense::of(&[], &[]).is_none(), "no rows");
}

/// Verifies: [DESIGN] RULES.md D601: the rows of a key come in the order
/// they were built; negative keys are placed; a fraction, a number out of
/// the span and -0 as 0 are probed as a key's number says.
#[test]
fn a_direct_table_finds_a_keys_rows_in_build_order() {
    let keys = [3.0_f64, -2.0_f64, 3.0_f64, 0.0_f64, -2.0_f64, 3.0_f64, 1.0_f64];
    let numbers: Vec<u64> = keys.into_iter().map(bits).collect();
    let table = Dense::of(&numbers, &[false; 7]).unwrap();
    assert_eq!(found(&table, 3.0_f64), [0, 2, 5]);
    assert_eq!(found(&table, -2.0_f64), [1, 4]);
    assert_eq!(found(&table, 0.0_f64), [3]);
    assert_eq!(found(&table, -0.0_f64), [3], "-0 as 0");
    assert!(found(&table, 2.0_f64).is_empty(), "a key of no row");
    assert!(found(&table, 1.5_f64).is_empty(), "a fraction");
    assert!(found(&table, 4.0_f64).is_empty() && found(&table, -3.0_f64).is_empty(), "out of the span");
    assert!(found(&table, f64::NAN).is_empty() && found(&table, f64::INFINITY).is_empty());
}

/// Verifies: [DESIGN] RULES.md D601: the rows of a null key whose nulls
/// match are kept apart in build order, found by `NULL_NUMBER` alone, and
/// count for no span; a key of nulls alone makes no direct table.
#[test]
fn null_keys_are_kept_apart() {
    let numbers = [bits(5.0_f64), NULL_NUMBER, bits(6.0_f64), NULL_NUMBER, bits(1e12_f64)];
    let table = Dense::of(&numbers, &[false, false, false, false, true]).unwrap();
    assert_eq!(table.candidates(NULL_NUMBER), [1, 3]);
    assert_eq!(found(&table, 5.0_f64), [0]);
    assert!(found(&table, f64::NAN).is_empty(), "another not-a-number");
    assert!(Dense::of(&[NULL_NUMBER, NULL_NUMBER], &[false, false]).is_none());
}

/// Verifies: [DESIGN] RULES.md D601: a key of 2^51 or more in magnitude,
/// or a fraction, makes no direct table; the extremes of `i64` overflow
/// no span.
#[test]
fn keys_past_exact_whole_numbers_are_not_direct() {
    assert!(Dense::of(&[bits(2_251_799_813_685_248.0_f64)], &[false]).is_none());
    assert!(Dense::of(&[bits(-2_251_799_813_685_248.0_f64)], &[false]).is_none());
    assert!(Dense::of(&[bits(0.5_f64)], &[false]).is_none());
    let near: Vec<u64> = [2_251_799_813_685_247.0_f64, 2_251_799_813_685_246.0_f64].into_iter().map(bits).collect();
    let table = Dense::of(&near, &[false, false]).unwrap();
    assert_eq!(found(&table, 2_251_799_813_685_247.0_f64), [0]);
    let wide: Vec<u64> = [-2_251_799_813_685_247.0_f64, 2_251_799_813_685_247.0_f64].into_iter().map(bits).collect();
    assert!(Dense::of(&wide, &[false, false]).is_none());
}
