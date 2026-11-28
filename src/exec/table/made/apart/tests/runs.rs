//! Unit tests of a morsel's runs of one key handed on an entry each
//! (RULES.md D1325): what is handed over is what the runs appended in
//! the order they come, each fed its rows, and then gathered by
//! partition give, of a batch that tells no run among batches that do,
//! of a batch a filter kept some rows of, of nulls in the keys and in
//! what the set functions take, a run of nulls alone among them, and of
//! a run whose sum leaves 64 bits; which batches are held an entry a
//! run; and where each entry's rows and its first row's number lie.

use std::sync::Arc;

use super::super::runs::Runs;
use super::*;
use crate::exec::Runner;
use crate::exec::batch::lanes;
use crate::storage::sel::kept_count;
use crate::txn::NO_TRANSACTION;

/// What COUNT(\*), SUM, MIN and MAX each take: rows and the third
/// column, none of it null; rows and the fourth, nulls in it; and the
/// fourth's values counted, with sums of the third.
const SUMS: [Taken; 4] = [Taken::Count, Taken::Column(2), Taken::Column(2), Taken::Column(2)];
const NULLS: [Taken; 4] = [Taken::Count, Taken::Column(3), Taken::Column(3), Taken::Column(3)];
const COUNTED: [Taken; 4] = [Taken::Column(3), Taken::Column(2), Taken::Column(3), Taken::Column(2)];

/// A morsel's batches, three of 400 rows. The first tells no run, its
/// integer key a row's own. The second, kept every row but each fifth,
/// holds each key three rows running, and the third four, a key coming
/// back in a later run; every eleventh run of the second is of null
/// keys. Beside the integer a text, with a trailing space in every
/// other stretch of eight rows. The third column holds the integers a
/// set function takes, unlike within a run, none null; the fourth
/// integers null in every seventh row and in every fifth stretch of
/// three rows, so whole runs of the second batch are of nulls alone and
/// runs of the third hold a null among values.
fn morsel() -> Vec<(Batch, Option<Sel>)> {
    let batch = |b: i64| {
        let n = |r: i64| b.saturating_mul(400).saturating_add(r);
        let key = |r: i64| [n(r), n(r).div_euclid(3).rem_euclid(40), n(r).div_euclid(4).rem_euclid(40)][usize::try_from(b).unwrap()];
        let null: Vec<bool> = (0..400).map(|r| b == 1 && n(r).div_euclid(3).rem_euclid(11) == 3).collect();
        let text = |r: i64| format!("t{}{}", n(r).div_euclid(8).rem_euclid(3), if n(r).div_euclid(8) % 2 == 0 { " " } else { "" });
        let none: Vec<bool> = (0..400).map(|r| n(r).rem_euclid(7) == 0 || n(r).div_euclid(3).rem_euclid(5) == 1).collect();
        let columns = vec![
            Column::Ints(Ints::new((0..400).map(key).collect(), 0, Some(Mask::from_bools(&null)))),
            Column::Values((0..400).map(|r| Value::Char(text(r).into())).collect()),
            Column::Ints(Ints::new((0..400).map(|r| n(r).rem_euclid(97)).collect(), 0, None)),
            Column::Ints(Ints::new((0..400).map(|r| n(r).rem_euclid(89)).collect(), 0, Some(Mask::from_bools(&none)))),
        ];
        let kept = (b == 1).then(|| (0..400_u32).filter(|r| r % 5 != 0).collect::<Sel>());
        (Batch { columns, rows: 400 }, kept)
    };
    (0..3).map(batch).collect()
}

/// Where each run of rows alike in both key columns ends among the rows
/// `rows` kept of `batch`, or all, the last their count.
fn ends_of(batch: &Batch, rows: Option<&[u32]>) -> Option<Vec<usize>> {
    let key = |n: usize| AT.map(|c| format!("{:?}", batch.cell(c, nth(rows, n)).to_value()));
    let count = kept_count(rows, batch.rows);
    let mut ends: Vec<usize> = (1..count).filter(|&n| key(n) != key(n.saturating_sub(1))).collect();
    ends.push(count);
    Some(ends)
}

/// A runner of a database of nothing, which hears the warnings.
fn runner() -> Runner {
    Runner::new(&Arc::new(crate::db::State::with_builtins(Box::new(crate::catalog::MemoryCatalog::default()))), NO_TRANSACTION)
}

