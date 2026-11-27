//! Unit tests of a grouping's waves: its groups are the same whichever of
//! its morsels are handed whole to the statement's thread (RULES.md
//! D1247).

use std::sync::Arc;

use super::super::super::pipeline::Pipeline;
use super::super::super::probe::PARTS;
use super::super::super::{Held, Limit};
use super::super::{Ordered, grouping_of, read_of};
use super::{Forced, few, waved_as};
use crate::db::{Database, Session};
use crate::exec::Db;
use crate::physical::{PhysicalPlanner, Plan};
use crate::storage::Row;
use crate::txn::NO_TRANSACTION;
use crate::value::{Decimal, Value};

/// Rows of T.
const ROWS: i64 = 24_000;

/// T, kept by column, of `ROWS` rows: K, and V, scattered among them.
fn table() -> (Database, Session) {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL, V INTEGER NOT NULL) USING columnstore").unwrap();
    let a = db.session("A");
    for first in (0..ROWS).step_by(1000) {
        let v = |k: i64| k.wrapping_mul(7).rem_euclid(1013);
        let values: Vec<String> = (first..first.saturating_add(1000)).map(|k| format!("({k}, {})", v(k))).collect();
        a.run(&format!("INSERT INTO T VALUES {}", values.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    let set = |name: &str, value: i128| db.set_setting(name, &Value::Exact(Decimal::new(value, 0))).unwrap();
    set("EXECUTOR.MORSEL_ROWS", 1000);
    set("EXECUTOR.WORKERS", 3);
    (db, a)
}

/// The groups of `sql`, a grouping of T under a projection, made by its
/// waves on three threads, the morsels `forced` names handed whole, or
/// those `waved` hands so: each group's row as its values are written
/// out, in the order they are given, and how many runs of groups its
/// waves ended with, one of a grouping merged on the statement's thread
/// alone, else one a partition.
fn groups_of(a: &Session, sql: &str, forced: Forced) -> (Vec<String>, usize) {
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
    let Held::Kept((runs, given)) = waved_as(pipeline, &Arc::new(grouping), (&db, NO_TRANSACTION, 3), &limit, forced).unwrap() else {
        panic!("the grouping gave way")
    };
    let count = runs.len();
    let rows = Ordered::of(runs, given).unwrap().collect::<Result<Vec<Row>, _>>().unwrap();
    (rows.iter().map(|row| format!("{row:?}")).collect(), count)
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
