//! Unit tests of exact states whose totals are held in 64 bits and widened
//! (RULES.md D1308): fed, merged from tables and parts of either form,
//! and finished across the widening, each group gives what its
//! accumulator gives of the same values in the same order, and a column
//! is charged the width of its form.

use super::*;
use crate::functions::Accumulator;
use crate::storage::mask::index;

/// The type of a NUMERIC(19, 0) column, whose integers fill 64 bits.
const WHOLE: DataType = DataType::Exact { precision: 19, scale: 0, spelled: Spelled::Numeric };

/// The greatest and least integers of 64 bits, as 128.
const MOST: i128 = 9_223_372_036_854_775_807;
const LEAST: i128 = -9_223_372_036_854_775_808;

/// More groups than lanes feed (`typed/few.rs`), whose rows are fed in
/// turn.
const MANY: usize = 70;

/// The bytes a group takes of a column of exact states, its total and its
/// count: of a narrow column and of a wide one.
const NARROW: usize = 16;
const WIDE: usize = 24;

/// The four set functions kept in a column of exact states.
const NAMES: [&str; 4] = ["SUM", "AVG", "MIN", "MAX"];

/// A batch's rows: each its group and its value, none of a null.
type Rows = [(u32, Option<i128>)];

/// Rows that leave every total within 64 bits, a null among them, the
/// third group given no value.
const FIT: &Rows = &[(0, Some(MOST - 10)), (1, Some(-5)), (2, None), (1, Some(LEAST + 10))];

/// Rows with values 64 bits do not hold, on either side.
const PAST: &Rows = &[(0, Some(MOST + 7)), (1, Some(4)), (2, Some(LEAST - 7))];

/// Rows that fit, whose sums with `FIT`'s leave 64 bits on either side.
const MORE: &Rows = &[(0, Some(11)), (1, Some(-6)), (2, Some(8))];

/// The three, each with the bytes a group of its column takes.
const TABLES: [(&Rows, usize); 3] = [(FIT, NARROW), (PAST, WIDE), (MORE, NARROW)];

/// A batch of one column of the values of `rows`: integers where 64 bits
/// hold every one, as a store gives them, else values.
fn batch_of(rows: &Rows) -> Batch {
    let units: Option<Vec<i64>> = rows.iter().map(|&(_, v)| i64::try_from(v.unwrap_or(0)).ok()).collect();
    let nulls: Vec<bool> = rows.iter().map(|row| row.1.is_none()).collect();
    let column = match units {
        Some(units) => Column::Ints(Ints::new(units, 0, nulls.contains(&true).then(|| Mask::from_bools(&nulls)))),
        None => Column::Values(rows.iter().map(|&(_, v)| v.map_or(Value::Null, |whole| Value::Exact(Decimal::new(whole, 0)))).collect()),
    };
    Batch { columns: vec![column], rows: rows.len() }
}

/// Feeds `rows` to `states` of `agg`.
fn fed(states: &mut States, agg: &AggCall, rows: &Rows, runner: &Runner) {
    let places: Vec<u32> = (0..u32::try_from(rows.len()).unwrap()).collect();
    let pending = Pending { given: places.clone(), read: places, groups: rows.iter().map(|row| row.0).collect() };
    states.feed((&pending, agg), (&batch_of(rows), &Taken::Column(0), &None), (&mut Vec::new(), runner)).unwrap();
    assert!(!matches!(states, States::Boxed(_)), "{} still a column", agg.label);
}

/// The states of `agg` of `groups` groups, fed each of `batches` in turn.
fn states_of(agg: &AggCall, groups: usize, batches: &[&Rows], runner: &Runner) -> States {
    let mut states = States::of(agg);
    states.grow_by(groups);
    for rows in batches {
        fed(&mut states, agg, rows, runner);
    }
    states
}

