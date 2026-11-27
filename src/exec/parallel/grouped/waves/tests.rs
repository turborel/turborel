//! Unit tests of a grouping's waves: its groups are the same whichever of
//! its morsels are handed whole to the statement's thread (RULES.md
//! D1247).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::super::super::pipeline::Pipeline;
use super::super::super::probe::PARTS;
use super::super::super::{Held, Limit};
use super::super::top::Top;
use super::super::{Ordered, grouping_of, read_of};
use super::wide::Widen;
use super::{Forced, How, few, waved_as};
use crate::db::{Database, Session};
use crate::exec::Db;
use crate::physical::{PhysicalPlanner, Plan};
use crate::storage::Row;
use crate::txn::NO_TRANSACTION;
use crate::value::{Decimal, Value};

/// Rows of T.
const ROWS: i64 = 24_000;

/// T, kept by column, of `ROWS` rows: K, V, scattered among them, and P,
/// one letter followed by none, one or two spaces, so every P is equal
/// to every other as strings compare padded and three are told apart by
/// their octets.
fn table() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL, V INTEGER NOT NULL, P VARCHAR(4) NOT NULL) USING columnstore",
    )
    .unwrap();
    let a = db.session("A");
    for first in (0..ROWS).step_by(1000) {
        let v = |k: i64| k.wrapping_mul(7).rem_euclid(1013);
        let p = |k: i64| ["x", "x ", "x  "][usize::try_from(k.wrapping_mul(5).rem_euclid(3)).unwrap()];
        let values: Vec<String> = (first..first.saturating_add(1000)).map(|k| format!("({k}, {}, '{}')", v(k), p(k))).collect();
        a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    set(&db, "EXECUTOR.MORSEL_ROWS", 1000);
    set(&db, "EXECUTOR.WORKERS", 3);
    (db, a)
}

/// Sets `name` of `db` to `value`.
fn set(db: &Database, name: &str, value: i128) {
    db.set_setting(name, &Value::Exact(Decimal::new(value, 0))).unwrap();
}

/// What a grouping's waves made: each group's row as its values are
/// written out, in the order they are given; how many runs of groups they
/// ended with, one of a grouping merged on the statement's thread alone,
/// else one a partition; and the waves the partitions merged and the
/// times their tables were written to the spill.
#[derive(Debug, PartialEq, Eq)]
struct Waved {
    rows: Vec<String>,
    runs: usize,
    waves: usize,
    spills: usize,
}

/// What the waves of `sql`, a grouping of T under a projection, made on
/// `degree` threads, run as `how` says.
fn waved(a: &Session, sql: &str, how: &How, degree: usize) -> Waved {
    let (st, optimized) = a.optimized(sql).unwrap();
    let planned = PhysicalPlanner::new(&st).plan(&optimized);
    let aggregate = if let Plan::Project { input, .. } = planned { *input } else { planned };
    let db = Db::of(&st);
    let (mut grouping, read) = grouping_of(&aggregate, (None, None), st.functions.seed().clone()).unwrap();
    let limit = Limit::of(&db);
    let Held::Kept(pipeline) = Pipeline::told(read, (&db, NO_TRANSACTION), &limit, &[]).unwrap() else { panic!("the pipeline gave way") };
    assert!(pipeline.count() > 20, "many morsels: {}", pipeline.count());
    grouping.read = pipeline.batched.as_ref().and_then(|by| read_of(&grouping, by));
    let unpadded = grouping.read.as_ref().map(|by_batch| by_batch.unpadded.clone());
    grouping.seed = grouping.seed.unpadded(&unpadded.expect("grouped by batch"));
    let Held::Kept((runs, given)) = waved_as(pipeline, &Arc::new(grouping), (&db, NO_TRANSACTION, degree), &limit, how).unwrap() else {
        panic!("the grouping gave way")
    };
    let count = runs.len();
    let rows = Ordered::of(runs, given).unwrap().collect::<Result<Vec<Row>, _>>().unwrap();
    let (waves, spills) = (how.counted.0.load(Ordering::Relaxed), how.counted.1.load(Ordering::Relaxed));
    Waved { rows: rows.iter().map(|row| format!("{row:?}")).collect(), runs: count, waves, spills }
}

