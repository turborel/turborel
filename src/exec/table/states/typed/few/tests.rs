//! Unit tests of states of few groups fed by lanes (RULES.md D1120), of
//! totals of either width (RULES.md D1308).

use super::*;

/// Each of `totals`, as 128 bits.
fn all(totals: &Totals) -> Vec<i128> {
    (0..totals.len()).map(|g| totals.get(g).unwrap()).collect()
}

/// Of `rows` rows, each's place among `rows` integers, read in another
/// order, and its group of `width`.
fn rows_of(rows: u32, width: u32) -> (Vec<i64>, Vec<u32>, Vec<u32>) {
    let units = (0..rows).map(|n| i64::from(n).wrapping_mul(7919).wrapping_rem(1000).wrapping_sub(300)).collect();
    let places = (0..rows).map(|n| n.wrapping_mul(37).wrapping_add(11).checked_rem(rows).unwrap_or(0)).collect();
    let groups = (0..rows).map(|n| n.wrapping_mul(13).checked_rem(width).unwrap_or(0)).collect();
    (units, places, groups)
}

/// Asserts that `rows` rows of `width` groups fed by lanes to totals of
/// either form, and counted, give what the rows fed one at a time give,
/// in the form the totals had.
fn fed_as_alone(rows: u32, width: usize) {
    let (units, places, groups) = rows_of(rows, u32::try_from(width).unwrap());
    let (mut alone, mut alone_counts) = (vec![5_i128; width], vec![2_i64; width]);
    for (&at, &g) in places.iter().zip(&groups) {
        alone[index(g)] = alone[index(g)].wrapping_add(i128::from(units[index(at)]));
        alone_counts[index(g)] = alone_counts[index(g)].wrapping_add(1);
    }
    for mut totals in [Totals::Narrow(vec![5; width]), Totals::Wide(vec![5; width])] {
        let (narrow, mut counts) = (matches!(totals, Totals::Narrow(_)), vec![2_i64; width]);
        assert_eq!(summed(&mut totals, &mut counts, (&units, &places, &groups)), Some(false));
        assert_eq!((&all(&totals), &counts), (&alone, &alone_counts), "{rows} rows of {width} groups");
        assert_eq!(matches!(totals, Totals::Narrow(_)), narrow, "the form kept, of sums that fit it");
    }
    let mut rows_counted = vec![2_i64; width];
    assert!(counted(&mut rows_counted, &groups));
    assert_eq!(rows_counted, alone_counts, "{rows} rows counted");
}

/// Verifies: [DESIGN] (RULES.md D1120) sums and counts of few groups fed
/// by lanes are those of the rows fed one at a time, added to totals
/// before them, narrow and wide (RULES.md D1308), of every count of rows
/// a turn of the lanes leaves over; a total past 128 bits is told; more
/// than `FEW` groups are not fed.
#[test]
fn sums_and_counts_by_lanes_are_those_fed_one_at_a_time() {
    for rows in [0, 1, 3, 4, 5, 7, 1001] {
        for width in [1, 4, 5, FEW] {
            fed_as_alone(rows, width);
        }
    }
    let mut near = Totals::Wide(vec![i128::MAX - 1]);
    assert_eq!(summed(&mut near, &mut [0], (&[1, 1], &[0, 1], &[0, 0])), Some(true), "past 128 bits");
    let (units, places, groups) = rows_of(10, 65);
    for mut many in [Totals::Narrow(vec![0; FEW + 1]), Totals::Wide(vec![0; FEW + 1])] {
        assert_eq!(summed(&mut many, &mut vec![0; FEW + 1], (&units, &places, &groups)), None);
        assert_eq!(all(&many), vec![0; FEW + 1], "nothing added");
    }
    assert!(!counted(&mut vec![0; FEW + 1], &groups));
}

/// A batch fed to narrow totals: the totals before, the batch's integers
/// and groups, and whether the batch leaves 64 bits.
type Case<'a> = (&'a [i64], &'a [i64], &'a [u32], bool);

/// Verifies: [DESIGN] (RULES.md D1308, D1120) narrow totals of few groups
/// fed by lanes a batch that leaves 64 bits are widened and hold what the
/// rows fed one at a time in 128 bits give, each row added once: of a
/// total that leaves them as the lanes' partials are added to it, on
/// either side; of a lane's partial that leaves them though the batch's
/// sum does not; and of a group of the same batch whose own sums fit. A
/// batch that leaves nothing keeps its totals narrow, the greatest and
/// least integers of 64 bits among them, and every row is counted once.
#[test]
fn narrow_totals_a_batch_leaves_are_widened_and_fed_it_once() {
    let (most, least) = (i64::MAX, i64::MIN);
    let cases: [Case<'_>; 6] = [
        (&[most - 10, 3], &[4, 5, 6, 1, 2, 9], &[0, 1, 0, 0, 1, 0], true),
        (&[least + 10, 3], &[-4, 5, -6, -1, 2], &[0, 1, 0, 0, 1], true),
        (&[0, 3], &[most, 7, 1, 2, most, 3, 4, 5, least, 6, 7, 8, least], &[0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0], true),
        (&[most - 10, least + 10], &[4, -4, 6, -6], &[0, 1, 0, 1], false),
        (&[most, least, 5], &[], &[], false),
        (&[5, most - 1], &[most, least, 1, 1, 1], &[0, 0, 0, 1, 1], true),
    ];
    for (n, (before, units, groups, leaves)) in cases.into_iter().enumerate() {
        let places: Vec<u32> = (0..u32::try_from(units.len()).unwrap()).collect();
        let (mut alone, mut alone_counts) = (before.iter().map(|&t| i128::from(t)).collect::<Vec<_>>(), vec![1_i64; before.len()]);
        for (&unit, &g) in units.iter().zip(groups) {
            alone[index(g)] += i128::from(unit);
            alone_counts[index(g)] += 1;
        }
        let (mut totals, mut counts) = (Totals::Narrow(before.to_vec()), vec![1_i64; before.len()]);
        assert_eq!(summed(&mut totals, &mut counts, (units, &places, groups)), Some(false), "case {n}");
        assert_eq!((all(&totals), counts), (alone, alone_counts), "case {n}");
        assert_eq!(matches!(totals, Totals::Wide(_)), leaves, "case {n}: widened only by a batch that leaves 64 bits");
    }
}
