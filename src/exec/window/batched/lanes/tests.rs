use super::*;
use crate::storage::Floats;

/// Verifies: [DESIGN] RULES.md D1289, [ISO/IEC 9075:1992 §6.5 GR1b]: a lane
/// holds 0 in a null's place whatever number its vector held there, as a
/// column store's chunk leaves a frame's least, a run's number or a
/// dictionary's first entry, so a sum over a frame, which adds every
/// place, leaves the null out; the places not null keep their numbers,
/// a vector with no null among them is taken as it is, and the nulls are
/// marked. Of approximate numbers the same.
#[test]
fn a_lane_holds_no_number_in_a_nulls_place() {
    let (held, more): (Vec<i64>, Vec<i64>) = (vec![5, 9, 7, -4], vec![3, 8]);
    let nulled = Column::Ints(Ints::new(held, 2, mask_of(&[false, true, false, true])));
    let whole = Column::Ints(Ints::new(more, 2, None));
    let Lane::Ints { units, scale, nulls } = Lane::of(&[&nulled, &whole]) else { panic!("not a lane of integers") };
    let cleared: Vec<i64> = vec![5, 0, 7, 0, 3, 8];
    assert_eq!((units, scale), (cleared, 2));
    assert_eq!(nulls, [false, true, false, true, false, false]);
    let approximate: Vec<f64> = vec![1.5, 9.25, -2.0];
    let floats = Column::Floats(Floats::new(approximate, mask_of(&[false, true, false])));
    let Lane::Floats { numbers, nulls: marked } = Lane::of(&[&floats]) else { panic!("not a lane of approximate numbers") };
    let expected: [f64; 3] = [1.5, 0.0, -2.0];
    assert_eq!(numbers.iter().map(|f| f.to_bits()).collect::<Vec<_>>(), expected.map(f64::to_bits));
    assert_eq!(marked, [false, true, false]);
}