/// The groups of `sql` and the runs its waves ended with, on three
/// threads, the morsels `forced` names handed whole, or those the
/// executor hands so.
fn groups_of(a: &Session, sql: &str, forced: Forced) -> (Vec<String>, usize) {
    let made = waved(a, sql, &How { forced, ..How::default() }, 3);
    (made.rows, made.runs)
}

/// Every morsel handed whole.
const EVERY: Forced = Some(|_| true);
/// No morsel handed whole.
const NONE: Forced = Some(|_| false);

/// Verifies: [DESIGN] RULES.md D1247, D356 a grouping gives the same
/// groups, in the same order, each set function the same value, whichever
/// of its morsels are handed whole to the statement's thread: as the
/// executor hands them, every one, none, every other, the first half
/// alone and the second half alone, of a grouping of few groups and of
/// two of more groups than a morsel has rows, whose table the statement's
/// thread takes to the partitions each time it passes that.
#[test]
fn the_groups_are_the_same_whichever_morsels_are_handed_whole() {
    let (_db, a) = table();
    let handed: [(&str, Forced); 6] = [
        ("as the executor hands them", None),
        ("every morsel", EVERY),
        ("no morsel", NONE),
        ("every other", Some(|m| m.is_multiple_of(2))),
        ("the first half", Some(|m| m < 12)),
        ("the second half", Some(|m| m >= 12)),
    ];
    for (keyed, groups) in [("MOD(K, 40)", 40_usize), ("K - MOD(K, 10)", 2400), ("MOD(K, 6000)", 6000)] {
        let sql = format!("SELECT G, COUNT(*), SUM(V), MIN(V), MAX(K) FROM (SELECT {keyed} AS G, K, V FROM T) AS Q GROUP BY G");
        let (expected, _) = groups_of(&a, &sql, NONE);
        assert_eq!(expected.len(), groups, "{sql}");
        for (which, forced) in handed {
            assert_eq!(groups_of(&a, &sql, forced).0, expected, "{which}: {sql}");
        }
    }
}

/// A grant that holds every grouping of T, and one its groups pass.
const HELD: i128 = 67_108_864;
/// A grant the groups of T pass, so its partitions go to the spill.
const PASSED: i128 = 131_072;

/// Where the rows of `made` first differ from those of `expected`, told
/// shortly: how many each holds, and the first row of `made` that is not
/// the row `expected` holds there. None if they are the same rows.
fn differing(made: &Waved, expected: &Waved) -> Option<(usize, usize, Option<String>)> {
    let first = made.rows.iter().zip(&expected.rows).find(|(row, same)| row != same).map(|(row, _)| row.clone());
    (first.is_some() || made.rows.len() != expected.rows.len()).then_some((made.rows.len(), expected.rows.len(), first))
}

/// Verifies: [DESIGN] RULES.md D1276, D1247, D1226 a grouping whose morsels
/// are all handed whole to the statement's thread, and whose table there
/// outgrows the grant and goes to the spill as it is, gives every group,
/// in the order and with the values a grant that holds them gives. A
/// regression test: with no wave merged the spill was forgotten at the
/// end, and the groups of the statement thread's last table alone were
/// given.
#[test]
fn a_table_of_the_statements_thread_past_the_grant_loses_no_group() {
    let (db, a) = table();
    let sql = "SELECT G, COUNT(*), SUM(V), MIN(V), MAX(K), MIN(P) FROM (SELECT MOD(K, 6000) AS G, K, V, P FROM T) AS Q GROUP BY G";
    let whole = |grant: i128| {
        set(&db, "MEMORY.WORK_BYTES", grant);
        waved(&a, sql, &How { forced: EVERY, ..How::default() }, 3)
    };
    let (held, spilled) = (whole(HELD), whole(PASSED));
    assert_eq!((held.rows.len(), held.spills), (6000, 0));
    assert!(spilled.spills > 0, "a grant its table passes");
    assert_eq!(differing(&spilled, &held), None);
}

