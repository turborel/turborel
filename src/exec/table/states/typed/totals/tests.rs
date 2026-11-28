//! Unit tests of exact totals held in 64 bits and widened (RULES.md
//! D1308): a narrow column and a wide one hold the same integers after
//! the same adds and values, a narrow one widens at the first that leaves
//! 64 bits and not before, and each charges the width of its form.

use super::*;
use crate::storage::Mask;

/// The greatest and least integers of 64 bits, as 128.
const MOST: i128 = 9_223_372_036_854_775_807;
const LEAST: i128 = -9_223_372_036_854_775_808;

/// Each of `totals`, as 128 bits.
fn all(totals: &Totals) -> Vec<i128> {
    (0..totals.len()).map(|g| totals.get(g).unwrap()).collect()
}

/// Whether `totals` are held in 64 bits.
fn narrow(totals: &Totals) -> bool {
    matches!(totals, Totals::Narrow(_))
}

/// A step of a column of three groups: a group, units, and whether they
/// are set as its total, not added to it.
type Step = (usize, i128, bool);

/// Verifies: [DESIGN] (RULES.md D1308) a narrow column of totals and a
/// wide one hold the same integers after the same adds and values set,
/// those of a model in 128 bits. The narrow one stays narrow while every
/// total fits 64 bits, the greatest and least of them among its totals,
/// and is wide from the first add whose sum leaves them, on either side,
/// the first add of units they do not hold, and the first value set they
/// do not hold, with the totals it held before.
#[test]
fn narrow_and_wide_totals_are_the_same_integers() {
    // Each case: its steps, and the step the narrow column widens at.
    let cases: [(&[Step], Option<usize>); 6] = [
        (&[(0, 5, false), (1, -7, false), (0, MOST - 5, false), (1, LEAST + 7, false), (2, MOST, true), (2, LEAST, true)], None),
        (&[(0, MOST, false), (1, -3, false), (0, 1, false), (0, -2, false), (2, 9, false)], Some(2)),
        (&[(1, LEAST, false), (0, 4, false), (1, -1, false), (1, 1, false)], Some(2)),
        (&[(0, 4, false), (2, MOST + 1, false), (0, 1, false)], Some(1)),
        (&[(0, 4, false), (2, LEAST - 1, false), (2, 1, false)], Some(1)),
        (&[(1, 6, true), (0, MOST + 1, true), (1, 2, false), (0, LEAST - 1, true)], Some(1)),
    ];
    for (n, (steps, widens)) in cases.into_iter().enumerate() {
        let (mut held, mut wide, mut model) = (Totals::new(), Totals::Wide(Vec::new()), vec![0_i128; 3]);
        held.grow_by(2);
        held.grow();
        wide.grow_by(2);
        wide.grow();
        for (at, &(g, units, set)) in steps.iter().enumerate() {
            for totals in [&mut held, &mut wide] {
                if set { totals.set(g, units) } else { totals.add(g, units).unwrap() }
            }
            model[g] = if set { units } else { model[g] + units };
            assert_eq!((all(&held), all(&wide)), (model.clone(), model.clone()), "case {n}, step {at}");
            assert_eq!(narrow(&held), widens.is_none_or(|first| at < first), "case {n}, step {at}: the form");
            assert!(!narrow(&wide), "a wide column stays wide");
        }
    }
}

/// Verifies: [DESIGN] (RULES.md D1308, D356) a sum past 128 bits is
/// raised (SQLCODE -900) by the add in 128 bits, of a wide column and of
/// a narrow one given units 64 bits do not hold, on either side, and
/// leaves the total as it was; an add or a value of a group the column
/// does not hold changes nothing, and widens nothing.
#[test]
fn a_sum_past_128_bits_is_raised_by_the_wide_add() {
    for (first, then) in [(i128::MAX, 1), (i128::MIN, -1), (MOST, i128::MAX), (LEAST, i128::MIN)] {
        let mut totals = Totals::new();
        totals.grow_by(2);
        totals.add(1, first).unwrap();
        assert_eq!(totals.add(1, then).unwrap_err().sqlcode(), -900_i32);
        assert_eq!(all(&totals), [0, first], "the total as it was");
    }
    let mut totals = Totals::new();
    totals.grow();
    totals.add(1, i128::MAX).unwrap();
    totals.set(1, i128::MAX);
    assert!(narrow(&totals) && all(&totals) == [0], "a group it does not hold");
    let mut wide = Totals::Wide(vec![3]);
    wide.add(1, 4).unwrap();
    wide.set(1, 4);
    assert_eq!(all(&wide), [3]);
}