/// An accumulator a group of `agg`, of `groups` groups, each given its
/// group's values of `batches` in order, as a grouping a row at a time
/// gives them.
fn alone(agg: &AggCall, groups: usize, batches: &[&Rows]) -> Vec<Box<dyn Accumulator>> {
    let mut accs: Vec<_> = (0..groups).map(|_| agg.func.0.accumulator(agg.result.clone())).collect();
    for &(g, value) in batches.iter().copied().flatten() {
        if let Some(units) = value {
            accs[index(g)].update(&Value::Exact(Decimal::new(units, 0))).unwrap();
        }
    }
    accs
}

/// Asserts each group of `states` gives what its accumulator of `accs`
/// gives: its result, or its error, and its written state.
fn same(states: &States, accs: &[Box<dyn Accumulator>], what: &str) {
    for (g, acc) in accs.iter().enumerate() {
        assert_eq!(format!("{:?}", states.finish(g)), format!("{:?}", acc.finish()), "{what}: group {g}");
        let (mut written, mut expected) = (Vec::new(), Vec::new());
        assert_eq!(states.write(g, &mut written), acc.write_state(&mut expected), "{what}: group {g}");
        assert_eq!(written, expected, "{what}: group {g} written");
    }
}

/// The bytes a group takes of `states` kept to their `groups` groups.
fn width(states: &mut States, groups: usize) -> usize {
    states.truncate(groups);
    states.bytes().checked_div(groups).unwrap()
}

/// A runner of a database of the built-in functions, given to `test`.
fn with_runner(test: impl Fn(&Runner)) {
    let db = Arc::new(crate::db::State::with_builtins(Box::new(crate::catalog::MemoryCatalog::default())));
    test(&Runner::new(&db, NO_TRANSACTION));
}

/// Verifies: [DESIGN] (RULES.md D1308, D356) SUM, AVG, MIN and MAX of
/// exact integers kept in columns give their accumulator's result and
/// written state of each group after every batch, of few groups, fed by
/// lanes, and of many, fed in turn, where a group's total leaves 64 bits
/// in the middle of a batch, at the first row of one, below the least
/// integer of 64 bits, and at the first value fed, one 64 bits do not
/// hold. A null among the rows is passed over. A sum's column takes 16
/// bytes a group, its total in 64 bits, until a total leaves them, and
/// 24 after; totals left at the greatest and least integers of 64 bits
/// stay narrow, as an extreme's do unless a value itself does not fit.
#[test]
fn totals_fed_past_64_bits_are_their_accumulators() {
    // Each case: the batches fed, and the bytes a group of a sum's
    // column and of an extreme's take after them.
    let cases: [(&[&Rows], usize, usize); 5] = [
        (&[FIT, &[(1, Some(3)), (0, Some(4)), (0, Some(6)), (0, Some(1)), (1, Some(-9)), (0, Some(2))]], WIDE, NARROW),
        (&[FIT, &[(0, Some(11)), (1, Some(1))]], WIDE, NARROW),
        (&[FIT, &[(0, Some(1)), (1, Some(1)), (1, Some(-7)), (0, Some(2))]], WIDE, NARROW),
        (&[PAST, &[(1, Some(LEAST - 1)), (0, Some(3))]], WIDE, WIDE),
        (&[FIT, &[(0, Some(10)), (1, Some(-5))]], NARROW, NARROW),
    ];
    with_runner(|runner| {
        for (n, name, groups) in (0..cases.len()).flat_map(|n| NAMES.iter().flat_map(move |name| [(n, *name, 3), (n, *name, MANY)])) {
            let (agg, (batches, summed, extreme)) = (call(name, Some(WHOLE)), cases[n]);
            let mut states = states_of(&agg, groups, &[], runner);
            for (upto, rows) in batches.iter().enumerate() {
                fed(&mut states, &agg, rows, runner);
                same(&states, &alone(&agg, groups, &batches[..=upto]), &format!("{name}, case {n}, {groups} groups, batch {upto}"));
            }
            let expected = if name == "SUM" || name == "AVG" { summed } else { extreme };
            assert_eq!(width(&mut states, groups), expected, "{name}, case {n}, {groups} groups: the form");
        }
    });
}