/// Verifies: [DESIGN] RULES.md D1247 morsels are handed whole only while
/// the statement thread's table holds no more groups than a morsel has
/// rows, a thousand here. Of a grouping of 40 groups every morsel handed
/// whole is merged there alone, its waves ending with one run of groups.
/// Of one of 2,400 groups that table passes a thousand groups and is taken
/// to the partitions, so its waves end with a run a partition, as they do
/// of any grouping no morsel of which is handed whole. And a morsel of few
/// groups beside its rows is handed whole no longer once morsels are not,
/// as one of many groups never was.
///
/// Decision: the table holds more groups than a morsel has rows, or not;
/// the morsel's groups are few beside its rows, or not; morsels are still
/// handed whole, or not.
#[test]
fn a_grouping_of_many_groups_stops_handing_morsels_whole() {
    let (_db, a) = table();
    let runs = |keyed: &str, forced: Forced| {
        let sql = format!("SELECT G, COUNT(*) FROM (SELECT {keyed} AS G FROM T) AS Q GROUP BY G");
        groups_of(&a, &sql, forced).1
    };
    assert_eq!(runs("MOD(K, 40)", EVERY), 1, "few groups in all stay the statement thread's");
    assert_eq!(runs("K - MOD(K, 10)", EVERY), PARTS, "many groups in all go to the partitions");
    assert_eq!(runs("MOD(K, 40)", NONE), PARTS, "as do those of morsels not handed whole");
    assert!(few(true, (100, 1000), 3), "few beside its rows, morsels still handed whole");
    assert!(!few(false, (100, 1000), 3), "handed whole no longer");
    assert!(!few(true, (200, 1000), 3), "many beside its rows");
    assert!(!few(false, (200, 1000), 3));
}

/// The waves of `sql` on `degree` threads, its run leading the
/// statement's thread by `lead` morsels a thread.
fn led(a: &Session, sql: &str, lead: usize, degree: usize) -> Waved {
    waved(a, sql, &How { lead, ..How::default() }, degree)
}