/// Verifies: [DESIGN] (RULES.md D1308) a column of totals charges the
/// width of its form, 8 bytes a total it has room for while narrow and 16
/// once wide, with the room it had; gathered, it gives the totals at the
/// places in their order, a place past its end as nothing, in the form it
/// holds them in; kept to its first groups it keeps their totals in the
/// room they take; and a wide column widened again is as it was.
#[test]
fn a_column_of_totals_charges_its_form_and_gathers_in_it() {
    let mut totals = Totals::new();
    totals.reserve(10);
    totals.grow_by(4);
    for (g, units) in [(0, 7), (1, MOST), (2, LEAST), (3, -1)] {
        totals.set(g, units);
    }
    let room = totals.bytes().checked_div(size_of::<i64>()).unwrap();
    assert!(room >= 10 && totals.bytes() == room * 8, "8 bytes a total, of the room reserved");
    let gathered = totals.gathered(&[3, 9, 1]);
    assert!(narrow(&gathered) && all(&gathered) == [-1, 0, MOST]);
    totals.widen();
    assert!(!narrow(&totals) && all(&totals) == [7, MOST, LEAST, -1], "widened with its totals");
    assert_eq!(totals.bytes(), room * 16, "16 bytes a total once wide, of the room it had");
    totals.widen();
    assert_eq!((totals.bytes(), all(&totals)), (room * 16, vec![7, MOST, LEAST, -1]), "widened again, as it was");
    let again = totals.gathered(&[2, 9, 0]);
    assert!(!narrow(&again) && all(&again) == [LEAST, 0, 7]);
    assert!(!narrow(&totals.gathered(&[])), "the form kept of no group");
    totals.truncate(2);
    assert_eq!((totals.bytes(), all(&totals)), (2 * 16, vec![7, MOST]));
    let mut kept = Totals::Narrow(vec![4, 5, 6]);
    kept.truncate(1);
    assert_eq!((kept.bytes(), all(&kept)), (8, vec![4]));
}

/// Verifies: [DESIGN] (RULES.md D1308) the rows a
/// narrow column takes in 64 bits are those before the first whose sum
/// leaves them. It stops at that row, on either side, with every row
/// before it added and counted, a null among them passed over and told,
/// and neither that row nor those after it taken; with no such row it
/// takes every row, a total left at the greatest integer of 64 bits; and
/// a wide column takes none.
#[test]
fn a_narrow_column_takes_the_rows_before_the_first_that_leaves_64_bits() {
    let nulls = Mask::from_bools(&[false, false, true, false, false, false, false]);
    let ints = Ints::new(vec![5, i64::MAX - 9, 77, 4, 1, -6, 2], 0, Some(nulls));
    let (places, groups): ([u32; 7], [u32; 7]) = ([1, 0, 2, 3, 4, 5, 6], [0, 0, 1, 0, 0, 1, 0]);
    // Group 0 takes MAX - 9, 5, 4 and then 1, which leaves 64 bits.
    let (mut totals, mut counts) = (Totals::Narrow(vec![0, 0]), vec![0_i64; 2]);
    assert_eq!(totals.fed(&mut counts, &ints, (&places, &groups)), (4, true), "stopped at the row that leaves");
    assert_eq!((all(&totals), &counts), (vec![MOST, 0], &vec![3, 0]), "the rows before it, the null not counted");
    assert!(narrow(&totals), "not widened by the rows it took");
    let (mut low, mut low_counts) = (Totals::Narrow(vec![0, i64::MIN + 5]), vec![0_i64; 2]);
    assert_eq!(low.fed(&mut low_counts, &ints, (&places[3..], &groups[3..])), (2, false), "stopped below the least");
    assert_eq!((all(&low), low_counts), (vec![5, LEAST + 5], vec![2, 0]));
    let (mut whole, mut whole_counts) = (Totals::Narrow(vec![-3, 0]), vec![0_i64; 2]);
    assert_eq!(whole.fed(&mut whole_counts, &ints, (&places, &groups)), (7, true), "every row");
    assert_eq!((all(&whole), whole_counts), (vec![MOST, -6], vec![5, 1]));
    let (mut wide, mut wide_counts) = (Totals::Wide(vec![0, 0]), vec![0_i64; 2]);
    assert_eq!(wide.fed(&mut wide_counts, &ints, (&places, &groups)), (0, false), "a wide column takes none");
    assert_eq!((all(&wide), wide_counts), (vec![0, 0], vec![0, 0]));
}