/// Verifies: [DESIGN] (RULES.md D1308, D1110, D1300) one column of states
/// fed nothing takes the states two tables send, one after the other,
/// the second's groups in another order: a table of narrow totals then
/// one of wide, the other way round, and two of narrow totals whose sums
/// leave 64 bits in the merge, on either side. After the first it gives
/// of each group what that table's rows give an accumulator, and after
/// both what the rows of both in that order give, its result and written
/// state, of SUM, AVG, MIN and MAX. What a table sends is charged the
/// width of its form, 16 bytes a group of narrow totals and 24 of wide.
#[test]
fn states_sent_in_either_form_merge_in_turn_as_their_rows() {
    with_runner(|runner| {
        for (name, (first, second)) in NAMES.iter().flat_map(|name| [(0, 1), (1, 0), (0, 2), (2, 0)].map(|pair| (*name, pair))) {
            let (agg, (rows, bytes), (later, later_bytes)) = (call(name, Some(WHOLE)), TABLES[first], TABLES[second]);
            let sent = |of: &Rows| {
                let mut states = states_of(&agg, 3, &[of], runner);
                states.truncate(3);
                states.sent().unwrap()
            };
            let (a, b) = (sent(rows), sent(later));
            assert_eq!((a.bytes(), b.bytes()), (3 * bytes, 3 * later_bytes), "{name}: charged the width of the form");
            let mut states = states_of(&agg, 3, &[], runner);
            states.merge_all(&agg, &[0, 1, 2], (&a, &[0, 1, 2])).unwrap();
            same(&states, &alone(&agg, 3, &[rows]), &format!("{name}: table {first} into states fed nothing"));
            states.merge_all(&agg, &[2, 0, 1], (&b.gathered(&[2, 0, 1]), &[0, 1, 2])).unwrap();
            same(&states, &alone(&agg, 3, &[rows, later]), &format!("{name}: table {first} then table {second}"));
        }
    });
}

/// Every bit of `states` of three groups: each group's total and count
/// and their scale as a spill is written them, and the bytes charged,
/// which tell the form.
fn bits(mut states: States) -> (Vec<u8>, usize) {
    states.truncate(3);
    let Sent::Typed(typed) = states.sent().unwrap() else { panic!("a column of states") };
    let mut out = Vec::new();
    typed.put_groups(&[0, 1, 2], &mut out);
    (out, typed.bytes())
}

/// Verifies: [DESIGN] (RULES.md D1308, D1110) a group merged alone and
/// the same group merged as a list of one leave a column of states the
/// same, bit for bit and in the same form: each group of a table of
/// narrow totals and of one of wide, a group that took no value among
/// them, merged into each group of a column fed nothing, of a narrow
/// column and of a wide one, a merge that leaves 64 bits among them, of
/// SUM, AVG, MIN and MAX.
#[test]
fn a_group_merged_alone_is_merged_as_a_list_of_one() {
    let targets: [&[&Rows]; 4] = [&[], &[FIT], &[PAST], &[MORE]];
    with_runner(|runner| {
        for (name, target, source) in NAMES.iter().flat_map(|name| targets.iter().flat_map(move |t| TABLES.map(|of| (*name, *t, of.0)))) {
            let agg = call(name, Some(WHOLE));
            let sent = states_of(&agg, 3, &[source], runner).sent().unwrap();
            for (g, i) in (0..3_u32).flat_map(|g| (0..3_u32).map(move |i| (g, i))) {
                let (mut one, mut list) = (states_of(&agg, 3, target, runner), states_of(&agg, 3, target, runner));
                one.merge((index(g), &agg), (&sent, index(i))).unwrap();
                list.merge_all(&agg, &[g], (&sent, &[i])).unwrap();
                assert_eq!(bits(one), bits(list), "{name}: group {i} merged into group {g}");
            }
        }
    });
}

