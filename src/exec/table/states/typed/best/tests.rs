//! Unit tests of the column of each group's best string (RULES.md D1290):
//! it gives what an accumulator a group gives of the same strings in the
//! same order, merged, gathered, written and read back; it copies its
//! live strings once the dead pass them; and what is no MAX or MIN of
//! character strings by their text alone has no such column.

use std::sync::Arc;

use super::super::super::{Sent, States};
use super::*;
use crate::functions::{CollatedExtreme, PART_TYPE, Partial, Registry};
use crate::logical::Expr;
use crate::storage::{Coded, Ints, Mask};
use crate::txn::NO_TRANSACTION;
use crate::value::CharUnits;
use crate::value::collation::{CASE_INSENSITIVE, CollationRef, Pad, UCS_BASIC, builtin};
use crate::value::lob::LobKind;

/// A call of the set function `func` of a column of type `arg`.
fn call_of(func: AggregateRef, arg: &DataType) -> AggCall {
    let result = func.0.result_type(Some(arg.clone())).unwrap();
    let label = func.0.name().to_owned();
    AggCall { func, arg: Some(Expr::Column(0)), distinct: false, result, filter: None, distribution: None, label }
}

/// MAX, or MIN, with no collation, of a column of type `arg`.
fn plain(max: bool, arg: &DataType) -> AggCall {
    call_of(Registry::with_set_functions().aggregate(if max { "MAX" } else { "MIN" }).unwrap(), arg)
}

/// MAX, or MIN, of a VARCHAR column by the collation named `name` of the
/// pad characteristic `pad`.
fn collated(max: bool, name: &str, pad: Pad) -> AggCall {
    let collation = builtin().into_iter().find(|c| c.name() == name).unwrap();
    call_of(AggregateRef(Arc::new(CollatedExtreme { max, collation: CollationRef { collation, pad } })), &DataType::varying(8))
}

/// The calls kept in a column: MAX and MIN with no collation, padded, and
/// by UCS_BASIC with NO PAD, by octets.
fn calls() -> Vec<AggCall> {
    let t = DataType::varying(8);
    vec![plain(true, &t), plain(false, &t), collated(true, UCS_BASIC, Pad::None), collated(false, UCS_BASIC, Pad::None)]
}

/// The strings of a batch's rows, a null where none: 'a' and 'a ' are
/// equal padded and apart unpadded, and a tab after 'a' is below 'a'
/// padded and above it unpadded.
const STRINGS: [Option<&str>; 10] =
    [Some("m"), Some("a "), None, Some("a"), Some("a\t"), Some("é"), Some(""), Some("m "), Some("zz"), Some("M")];

/// The groups of the batch's rows. Group 1 has 'a ', 'a' and 'a' before a
/// tab, group 2 the null alone, group 3 no row.
const GROUPS: [u32; 10] = [0, 1, 2, 1, 1, 0, 0, 0, 0, 0];

/// `STRINGS` as a vector of texts one after another, or as the codes of a
/// dictionary whose entries are in another order.
fn texts(dictionary: bool) -> Texts {
    let nulls = Some(Mask::from_bools(&STRINGS.map(|s| s.is_none())));
    let ends_of = |all: &[&str]| (1..=all.len()).map(|n| all[..n].concat().len()).collect();
    if !dictionary {
        let all = STRINGS.map(Option::unwrap_or_default);
        return Texts::new(all.concat(), ends_of(&all), nulls);
    }
    let entries = ["zz", "é", "M", "m ", "", "a\t", "a", "a ", "m"];
    let code = |s: &str| u32::try_from(entries.iter().position(|e| *e == s).unwrap()).unwrap();
    let codes = STRINGS.iter().map(|s| s.map_or(0, code)).collect();
    Texts::coded(Coded { codes, entries: Arc::new(Texts::new(entries.concat(), ends_of(&entries), None)) }, nulls)
}

/// A batch of the one column `column`, of as many rows as `STRINGS`.
fn batch_of(column: Column) -> Batch {
    Batch { columns: vec![column], rows: STRINGS.len() }
}