/// The runs of `kept` appended in the order they come, each an entry
/// of its first row's key and hash, numbered by that row's place among
/// all the rows kept from 1000, and fed every row of the run as a
/// table's group is fed: what a morsel hands on an entry a run.
fn appended_runs(kept: &[(Batch, Option<Sel>)], taken: &[Taken], seed: &Seed) -> GroupTable {
    let mut table = GroupTable::unplaced(AT.len(), &calls(), seed, 0);
    let (runner, mut one, mut number) = (runner(), Vec::new(), 1000_u64);
    for (batch, rows) in kept {
        let rows = rows.as_ref().map(Sel::as_slice);
        hash_rows(batch, (AT.as_slice(), rows), (seed, &Portable), &mut one);
        let mut from = 0_usize;
        for end in ends_of(batch, rows).unwrap() {
            let first = number.saturating_add(u64::try_from(from).unwrap());
            let own = table.appended(one[from], Key::Row(batch, &AT, nth(rows, from)), first);
            (from..end).for_each(|n| table.take(own, (n, nth(rows, n))));
            from = end;
        }
        number = number.saturating_add(u64::try_from(from).unwrap());
        table.feed(batch, (taken, &lanes(taken, batch, rows)), (&mut Vec::new(), &runner)).unwrap();
    }
    table
}

/// The batches `kept` held an entry a run and written by partition
/// among `parts` by `lanes`, each entry given its run's states as
/// `taken` says, its first row numbered from 1000; and their runs.
fn written_runs(kept: Vec<(Batch, Option<Sel>)>, taken: &[Taken], (parts, by_lanes): (usize, &[usize]), seed: &Seed) -> (GroupTable, Runs) {
    let (held, runs) = Runs::held(kept, ends_of);
    let (hashes, by) = hashed(&held, by_lanes, seed);
    let by = (!by_lanes.is_empty()).then_some(by.as_slice());
    let (of, how) = ((held.as_slice(), AT.as_slice()), (parts, by_lanes));
    let mut table = GroupTable::placed(of, (calls().as_slice(), seed), (hashes.as_slice(), by, runs.numbers(&held, 1000)), how);
    {
        let vectors: Vec<_> =
            held.iter().enumerate().map(|(b, (batch, heads))| lanes(taken, batch, runs.rows_of(b, heads.as_ref()))).collect();
        table.feed_placed((&held, &runs), (taken, &vectors), (&mut Vec::new(), &runner())).unwrap();
    }
    table.taken = runs.rows();
    (table, runs)
}