/// Verifies: [DESIGN] (RULES.md D1308, D1280) a part's column of exact
/// totals is merged whole by a second grouping whichever form holds its
/// totals. The column that takes the parts starts in the form of the
/// part it is made of. A narrow part and a wide one merged in turn,
/// either way round, and two narrow parts whose sums leave 64 bits, the
/// second's groups each into another group, give of each group what the
/// function's accumulator gives of the rows of both, of SUM, AVG, MIN
/// and MAX.
#[test]
fn parts_of_either_form_merge_whole_as_their_rows() {
    with_runner(|runner| {
        for (name, (first, second)) in NAMES.iter().flat_map(|name| [(0, 1), (1, 0), (0, 2), (2, 0)].map(|pair| (*name, pair))) {
            let (whole, (rows, bytes), (later, _)) = (call(name, Some(WHOLE)), TABLES[first], TABLES[second]);
            let agg = part(&whole);
            let part_of = |of: &Rows| {
                let states = states_of(&agg, 3, &[of], runner);
                assert!(states.is_merged_whole(), "PARTIAL {name} merged whole");
                states.into_part().unwrap()
            };
            let (a, b) = (part_of(rows), part_of(later));
            let mut merging = States::merging(&a);
            merging.grow_by(3);
            assert_eq!(width(&mut merging, 3), bytes, "{name}: the form of the part it is made of");
            merging.merge_part(&a, (&[0, 1, 2], &[0, 1, 2])).unwrap();
            merging.merge_part(&b, (&[1, 2, 0], &[0, 1, 2])).unwrap();
            let moved: Vec<_> = later.iter().map(|&(g, value)| ((g + 1) % 3, value)).collect();
            same(&merging, &alone(&whole, 3, &[rows, moved.as_slice()]), &format!("{name}: part {first} then part {second}"));
        }
    });
}

/// Verifies: [DESIGN] (RULES.md D1308, D1256) a result that raises is
/// raised of a total in either form as its accumulator raises it: a sum
/// of more digits than an exact number has, of a wide total, and a mean
/// whose total at its result's scale has more, of a total 64 bits hold.
/// A column finished for its errors alone raises what finishing its
/// groups in turn raises, of narrow totals each compared with the bound
/// under which none raises, and nothing where no group raises, of a
/// narrow column passed over whole.
#[test]
fn results_that_raise_are_raised_of_totals_in_either_form() {
    let most_digits = 6 * 10_i128.pow(37);
    let raised = |states: &States, g: usize| format!("{:?}", states.finish(g).map(drop));
    with_runner(|runner| {
        let sum = call("SUM", Some(WHOLE));
        let past: &Rows = &[(0, Some(5)), (1, Some(most_digits)), (1, Some(most_digits)), (2, Some(MOST))];
        let mut wide = states_of(&sum, 3, &[past], runner);
        same(&wide, &alone(&sum, 3, &[past]), "a sum of more digits than an exact number has");
        assert_eq!(wide.finish(1).unwrap_err().sqlcode(), -301_i32);
        assert_eq!((format!("{:?}", wide.checked(3)), width(&mut wide, 3)), (raised(&wide, 1), WIDE));
        let mut narrow = states_of(&sum, 3, &[FIT, &[(0, Some(10)), (2, Some(LEAST))]], runner);
        assert!(narrow.checked(3).is_ok() && (0..3).all(|g| narrow.finish(g).is_ok()), "no sum 64 bits hold raises");
        assert_eq!(width(&mut narrow, 3), NARROW);
        let scaled = DataType::Exact { precision: crate::value::MAX_EXACT_PRECISION, scale: 30, spelled: Spelled::Numeric };
        let mean = AggCall { result: scaled, ..call("AVG", Some(WHOLE)) };
        let rows: &Rows = &[(0, Some(5)), (1, Some(10_i128.pow(12))), (2, Some(7))];
        let mut means = states_of(&mean, 3, &[rows], runner);
        same(&means, &alone(&mean, 3, &[rows]), "a mean whose total at its result's scale has more digits");
        assert!(means.finish(1).is_err() && means.finish(0).is_ok() && means.finish(2).is_ok());
        assert_eq!(format!("{:?}", means.checked(3)), raised(&means, 1), "raised of a narrow total compared with the bound");
        assert!(means.checked(1).is_ok(), "the groups before it raise nothing");
        assert_eq!(width(&mut means, 3), NARROW);
    });
}