/// The batch's rows taken, each its group's.
fn pending() -> Pending {
    Pending { given: (0..10).collect(), read: (0..10).collect(), groups: GROUPS.to_vec() }
}

/// An accumulator of `agg` given group `g`'s strings in order, each as a
/// value.
fn alone(agg: &AggCall, g: u32) -> Box<dyn Accumulator> {
    let mut acc = agg.func.0.accumulator(agg.result.clone());
    for (s, _) in STRINGS.iter().zip(GROUPS).filter(|(s, of)| *of == g && s.is_some()) {
        acc.update(&Value::Char(s.unwrap_or_default().into())).unwrap();
    }
    acc
}

/// The states of `agg` of four groups, fed the batch's strings, plain or
/// `coded`, read of the batch's column.
fn fed(agg: &AggCall, coded: bool, runner: &Runner) -> States {
    let mut states = States::of(agg);
    states.grow_by(4);
    let batch = batch_of(Column::Texts(texts(coded)));
    states.feed((&pending(), agg), (&batch, &Taken::Column(0), &None), (&mut Vec::new(), runner)).unwrap();
    states
}

/// A database of the built-in functions.
fn database() -> Arc<crate::db::State> {
    Arc::new(crate::db::State::with_builtins(Box::new(crate::catalog::MemoryCatalog::default())))
}

/// Verifies: [DESIGN] (RULES.md D1290) [X3.135-1986 §5.8 GR4d] MAX and MIN
/// of character strings, padded with no collation and by octets under
/// UCS_BASIC with NO PAD, kept in a column, give of each group what its
/// accumulator gives of the group's strings in order, its result and its
/// written state, of texts one after another and of a dictionary's codes;
/// a group of a null alone and one of no row give a null; and a state an
/// accumulator wrote is merged as the accumulator merges it.
#[test]
fn a_column_of_best_strings_is_an_accumulator_a_group() {
    let db = database();
    let runner = Runner::new(&db, NO_TRANSACTION);
    for (agg, coded) in calls().iter().flat_map(|agg| [(agg, false), (agg, true)]) {
        let mut states = fed(agg, coded, &runner);
        assert!(matches!(states, States::Typed(Typed::Best(_))), "{} kept in a column", agg.label);
        for g in 0..4_u32 {
            let (at, acc) = (usize::try_from(g).unwrap(), alone(agg, g));
            assert_eq!(format!("{:?}", states.finish(at)), format!("{:?}", acc.finish()), "{} of group {g} coded {coded}", agg.label);
            let (mut written, mut expected) = (Vec::new(), Vec::new());
            assert_eq!(states.write(at, &mut written), acc.write_state(&mut expected));
            assert_eq!(written, expected, "{} written", agg.label);
        }
        // Every group is merged with group 1's state and with group 0's.
        for (g, from) in [(0_u32, 1_u32), (1, 0), (2, 1), (3, 0)] {
            let (at, mut merged, mut state) = (usize::try_from(g).unwrap(), alone(agg, g), Vec::new());
            assert!(alone(agg, from).write_state(&mut state));
            merged.merge(&state).unwrap();
            states.merge_written((at, agg), &state).unwrap();
            assert_eq!(format!("{:?}", states.finish(at)), format!("{:?}", merged.finish()), "{} merged into {g}", agg.label);
        }
        assert!(matches!(states, States::Typed(Typed::Best(_))), "{} still a column", agg.label);
    }
}

