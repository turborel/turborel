//! Unit tests of a grouping's waves over rows of one key that come one
//! after another (RULES.md D1325): the morsels' tables are judged by the
//! rows they looked up, a morsel handed on ungrouped hands on an entry a
//! run where the tables followed runs, such a morsel is never handed
//! whole, and the groups are the same whichever morsels hand their runs
//! on.

use std::sync::atomic::AtomicBool;

use super::super::{Handed, grouped};
use super::*;
use crate::exec::table::GroupTable;

/// R, kept by column, of as many rows as T in the order of K. G holds
/// each value ten rows running, a value a run, and is null in every
/// thirteenth run; U each of three values five rows running; H each of
/// seven values ten rows running. V is scattered, W null in every
/// seventh row and in every fourth stretch of five rows, and P as T's.
fn runs_table() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE R (K INTEGER NOT NULL, G INTEGER, U INTEGER NOT NULL, H INTEGER NOT NULL, V INTEGER NOT NULL, W INTEGER,
                           P VARCHAR(4) NOT NULL) USING columnstore",
    )
    .unwrap();
    let a = db.session("A");
    for first in (0..ROWS).step_by(1000) {
        let row = |k: i64| {
            let g = if k.div_euclid(10) % 13 == 5 { "NULL".to_owned() } else { k.div_euclid(10).to_string() };
            let w = if k % 7 == 0 || k.div_euclid(5) % 4 == 1 { "NULL".to_owned() } else { (k % 89).to_string() };
            let p = ["x", "x ", "x  "][usize::try_from(k.wrapping_mul(5).rem_euclid(3)).unwrap()];
            format!("({k}, {g}, {}, {}, {}, {w}, '{p}')", k.div_euclid(5) % 3, k.div_euclid(10) % 7, k.wrapping_mul(7).rem_euclid(1013))
        };
        let values: Vec<String> = (first..first.saturating_add(1000)).map(row).collect();
        a.run(&format!("INSERT INTO R VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    set(&db, "EXECUTOR.MORSEL_ROWS", 1000);
    set(&db, "EXECUTOR.WORKERS", 3);
    (db, a)
}

/// What each of the first `morsels` morsels of `sql`, a grouping of R,
/// hands over, grouped one after another on one of three threads as a
/// statement groups them, morsels handed whole while `still`: whether
/// whole, whether two of its groups may hold one key, and its groups,
/// those of a morsel handed whole counted as a table merges them.
fn handed(a: &Session, sql: &str, (morsels, still): (usize, bool)) -> Vec<(bool, bool, usize)> {
    let (st, optimized) = a.optimized(sql).unwrap();
    let planned = PhysicalPlanner::new(&st).plan(&optimized);
    let aggregate = if let Plan::Project { input, .. } = planned { *input } else { planned };
    let db = Db::of(&st);
    let (mut grouping, read) = grouping_of(&aggregate, (None, None), st.functions.seed().clone()).unwrap();
    let limit = Limit::of(&db);
    let Held::Kept(pipeline) = Pipeline::told(read, (&db, NO_TRANSACTION), &limit, &[]).unwrap() else { panic!("the pipeline gave way") };
    assert!(pipeline.count() > morsels, "morsels: {}", pipeline.count());
    grouping.read = pipeline.batched.as_ref().and_then(|by| read_of(&grouping, by));
    let unpadded = grouping.read.as_ref().map(|by_batch| by_batch.unpadded.clone());
    grouping.seed = grouping.seed.unpadded(&unpadded.expect("grouped by batch"));
    let width = grouping.read.as_ref().map_or(0, |by_batch| by_batch.groups.len());
    let grouping = Arc::new(grouping);
    let work = grouped(pipeline, &grouping, (&db, NO_TRANSACTION, 3), (Arc::new(AtomicBool::new(still)), None, None));
    let told = |m: usize| match work(m).unwrap() {
        Handed::Whole(made) => {
            let mut table = GroupTable::new(width, &grouping.spec.aggs, &grouping.seed, usize::MAX);
            table.absorb_whole(&made).unwrap();
            (true, made.repeats(), table.len())
        }
        Handed::Parted(made) => (false, made.repeats(), (0..PARTS).map(|p| made.groups_of(p)).sum()),
    };
    (0..morsels).map(told).collect()
}

/// Verifies: [DESIGN] RULES.md D1325, D1260 a morsel's table is judged
/// by the rows it looked up. Of rows that come five of a key running,
/// each run a key the morsel has not held, the first morsel's table
/// reduces its thousand rows to under a fifth and still finds no group
/// for a row it looks up, so the morsels after it hand their rows on
/// ungrouped, an entry a run of five rows and not a row. Of runs of ten
/// rows of seven keys in all the table finds its groups, and the morsels
/// go on making tables. Of rows none of which repeats the one before it,
/// the morsels hand on an entry a row, as before. And a morsel that
/// hands its runs on is split among the partitions though its entries
/// are few beside its rows and morsels are still handed whole, as its
/// entries do not lie in the order of their first rows.
///
/// Decision: handed whole of a morsel not handed on ungrouped whose
/// groups are few. A table of few groups while morsels are handed whole:
/// whole. A morsel handed on, its entries as few: split (handed on
/// ungrouped alone differs).
#[test]
fn morsels_hand_on_their_runs_after_a_table_that_found_no_group_for_its_lookups() {
    let (_db, a) = runs_table();
    let by_runs = handed(&a, "SELECT G, U, COUNT(*), SUM(V) FROM R GROUP BY G, U", (2, false));
    assert_eq!((by_runs[0].0, by_runs[0].1), (false, false), "the first morsel makes a table");
    assert!(by_runs[0].2 > 150 && by_runs[0].2 <= 200, "the groups of 200 runs, the null keys' few: {}", by_runs[0].2);
    assert_eq!(by_runs[1], (false, true, 200), "handed on, an entry a run of five rows");
    let found = handed(&a, "SELECT H, COUNT(*), SUM(V) FROM R GROUP BY H", (2, false));
    assert_eq!(found, [(false, false, 7), (false, false, 7)], "runs of seven keys are grouped in their morsel");
    let by_rows = handed(&a, "SELECT K, COUNT(*), SUM(V) FROM R GROUP BY K", (2, false));
    assert_eq!(by_rows, [(false, false, 1000), (false, true, 1000)], "no row repeats: handed on a row each");
    let long = handed(&a, "SELECT G, COUNT(*), SUM(V) FROM R GROUP BY G", (2, true));
    assert_eq!(long[0], (true, false, 93), "a table of 93 groups of a thousand rows is handed whole");
    assert_eq!(long[1], (false, true, 100), "100 entries of a thousand rows handed on are split among the partitions");
}

/// Verifies: [DESIGN] RULES.md D1325, D1313 [X3.135-1986 §5.22 GR2] a
/// grouping gives the same groups, in the same order, each set function
/// the same value, whichever of its morsels hand their runs on an entry
/// each: those the tables tell of, every one, every other and the second
/// half alone, against none. Of a key of two columns whose runs end
/// apart, nulls in one; of rows a filter takes out of the middle of
/// runs; and of one column of runs. COUNT(\*), sums, a mean and a count
/// of a column with nulls, whole runs of them among them, extremes, a
/// sum of an expression, which is fed every row, and MIN of strings
/// equal as they compare padded, which is a group's first row's only if
/// the entries are merged in the order their runs came. Split among the
/// partitions, as the executor hands them, and handed whole; held by
/// the grant and through the spill.
#[test]
fn the_groups_are_the_same_whichever_morsels_hand_their_runs_on() {
    let (db, a) = runs_table();
    let named: [Named; 4] = [
        ("as the tables tell", None),
        ("every morsel", EVERY),
        ("every other", Some(|m| m.is_multiple_of(2))),
        ("the second half", Some(|m| m >= 12)),
    ];
    let taken = "COUNT(*), SUM(V), MIN(V), MAX(K), MIN(P), AVG(W), COUNT(W), MAX(W), SUM(V + K)";
    for (keyed, kept, groups) in [("G, U", "", 4433_usize), ("G, U", " WHERE MOD(K, 4) <> 1", 4433), ("G", "", 2216)] {
        let sql = format!("SELECT {keyed}, {taken} FROM R{kept} GROUP BY {keyed}");
        set(&db, "MEMORY.WORK_BYTES", HELD);
        let expected = waved(&a, &sql, &How { forced: NONE, apart: NONE, ..How::default() }, 3);
        assert_eq!((expected.rows.len(), expected.spills), (groups, 0), "{sql}");
        for (grant, (which, apart), (forced, _)) in ways(&named) {
            set(&db, "MEMORY.WORK_BYTES", grant);
            let made = waved(&a, &sql, &How { forced, apart, ..How::default() }, 3);
            assert_eq!(differing(&made, &expected), None, "{which}, a grant of {grant}, {} spills: {sql}", made.spills);
        }
    }
}