/// Verifies: [DESIGN] RULES.md D1325, D1313 [X3.135-1986 §5.8 GR1] a
/// morsel's runs handed on an entry each hand over what the runs
/// appended in the order they come, each fed its rows, and then gathered
/// by partition hand over: the same hashes, first rows' numbers, places,
/// starts and keys, and each partition's table merges them into the same
/// groups of the same states. An entry's first row is numbered by its
/// place among all the rows kept of the morsel, of a batch after one
/// that told no run and after one a filter kept some rows of. Its COUNT
/// is its run's length, its SUM the sum of its run's unlike values and
/// not its first row's, its MIN and MAX the run's; of nulls nothing is
/// taken, so a run of nulls alone leaves the state of no value, as the
/// group fed those rows holds; and the count of a column, which is not
/// set, is fed every row of each run. By whole keys and by a lane.
#[test]
fn runs_handed_on_are_the_runs_appended_and_fed() {
    let seed = Seed::fixed(0x5EED);
    for taken in [SUMS, NULLS, COUNTED] {
        for by_lanes in BY {
            let apart = appended_runs(&morsel(), &taken, &seed);
            let (placed, runs) = written_runs(morsel(), &taken, (8, by_lanes), &seed);
            assert!(runs.any() && placed.placed.is_some(), "runs told, and written by partition");
            assert_eq!((placed.taken, placed.len()), (1120, apart.len()), "every row taken, an entry a run");
            assert!(placed.len() > 500 && placed.len() < 800, "400 rows of the first batch and the runs of 720 more");
            let (gathered, lying) = (apart.made(8, by_lanes).unwrap(), placed.made(8, by_lanes).unwrap());
            assert_eq!(told(&lying, 8, &seed), told(&gathered, 8, &seed), "taken {taken:?}, lanes {by_lanes:?}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1325 [ISO/IEC 9075:1992 §6.5 GR1b] a
/// column of states set as its runs' says whether a null was passed
/// over, of a null within a run and of a run of nulls alone, and says
/// none was where no value is null; COUNT(\*) is set of rows alone, and
/// a column that holds states already, the count of a column and a
/// column that holds no integers are left as they are to be fed. The
/// entry of a run of nulls alone has no sum, least or greatest.
#[test]
fn states_set_as_their_runs_say_whether_a_null_was_passed_over() {
    let seed = Seed::fixed(0x5EED);
    let (held, runs) = Runs::held(morsel(), ends_of);
    let (hashes, _) = hashed(&held, WHOLE, &seed);
    let numbers = runs.numbers(&held, 0);
    let mut table =
        GroupTable::placed((held.as_slice(), AT.as_slice()), (calls().as_slice(), &seed), (hashes.as_slice(), None, numbers), (8, WHOLE));
    let places = table.placed.as_ref().unwrap().places.clone();
    let of = (held.as_slice(), &runs);
    let [count, sum, least, most] = table.states.as_mut_slice() else { panic!("four set functions") };
    assert_eq!(count.set_runs(&Taken::Column(3), of, &places), None, "COUNT of a column is fed");
    assert_eq!(count.set_runs(&Taken::Count, of, &places), Some(false), "rows counted, none a null");
    assert_eq!(count.set_runs(&Taken::Count, of, &places), None, "states held already");
    assert_eq!(least.set_runs(&Taken::Column(1), of, &places), None, "no integers");
    assert_eq!(least.set_runs(&Taken::Count, of, &places), None, "an extreme takes a column");
    assert_eq!(sum.set_runs(&Taken::Column(2), of, &places), Some(false), "no null among the values");
    assert_eq!(most.set_runs(&Taken::Column(3), of, &places), Some(true), "nulls passed over");
    assert_eq!(most.set_runs(&Taken::Column(3), of, &places), None, "states held already");
    // The second batch keeps rows 1, then 2 to 4, then 6 and 7, then 8
    // and 9, a run each. The fourth column is null in rows 8 and 9, so
    // that run's entry, the fourth after the first batch's 400, takes no
    // value; and null in row 6 alone of the run before it.
    let (second, kept) = (&held[1].0, runs.rows_of(1, held[1].1.as_ref()).unwrap());
    assert_eq!(&kept[..8], [1_u32, 2, 3, 4, 6, 7, 8, 9]);
    assert_eq!([6, 7, 8, 9].map(|r| second.cell(3, r).is_null()), [true, false, true, true]);
    let shown =
        |entry: usize| format!("{:?} {:?}", count.finish(index(places[entry])).unwrap(), most.finish(index(places[entry])).unwrap());
    assert_eq!(shown(403), format!("{:?} {:?}", exact(2), Value::Null), "a run of nulls alone: two rows, no greatest");
    assert_eq!(shown(402), format!("{:?} {:?}", exact(2), second.cell(3, 7).to_value()), "a null within a run passed over");
}

/// Verifies: [DESIGN] RULES.md D1325, D1308 a sum that leaves 64 bits
/// within one run is not set: nothing is made of the column, which is
/// then fed every row of each run and sums as wide as a sum goes, while
/// the least and the greatest of the same values are set. What is handed
/// over merges into the groups the runs appended and fed give.
#[test]
fn a_run_whose_sum_leaves_64_bits_is_fed() {
    let seed = Seed::fixed(0x5EED);
    let wide = || {
        let mut kept = morsel();
        for (batch, _) in &mut kept {
            batch.columns[2] = Column::Ints(Ints::new((0..400).map(|r| NEAR.saturating_add(r)).collect(), 0, None));
        }
        kept
    };
    let (held, runs) = Runs::held(wide(), ends_of);
    let mut sum = crate::exec::table::states::States::of(&calls()[1]);
    assert_eq!(sum.set_runs(&Taken::Column(2), (held.as_slice(), &runs), &[0; 1120]), None, "no sum set");
    assert_eq!(sum.bytes(), 0, "and nothing made of the column");
    let (apart, (placed, _)) = (appended_runs(&wide(), &SUMS, &seed), written_runs(wide(), &SUMS, (8, WHOLE), &seed));
    let (gathered, lying) = (apart.made(8, WHOLE).unwrap(), placed.made(8, WHOLE).unwrap());
    assert_eq!(merged(&[&lying], 8, &seed), merged(&[&gathered], 8, &seed));
    let digits = |row: &String| row.split(|c: char| !c.is_ascii_digit()).any(|number| number.len() > 19);
    assert!(merged(&[&lying], 8, &seed).iter().any(digits), "a run's sum is past the 19 digits 64 bits hold");
}

/// Verifies: [DESIGN] RULES.md D1325 a batch is held an entry a run
/// only where it is told runs fewer than its rows that end with its
/// last row. It is then held with each run's first row as the rows kept
/// of it, a filter's rows among them, and every row kept of it is still
/// given for the states. A batch told no run, a run a row, or runs that
/// stop short of its rows is held as it came, an entry a row.
///
/// Decision: runs fewer than the rows, and ending with the last row.
/// Both: held by runs. As many runs as rows: as it came (the count alone
/// differs). Fewer runs that end before the last row: as it came (the
/// end alone differs).
#[test]
fn a_batch_is_held_by_its_runs_only_where_they_cover_its_rows() {
    let batch = || Batch { columns: vec![Column::Ints(Ints::new((0..6).collect(), 0, None))], rows: 6 };
    let kept = || Some([0_u32, 2, 3, 5].into_iter().collect::<Sel>());
    let held_of = |ends: Option<Vec<usize>>| Runs::held(vec![(batch(), kept())], |_, _| ends.clone());
    let (held, runs) = held_of(Some(vec![1, 4]));
    assert!(runs.any() && runs.rows() == 4);
    assert_eq!(held[0].1.as_ref().unwrap().as_slice(), [0, 2], "the first rows of the runs, as the batch holds them");
    assert_eq!(runs.rows_of(0, held[0].1.as_ref()), Some(&[0_u32, 2, 3, 5][..]), "every row kept, for the states");
    assert_eq!(runs.spans(0, 2).collect::<Vec<_>>(), [(0, 1), (1, 4)]);
    for ends in [None, Some(vec![1, 2, 3, 4]), Some(vec![1, 3])] {
        let (held, runs) = held_of(ends.clone());
        assert!(!runs.any() && runs.rows() == 4, "{ends:?}");
        assert_eq!(held[0].1, kept(), "{ends:?}: held as it came");
        assert_eq!(runs.rows_of(0, held[0].1.as_ref()), Some(&[0_u32, 2, 3, 5][..]));
        assert_eq!(runs.spans(0, 4).collect::<Vec<_>>(), [(0, 1), (1, 2), (2, 3), (3, 4)], "an entry a row");
    }
    let (whole, runs) = Runs::held(vec![(batch(), None)], |_, _| Some(vec![2, 6]));
    assert_eq!((whole[0].1.as_ref().unwrap().as_slice(), runs.rows_of(0, whole[0].1.as_ref())), (&[0_u32, 2][..], None), "of every row");
}

/// Verifies: [DESIGN] RULES.md D1325 an entry's first row is numbered
/// by its place among all the rows kept of the morsel, the rows of the
/// batches before it and of the runs before it in its batch counted, of
/// a batch after one that told no run and of one after a batch a filter
/// kept some rows of; and the rows fed of a batch held by its runs are
/// all the rows kept of it, each to the entry of its run, what was
/// noted of the batch before forgotten.
#[test]
fn an_entry_is_numbered_and_fed_among_all_the_morsels_rows() {
    let (held, runs) = Runs::held(morsel(), ends_of);
    let numbers: Vec<u64> = runs.numbers(&held, 1000).collect();
    let entries = spans(&held);
    assert_eq!((entries[0], entries[1].0), ((0, 400), 400), "the first batch an entry a row");
    assert_eq!(&numbers[..3], [1000, 1001, 1002]);
    // The second batch keeps rows 1, 2, 3, 4, 6, 7, 8, 9 and so on: its
    // runs are row 1, rows 2 to 4, rows 6 and 7, rows 8 and 9.
    assert_eq!(&numbers[400..404], [1400, 1401, 1404, 1406], "after the 400 rows before, by the rows of the runs before");
    assert_eq!(numbers[entries[2].0], 1720, "the third batch's first entry after 400 and 320 rows");
    assert_eq!(&numbers[entries[2].0..][..3], [1720, 1724, 1728], "runs of four");
    assert_eq!(numbers.len(), entries[2].1);
    let mut pending = Pending::default();
    let places: Vec<u32> = (0..u32::try_from(numbers.len()).unwrap()).map(|e| e.saturating_mul(10)).collect();
    assert!(!runs.take(0, &mut pending, &places[..400]), "the first batch is taken a row an entry");
    assert!(pending.groups.is_empty(), "and nothing is noted of it");
    assert!(runs.take(1, &mut pending, &places[400..entries[1].1]));
    assert_eq!(&pending.groups[..8], [4000, 4010, 4010, 4010, 4020, 4020, 4030, 4030], "each row to its run's entry");
    assert_eq!((&pending.read[..5], &pending.given[..5]), (&[1_u32, 2, 3, 4, 6][..], &[0_u32, 1, 2, 3, 4][..]));
    assert_eq!((pending.groups.len(), pending.read.len(), pending.given.len()), (320, 320, 320));
    assert!(runs.take(2, &mut pending, &places[entries[2].0..]));
    assert_eq!((pending.groups.len(), &pending.read[..5]), (400, &[0_u32, 1, 2, 3, 4][..]), "the batch before forgotten");
    let third = places[entries[2].0];
    assert_eq!(&pending.groups[..5], [third, third, third, third, third.saturating_add(10)], "runs of four");
}