/// Verifies: [DESIGN] (RULES.md D1290, D1110, D1226) a column of best
/// strings merged with another table's column, a group at a time and all
/// at once, gives what the accumulators merged give; gathered in another
/// order it gives each group's at its new place; and written to a spill
/// and read back it gives what it gave, a group of no string too.
#[test]
fn best_strings_merge_gather_and_come_back_from_a_spill() {
    let db = database();
    let runner = Runner::new(&db, NO_TRANSACTION);
    for agg in calls() {
        let wanted = |g: u32| format!("{:?}", alone(&agg, g).finish());
        // Groups 0 to 3 of the other table go into groups 1, 0, 3 and 3.
        let merged_of = |g: u32, from: &[u32]| {
            let mut acc = alone(&agg, g);
            for &i in from {
                let mut state = Vec::new();
                assert!(alone(&agg, i).write_state(&mut state));
                acc.merge(&state).unwrap();
            }
            format!("{:?}", acc.finish())
        };
        let sent = fed(&agg, true, &runner).sent().unwrap();
        let mut all = fed(&agg, false, &runner);
        all.merge_all(&agg, &[1, 0, 3, 3], (&sent, &[0, 1, 2, 3][..])).unwrap();
        let mut each = fed(&agg, false, &runner);
        for (g, i) in [(1, 0), (0, 1), (3, 2), (3, 3)] {
            each.merge((g, &agg), (&sent, i)).unwrap();
        }
        for states in [&all, &each] {
            assert!(matches!(states, States::Typed(Typed::Best(_))), "{} merged as a column", agg.label);
            let found: Vec<String> = (0..4).map(|g| format!("{:?}", states.finish(g))).collect();
            assert_eq!(found, [merged_of(0, &[1]), merged_of(1, &[0]), wanted(2), merged_of(3, &[2, 3])], "{}", agg.label);
        }
        let gathered = sent.gathered(&[3, 1, 0]);
        let mut out = Vec::new();
        assert!(gathered.put_groups(&[2, 0, 1], &mut out));
        let mut reader = Reader::new(&out, "states");
        let back = States::thawed(Sent::read(&mut reader, 3, &agg).unwrap(), &agg, 3).unwrap();
        reader.finish().unwrap();
        assert!(matches!(back, States::Typed(Typed::Best(_))), "{} read back as a column", agg.label);
        let found: Vec<String> = (0..3).map(|g| format!("{:?}", back.finish(g))).collect();
        assert_eq!(found, [wanted(0), wanted(3), wanted(1)], "{} gathered and read back", agg.label);
    }
}

/// Verifies: [DESIGN] (RULES.md D1290) a column whose groups' strings are
/// bettered again and again copies its live strings to a text of their
/// own once the dead octets pass the live ones and the groups' places, so
/// it never holds more than twice what it needs and a string more; every
/// group keeps its best through each copy, and groups cut off leave
/// their strings dead as bettered ones do.
#[test]
fn a_column_of_best_strings_copies_its_live_strings_once_the_dead_pass_them() {
    let agg = collated(false, UCS_BASIC, Pad::None);
    let Some(Typed::Best(mut best)) = Typed::best(&agg) else { panic!("MIN by octets has a column") };
    best.grow_by(3);
    best.take(0, "kept all along");
    let string = |n: u32| format!("{:05}{}", 99_999 - n, "x".repeat(200));
    let mut copies = 0;
    for n in 0..2_000_u32 {
        let before = best.dead;
        best.take(1, &string(n));
        if n % 7 == 0 {
            best.take(2, &string(n));
        }
        copies += u32::from(best.dead < before);
        let live = best.get(0).map_or(0, str::len) + best.get(1).map_or(0, str::len) + best.get(2).map_or(0, str::len);
        assert_eq!(best.text.len(), live + best.dead, "the text is the live and the dead");
        assert!(best.dead <= live + 3 * size_of::<(usize, usize)>(), "the dead stay within the live and the places after {n}");
    }
    // Three strings of 205 octets die before they pass the 424 live and the 48 of three places.
    assert!((600..1_000).contains(&copies), "copied {copies} times");
    assert_eq!(best.get(0), Some("kept all along"));
    assert_eq!(best.get(1), Some(string(1_999).as_str()));
    assert_eq!(best.get(2), Some(string(1_995).as_str()));
    best.grow_by(40);
    (3..43).for_each(|g| best.take(g, &"y".repeat(100)));
    best.truncate(2);
    assert_eq!((best.len(), best.dead), (2, 0), "what was cut off died and was copied away");
    assert_eq!((best.get(0), best.get(1)), (Some("kept all along"), Some(string(1_999).as_str())));
    assert_eq!(best.text.len(), 14 + 205);
}