/// Verifies: [DESIGN] RULES.md D1262, D356 a grouping whose threads lead
/// the statement's thread by many morsels gives what one that leads by
/// two does, and what one thread alone gives: its groups, their order,
/// and a state that depends on the order its values come in, MIN of
/// strings equal as they compare padded and different in their octets,
/// which is the string of the group's first row only if each partition
/// merged its morsels in their order.
#[test]
fn a_wide_lead_keeps_the_order_states_are_merged_in() {
    let (_db, a) = table();
    for keyed in ["MOD(K, 40)", "MOD(K, 6000)"] {
        let sql = format!("SELECT G, MIN(P) FROM (SELECT {keyed} AS G, P FROM T) AS Q GROUP BY G");
        let alone = led(&a, &sql, 2, 1);
        let kept: std::collections::BTreeSet<&str> =
            alone.rows.iter().filter_map(|row| row.split_once(", ")).map(|(_, least)| least).collect();
        assert!(kept.len() > 1, "strings of different octets are kept, by the order they came in: {kept:?}");
        for lead in [2, 16, 64] {
            assert_eq!(led(&a, &sql, lead, 3).rows, alone.rows, "a lead of {lead}: {sql}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1262, D1155, D38 what a grouping's threads
/// hold ahead of the statement's thread decides nothing of its grant. Of
/// a grant that holds its groups only in waves, and of one its groups
/// pass, a run leading by many morsels merges as many waves and writes
/// its partitions to the spill as many times as one leading by two, and
/// gives the same groups: waves of the first and none spilled, some
/// spilled of the second.
///
/// Decision: the grant holds the groups, or not; the lead is two morsels
/// a thread, or many.
#[test]
fn a_wide_lead_merges_and_spills_as_a_narrow_one() {
    let (db, a) = table();
    let sql = "SELECT G, COUNT(*), SUM(V), MIN(V), MAX(K) FROM (SELECT MOD(K, 6000) AS G, K, V FROM T) AS Q GROUP BY G";
    for (grant, spilled) in [(1_048_576_i128, false), (131_072_i128, true)] {
        set(&db, "MEMORY.WORK_BYTES", grant);
        let narrow = led(&a, sql, 2, 3);
        assert_eq!(narrow.rows.len(), 6000);
        assert!(narrow.waves > 0, "{} waves of a grant of {grant}", narrow.waves);
        assert_eq!(narrow.spills > 0, spilled, "{} spills of a grant of {grant}", narrow.spills);
        for lead in [16, 64] {
            assert_eq!(led(&a, sql, lead, 3), narrow, "a lead of {lead}, a grant of {grant}");
        }
    }
}

/// A grant under which the groupings of T of 6,000 groups merge several
/// waves and hold their partitions in memory.
const WAVED: i128 = 4_194_304;

/// Verifies: [DESIGN] RULES.md D1300, D356 a grouping gives the same
/// groups, in the same order, each set function the same value, whether
/// its partitions go wide or not: at their first group, past five, past
/// forty, and never; of few groups a partition and of many, of counts,
/// sums, means and the least and greatest of texts equal as padded;
/// whichever morsels are handed whole; under a grant that holds every
/// group, one under which several waves are merged, so a wide partition
/// is kept and taken up again, and one the groups pass, so a wide
/// partition is gathered and written to the spill.
#[test]
fn the_groups_are_the_same_whether_partitions_go_wide() {
    let (db, a) = table();
    let statements = [
        ("MOD(K, 40)", "COUNT(*), SUM(V), MIN(V), MAX(K)"),
        ("K - MOD(K, 10)", "COUNT(*), SUM(V), MIN(P), MAX(K)"),
        ("MOD(K, 6000)", "COUNT(*), SUM(V), AVG(V), MIN(P), MAX(P)"),
        ("K", "COUNT(*), SUM(V), AVG(V), MAX(P)"),
    ];
    let (mut kept_wide, mut seen) = (false, Vec::new());
    for (keyed, set_functions) in statements {
        let sql = format!("SELECT G, {set_functions} FROM (SELECT {keyed} AS G, K, V, P FROM T) AS Q GROUP BY G");
        for grant in [HELD, WAVED, PASSED] {
            set(&db, "MEMORY.WORK_BYTES", grant);
            let never = How { forced: NONE, widen: Widen::past(usize::MAX), ..How::default() };
            let expected = waved(&a, &sql, &never, 3);
            assert_eq!(never.widen.merged.load(Ordering::Relaxed), 0, "no partition wide");
            let ways: [(usize, Forced); 4] = [(0, NONE), (5, NONE), (5, None), (40, Some(|m| m.is_multiple_of(2)))];
            for (past, forced) in ways {
                let how = How { forced, widen: Widen::past(past), ..How::default() };
                let made = waved(&a, &sql, &how, 3);
                assert_eq!(differing(&made, &expected), None, "wide past {past} groups, grant {grant}: {sql}");
                let wide = how.widen.merged.load(Ordering::Relaxed);
                // Twice as many groups a partition as a table holds before
                // it goes wide, unless each wave's go to the spill alone.
                if made.spills == 0 && expected.rows.len() > past.saturating_mul(PARTS).saturating_mul(2) {
                    assert!(wide > 0, "no partition went wide past {past} groups, grant {grant}: {sql}");
                }
                kept_wide |= made.waves > 0 && made.spills == 0 && wide > PARTS;
                seen.push((expected.rows.len(), grant, past, made.waves, made.spills, wide));
            }
        }
    }
    assert!(kept_wide, "no wide partition was kept between waves and taken up again with no spill: {seen:?}");
}

/// The first `most` rows each partition keeps of the groups of `sql`, a
/// grouping of T by one column, under a sort by its set function at
/// `by`, the greatest first, its waves run as `how` says on three
/// threads: every partition's rows, in the order of their text.
fn first_rows(a: &Session, sql: &str, how: &How, (by, most): (usize, usize)) -> Vec<String> {
    let (st, optimized) = a.optimized(sql).unwrap();
    let planned = PhysicalPlanner::new(&st).plan(&optimized);
    let aggregate = if let Plan::Project { input, .. } = planned { *input } else { planned };
    let db = Db::of(&st);
    let (mut grouping, read) = grouping_of(&aggregate, (None, None), st.functions.seed().clone()).unwrap();
    let limit = Limit::of(&db);
    let Held::Kept(pipeline) = Pipeline::told(read, (&db, NO_TRANSACTION), &limit, &[]).unwrap() else { panic!("the pipeline gave way") };
    grouping.read = pipeline.batched.as_ref().and_then(|by_batch| read_of(&grouping, by_batch));
    let unpadded = grouping.read.as_ref().map(|by_batch| by_batch.unpadded.clone());
    grouping.seed = grouping.seed.unpadded(&unpadded.expect("grouped by batch"));
    grouping.top = Some(Arc::new(Top::by_function((1, grouping.spec.aggs.len()), (by, most), (&db, NO_TRANSACTION))));
    let Held::Kept((runs, given)) = waved_as(pipeline, &Arc::new(grouping), (&db, NO_TRANSACTION, 3), &limit, how).unwrap() else {
        panic!("the grouping gave way")
    };
    let mut rows: Vec<String> = Ordered::of(runs, given).unwrap().map(|row| format!("{:?}", row.unwrap())).collect();
    rows.sort();
    rows
}

/// Verifies: [DESIGN] RULES.md D1300, D1256 the first rows a partition
/// keeps for a sort by a set function are the same rows, each with the
/// same values and first row's number, whether the partition is wide,
/// and its groups read where they lie, or a table: by a count, a sum and
/// a greatest value, of groupings whose groups took one row each, whose
/// groups took rows of several morsels, of one where some did and some
/// did not, so the rows are picked of both, and of few groups, with ties
/// cut by the first rows' numbers.
#[test]
fn the_first_rows_by_a_set_function_are_the_same_of_wide_partitions() {
    let (_db, a) = table();
    let sorts = [("K", 1_usize), ("K - MOD(K, 10)", 1), ("K - MOD(K, 7)", 1), ("MOD(K, 6000)", 0), ("MOD(K, 6000)", 2), ("MOD(K, 40)", 1)];
    for (keyed, by) in sorts {
        let sql = format!("SELECT G, COUNT(*), SUM(V), MAX(K) FROM (SELECT {keyed} AS G, K, V FROM T) AS Q GROUP BY G");
        let never = How { forced: NONE, widen: Widen::past(usize::MAX), ..How::default() };
        let expected = first_rows(&a, &sql, &never, (by, 7));
        assert!(expected.len() > 30 && expected.len() <= PARTS.saturating_mul(7), "{} rows: {sql}", expected.len());
        for past in [0_usize, 5] {
            let how = How { forced: NONE, widen: Widen::past(past), ..How::default() };
            assert_eq!(first_rows(&a, &sql, &how, (by, 7)), expected, "wide past {past} groups, by {by}: {sql}");
            let wide = how.widen.merged.load(Ordering::Relaxed);
            assert!(past > 0 || wide > 0, "no partition went wide: {sql}");
        }
    }
}