/// Asserts that each group of the states of `agg` fed `batches`, read at
/// its place of the column a table sends, is that state merged into one
/// fed nothing, its result or its error, a place past the column's end
/// too; and that the column finished at places for its errors alone
/// raises what the merged states finished for theirs raise.
fn lying_as_merged(agg: &AggCall, batches: &[&Rows], runner: &Runner) {
    let sent = states_of(agg, 4, batches, runner).sent().unwrap();
    let Sent::Typed(typed) = &sent else { panic!("{} sent as a column", agg.label) };
    for place in 0..5_u32 {
        let (lying, merged) = (format!("{:?}", typed.finish(index(place))), format!("{:?}", sent.finished_at(agg, place)));
        assert_eq!(lying, merged, "{} at place {place}", agg.label);
    }
    let lists: [&[u32]; 5] = [&[0, 1, 2, 3], &[0, 2, 3], &[3, 1, 0], &[2, 9], &[]];
    for places in lists {
        let groups: Vec<u32> = (0_u32..).take(places.len()).collect();
        let fed_nothing = Sent::checked_of(agg, &[(&sent, places)], &groups);
        assert_eq!(format!("{:?}", typed.checked_at(places)), format!("{fed_nothing:?}"), "{} checked at {places:?}", agg.label);
    }
    assert_eq!(typed.counts().is_some(), agg.label == "COUNT", "COUNT's counts alone are given as they lie");
}

/// Verifies: [DESIGN] (RULES.md D1337, D1308, D1300) a group's result read
/// at its place of the column a table sends is the result of that state
/// merged into one fed nothing, and its error where it raises: of COUNT,
/// and of SUM, AVG, MIN and MAX with narrow totals, with wide ones and
/// with a column widened midway, a group that took no value among them, a
/// sum of more digits than a number has and a mean whose total at its
/// result's scale has more. A column finished at places for its errors
/// alone raises what the merged states raise, of the first place that
/// fails, and nothing of places that raise nothing. An approximate sum is
/// the one column of which this does not hold. It writes no state, so
/// merged it is refused (SQLCODE -916) where read as it lies it gives a
/// value, and a reader of states where they lie must not take one.
#[test]
fn a_state_read_where_it_lies_is_the_state_merged_into_one_fed_nothing() {
    let most_digits = 6 * 10_i128.pow(37);
    let raising: &Rows = &[(0, Some(5)), (1, Some(most_digits)), (1, Some(most_digits)), (2, Some(MOST))];
    let small: &Rows = &[(0, Some(5)), (1, Some(10_i128.pow(12))), (2, Some(7))];
    let columns: [&[&Rows]; 5] = [&[FIT], &[PAST], &[FIT, MORE], &[raising], &[]];
    with_runner(|runner| {
        for (name, batches) in NAMES.iter().chain(&["COUNT"]).flat_map(|name| columns.map(|batches| (*name, batches))) {
            lying_as_merged(&call(name, Some(WHOLE)), batches, runner);
        }
        let scaled = DataType::Exact { precision: crate::value::MAX_EXACT_PRECISION, scale: 30, spelled: Spelled::Numeric };
        lying_as_merged(&AggCall { result: scaled, ..call("AVG", Some(WHOLE)) }, &[small], runner);
        let sum = call("SUM", Some(DOUBLE));
        let mut approx = States::of(&sum);
        approx.grow_by(2);
        approx.feed((&pending(), &sum), (&batch(), &Taken::Column(1), &None), (&mut Vec::new(), runner)).unwrap();
        let sent = approx.sent().unwrap();
        let Sent::Typed(typed) = &sent else { panic!("an approximate sum sent as a column") };
        assert!(typed.finish(0).is_ok_and(|value| !value.is_null()), "read as it lies it gives a value");
        assert_eq!(sent.finished_at(&sum, 0).unwrap_err().sqlcode(), -916_i32, "merged it is refused");
    });
}