/// Verifies: [DESIGN] (RULES.md D1290) a column of best strings fed a
/// vector of values that are strings and nulls takes them as it takes
/// texts, and so an expression's vector by the rows' places among those
/// given; fed a vector holding anything else, or a vector of integers, it
/// turns into accumulators, each keeping its group's best, and is fed as
/// they are.
#[test]
fn a_column_fed_other_than_strings_turns_into_accumulators() {
    let db = database();
    let runner = Runner::new(&db, NO_TRANSACTION);
    let values = |extra: Value| -> Vec<Value> {
        let mut all: Vec<Value> = STRINGS.iter().map(|s| s.map_or(Value::Null, |s| Value::Char(s.into()))).collect();
        all[2] = extra;
        all
    };
    for agg in calls() {
        let wanted: Vec<String> = (0..4).map(|g| format!("{:?}", alone(&agg, g).finish())).collect();
        let mut states = States::of(&agg);
        states.grow_by(4);
        let (batch, mut row) = (batch_of(Column::Values(values(Value::Null))), Vec::new());
        states.feed((&pending(), &agg), (&batch, &Taken::Column(0), &None), (&mut row, &runner)).unwrap();
        assert!(matches!(states, States::Typed(Typed::Best(_))), "{} of values stays a column", agg.label);
        assert_eq!((0..4).map(|g| format!("{:?}", states.finish(g))).collect::<Vec<_>>(), wanted, "{}", agg.label);
        // The same strings again add nothing, whatever they are fed as.
        let mixed = batch_of(Column::Values(values(Value::Bool(true))));
        let mut turned = fed(&agg, true, &runner);
        turned.feed((&pending(), &agg), (&mixed, &Taken::Column(0), &None), (&mut row, &runner)).unwrap();
        assert!(matches!(turned, States::Boxed(_)), "{} of a truth value turns into accumulators", agg.label);
        for g in [0, 1, 3] {
            assert_eq!(format!("{:?}", turned.finish(g)), wanted[g], "{} group {g} kept its best", agg.label);
        }
        let ints = batch_of(Column::Ints(Ints::new(vec![0; 10], 0, None)));
        let mut of_ints = fed(&agg, false, &runner);
        of_ints.feed((&pending(), &agg), (&ints, &Taken::Column(0), &None), (&mut row, &runner)).unwrap();
        assert!(matches!(of_ints, States::Boxed(_)), "{} of integers turns into accumulators", agg.label);
    }
}

/// Verifies: [DESIGN] (RULES.md D1290) what has no column of best
/// strings, and keeps an accumulator a group: MAX and MIN under a
/// collation that makes sort keys, one that folds case and UCS_BASIC with
/// PAD SPACE; of a character large object; of a binary string; and a part
/// of MAX or MIN of character strings, which the first of two groupings
/// computes. MAX and MIN of exact numbers keep their own column.
#[test]
fn what_is_no_extreme_of_strings_by_their_text_has_no_column() {
    let lob = DataType::Lob { kind: LobKind::Character, length: 1 << 20_u32, units: CharUnits::Characters };
    let part = |agg: AggCall| {
        let func = AggregateRef(Arc::new(Partial { func: agg.func.clone(), result: agg.result.clone() }));
        AggCall { func, result: PART_TYPE, ..agg }
    };
    let boxed = [
        collated(false, CASE_INSENSITIVE, Pad::Space),
        collated(true, CASE_INSENSITIVE, Pad::None),
        collated(false, UCS_BASIC, Pad::Space),
        plain(false, &lob),
        plain(true, &DataType::VarBinary { length: 8 }),
        part(plain(false, &DataType::varying(8))),
        part(collated(true, UCS_BASIC, Pad::None)),
    ];
    for agg in boxed {
        assert!(Typed::best(&agg).is_none(), "{} of {:?} has no column of best strings", agg.label, agg.result);
        assert!(matches!(States::of(&agg), States::Boxed(_)), "{} of {:?} keeps accumulators", agg.label, agg.result);
    }
    let exact = DataType::Exact { precision: 10, scale: 0, spelled: crate::value::Spelled::Integer };
    assert!(matches!(States::of(&plain(false, &exact)), States::Typed(Typed::Exact(_))));
}
