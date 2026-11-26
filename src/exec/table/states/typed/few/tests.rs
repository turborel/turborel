//! Unit tests of states of few groups fed by lanes (RULES.md D1120).

use super::*;

/// Of `rows` rows, each's place among `rows` integers, read in another
/// order, and its group of `width`.
fn rows_of(rows: u32, width: u32) -> (Vec<i64>, Vec<u32>, Vec<u32>) {
    let units = (0..rows).map(|n| i64::from(n).wrapping_mul(7919).wrapping_rem(1000).wrapping_sub(300)).collect();
    let places = (0..rows).map(|n| n.wrapping_mul(37).wrapping_add(11).checked_rem(rows).unwrap_or(0)).collect();
    let groups = (0..rows).map(|n| n.wrapping_mul(13).checked_rem(width).unwrap_or(0)).collect();
    (units, places, groups)
}

/// Verifies: [DESIGN] (RULES.md D1120) sums and counts of few groups fed
/// by lanes are those of the rows fed one at a time, added to totals
/// before them, of every count of rows a turn of the lanes leaves over;
/// a total past 128 bits is told; more than `FEW` groups are not fed.
#[test]
fn sums_and_counts_by_lanes_are_those_fed_one_at_a_time() {
    for rows in [0, 1, 3, 4, 5, 7, 1001] {
        for width in [1, 4, 5, FEW] {
            let width_u32 = u32::try_from(width).unwrap();
            let (units, places, groups) = rows_of(rows, width_u32);
            let (mut totals, mut counts) = (vec![5_i128; width], vec![2_i64; width]);
            let (mut alone, mut alone_counts) = (totals.clone(), counts.clone());
            for (&at, &g) in places.iter().zip(&groups) {
                alone[index(g)] = alone[index(g)].wrapping_add(i128::from(units[index(at)]));
                alone_counts[index(g)] = alone_counts[index(g)].wrapping_add(1);
            }
            assert_eq!(summed(&mut totals, &mut counts, (&units, &places, &groups)), Some(false));
            assert_eq!((&totals, &counts), (&alone, &alone_counts), "{rows} rows of {width} groups");
            let mut rows_counted = vec![2_i64; width];
            assert!(counted(&mut rows_counted, &groups));
            assert_eq!(rows_counted, alone_counts, "{rows} rows counted");
        }
    }
    let mut near = vec![i128::MAX - 1];
    assert_eq!(summed(&mut near, &mut [0], (&[1, 1], &[0, 1], &[0, 0])), Some(true), "past 128 bits");
    let (units, places, groups) = rows_of(10, 65);
    assert_eq!(summed(&mut vec![0; FEW + 1], &mut vec![0; FEW + 1], (&units, &places, &groups)), None);
    assert!(!counted(&mut vec![0; FEW + 1], &groups));
}
