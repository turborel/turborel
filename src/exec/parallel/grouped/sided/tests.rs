//! Unit tests of a pair's side (RULES.md D1324): which pairs keep their
//! plain set functions by the second's columns, and that such a pair
//! gives the groups it gives with a part's state a group of the first,
//! however its morsels are handed on and whether or not its partitions go
//! to the spill.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::super::super::pipeline::Pipeline;
use super::super::super::{Held, Limit};
use super::super::waves::{Forced, How, waved_as};
use super::super::{Ordered, Outer, grouping_of, read_of};
use super::merged_parts;
use crate::db::{Database, Session};
use crate::exec::Db;
use crate::exec::batch::Taken;
use crate::functions::{AggregateRef, Merged, Registry};
use crate::logical::{AggCall, Cond, Expr, Truth};
use crate::physical::{PhysicalPlanner, Plan};
use crate::storage::Row;
use crate::txn::NO_TRANSACTION;
use crate::value::{DataType, Decimal, Spelled, Value};

/// T, kept by column, of 24,000 rows in morsels of 1,000: K; G, one of
/// 40 in runs of 30 rows, null in every 17th run; V, scattered, null in
/// every 11th row; F, an approximate number; S, a short string; P, one
/// letter followed by none, one or two spaces, under a collation of NO
/// PAD by octets, so three strings equal but for their trailing spaces.
fn table() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A CREATE COLLATION BYTES FOR SQL_TEXT FROM UCS_BASIC NO PAD
           CREATE TABLE T (K INTEGER NOT NULL, G INTEGER, V INTEGER, F DOUBLE PRECISION, S VARCHAR(8), P VARCHAR(4) COLLATE A.BYTES)
           USING columnstore",
    )
    .unwrap();
    let a = db.session("A");
    for first in (0_i64..24_000).step_by(1000) {
        let row = |k: i64| {
            let run = k.div_euclid(30);
            let g = if run.rem_euclid(17) == 0 { "NULL".to_owned() } else { run.rem_euclid(40).to_string() };
            let v = if k.rem_euclid(11) == 0 { "NULL".to_owned() } else { k.wrapping_mul(7).rem_euclid(101).to_string() };
            let p = ["x", "x ", "x  "][usize::try_from(k.wrapping_mul(5).rem_euclid(3)).unwrap()];
            format!("({k}, {g}, {v}, {}E-1, 's{}', '{p}')", k.rem_euclid(50), k.rem_euclid(77))
        };
        let values: Vec<String> = (first..first.saturating_add(1000)).map(row).collect();
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

/// What a pair made: whether it kept a side, its groups as their rows
/// are written out, in the order given, or none if it gave way, and the
/// times its partitions were written to the spill.
#[derive(Debug, PartialEq, Eq)]
struct Paired {
    side: bool,
    rows: Option<Vec<String>>,
    spills: usize,
}

/// What the pair of `sql`, a grouping with a DISTINCT set function over
/// T, made on three threads, run as `how` says, with a side if `sided`
/// and its plain functions are kept so.
fn paired(a: &Session, sql: &str, sided: bool, how: &How) -> Paired {
    let (st, optimized) = a.optimized(sql).unwrap();
    let planned = PhysicalPlanner::new(&st).plan(&optimized);
    let second = if let Plan::Project { input, .. } = planned { *input } else { planned };
    // A plan that is no pair of groupings gave no rows as one.
    let Some((outer, first)) = Outer::of(&second) else { return Paired { side: false, rows: None, spills: 0 } };
    let db = Db::of(&st);
    let (mut grouping, read) = grouping_of(first, (None, Some(outer)), st.functions.seed().clone()).unwrap();
    let limit = Limit::of(&db);
    let Held::Kept(pipeline) = Pipeline::told(read, (&db, NO_TRANSACTION), &limit, &[]).unwrap() else { panic!("the pipeline gave way") };
    grouping.read = pipeline.batched.as_ref().and_then(|by| read_of(&grouping, by));
    let unpadded = grouping.read.as_ref().map(|by_batch| by_batch.unpadded.clone());
    grouping.seed = grouping.seed.unpadded(&unpadded.expect("grouped by batch"));
    if sided {
        grouping.sided().unwrap();
    }
    let side = grouping.side().is_some();
    let waved = waved_as(pipeline, &Arc::new(grouping), (&db, NO_TRANSACTION, 3), &limit, how).unwrap();
    let spills = how.counted.1.load(Ordering::Relaxed);
    let Held::Kept((runs, given)) = waved else { return Paired { side, rows: None, spills } };
    let rows = Ordered::of(runs, given).unwrap().collect::<Result<Vec<Row>, _>>().unwrap();
    Paired { side, rows: Some(rows.iter().map(|row| format!("{row:?}")).collect()), spills }
}

/// The pairs whose plain functions are kept in a side: counts and exact
/// totals beside one DISTINCT function and beside two, by one column and
/// by two.
const SIDED: [&str; 4] = [
    "SELECT G, COUNT(DISTINCT V), SUM(V), COUNT(*), AVG(K), COUNT(V) FROM T GROUP BY G",
    "SELECT G, SUM(K), COUNT(DISTINCT V) FROM T WHERE K < 20000 GROUP BY G",
    "SELECT G, M, COUNT(*), MIN(K), MAX(V), SUM(DISTINCT V) FROM (SELECT G, MOD(K, 3) AS M, K, V FROM T) AS Q GROUP BY G, M",
    "SELECT K2, COUNT(DISTINCT V), COUNT(*), SUM(V) FROM (SELECT K - MOD(K, 4) AS K2, V FROM T) AS Q GROUP BY K2",
];

/// A way a pair's morsels are handed on, and its name.
type Handed = (&'static str, fn() -> How);

/// Every morsel handed whole, and none.
const EVERY: Forced = Some(|_| true);
const NONE: Forced = Some(|_| false);

/// Verifies: [DESIGN] RULES.md D1324, D1280 [ISO/IEC 9075-2:2023 §10.9
/// GR7b i]: a pair whose plain set functions are counts and exact totals
/// keeps them in a side, and gives the groups, in the order, with the
/// values it gives when each group of the first holds a part's state:
/// run as the executor runs it, every morsel handed whole, none, and
/// every morsel's rows handed on ungrouped; of groups with a null key, of
/// values all null, of a key of two columns and of more groups than a
/// morsel has rows.
#[test]
fn a_sided_pair_gives_the_groups_of_a_pair_of_parts() {
    let (_db, a) = table();
    let hows: [Handed; 4] = [
        ("as the executor runs it", How::default),
        ("every morsel whole", || How { forced: EVERY, ..How::default() }),
        ("no morsel whole", || How { forced: NONE, ..How::default() }),
        ("rows handed on ungrouped", || How { forced: NONE, apart: EVERY, ..How::default() }),
    ];
    for sql in SIDED {
        let parts = paired(&a, sql, false, &How::default());
        assert!(!parts.side && parts.rows.as_ref().is_some_and(|rows| rows.len() > 30), "{sql}: {parts:?}");
        for (which, how) in hows {
            let sided = paired(&a, sql, true, &how());
            assert!(sided.side, "{which}: {sql}");
            assert_eq!(sided.rows, parts.rows, "{which}: {sql}");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1324, D1280: a pair keeps no side where
/// its first grouping computes no set function, where one of its parts
/// is no column of counts or exact totals, the least of strings; each
/// runs as a pair of parts, as before. A sum or an average of
/// approximate numbers, and a grouping by no column, keep none either,
/// whether the plan makes a pair of them or not.
#[test]
fn a_pair_of_other_parts_keeps_no_side() {
    let (_db, a) = table();
    for sql in ["SELECT G, COUNT(DISTINCT V) FROM T GROUP BY G", "SELECT G, COUNT(DISTINCT V), COUNT(*), MIN(S) FROM T GROUP BY G"] {
        let made = paired(&a, sql, true, &How::default());
        assert!(!made.side && made.rows.is_some(), "{sql}: {made:?}");
    }
    for sql in [
        "SELECT G, COUNT(DISTINCT V), AVG(F) FROM T GROUP BY G",
        "SELECT G, COUNT(DISTINCT V), COUNT(*), SUM(F) FROM T GROUP BY G",
        "SELECT COUNT(DISTINCT V), COUNT(*) FROM T",
    ] {
        assert!(!paired(&a, sql, true, &How::default()).side, "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md D1324, D1184 [ISO/IEC 9075-2:2023 §4.3.2]:
/// a side's key is grouped as the pair groups it, a string under a
/// collation of NO PAD by its octets, so three strings equal but for
/// their trailing spaces are three groups of the side as of the pair,
/// each with its own count and total.
#[test]
fn a_side_groups_strings_as_the_pair_groups_them() {
    let (_db, a) = table();
    let sql = "SELECT P, COUNT(DISTINCT V), COUNT(*), SUM(K) FROM T GROUP BY P";
    let (parts, sided) = (paired(&a, sql, false, &How::default()), paired(&a, sql, true, &How::default()));
    assert!(sided.side && sided.rows.as_ref().is_some_and(|rows| rows.len() == 3), "{sided:?}");
    assert_eq!(sided.rows, parts.rows);
}

/// A call of the built-in set function `name` of the column at `at`, an
/// INTEGER, or of rows of none.
fn call(name: &str, at: Option<usize>) -> AggCall {
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    let func = Registry::with_set_functions().aggregate(name).unwrap();
    let result = func.0.result_type(at.map(|_| integer)).unwrap();
    AggCall { func, arg: at.map(Expr::Column), distinct: false, result, filter: None, distribution: None, label: name.to_owned() }
}

/// The call merging the parts of `of` that the column at `at` holds.
fn merged(of: &AggCall, at: usize) -> AggCall {
    let func = AggregateRef(Arc::new(Merged { func: of.func.clone(), result: of.result.clone() }));
    AggCall { func, arg: Some(Expr::Column(at)), ..of.clone() }
}

/// Verifies: [DESIGN] RULES.md D1324, D1280: of the second grouping's
/// set functions over a first of two grouping columns, each that merges
/// a part whole names it and one fed a grouping column names none; and
/// no side is kept where two functions read one part, where a function
/// that is no merge reads a part, where a merge is filtered, or where a
/// part is kept as a group's first row's value.
#[test]
fn a_part_read_any_other_way_than_merged_whole_keeps_no_side() {
    let (sum, count) = (call("SUM", Some(0)), call("COUNT", None));
    let fed = call("COUNT", Some(1));
    let whole = [merged(&sum, 2), fed.clone(), merged(&count, 3)];
    let taken = [Taken::Column(2), Taken::Column(1), Taken::Column(3)];
    assert_eq!(merged_parts(&whole, &taken, 2), Some(vec![Some(0), None, Some(1)]));
    let twice = [merged(&sum, 2), merged(&sum, 2)];
    assert_eq!(merged_parts(&twice, &[Taken::Column(2), Taken::Column(2)], 2), None, "two read one part");
    assert_eq!(merged_parts(&[call("SUM", Some(2))], &[Taken::Column(2)], 2), None, "a part summed as a value");
    let filtered = AggCall { filter: Some(Cond::Const(Truth::True)), ..merged(&sum, 2) };
    assert_eq!(merged_parts(&[filtered], &[Taken::Column(2)], 2), None, "a merge under a filter");
    assert_eq!(merged_parts(&[merged(&sum, 2)], &[Taken::Kept(2)], 2), None, "a part kept as a first row's value");
    assert_eq!(merged_parts(&[fed], &[Taken::Count], 2), Some(vec![None]));
}

/// A grant the first grouping's groups pass, so its partitions go to the
/// spill, that holds the side of few groups.
const PASSED: i128 = 131_072;

/// Verifies: [DESIGN] RULES.md D1324, D1226: a sided pair whose first
/// grouping's partitions pass the grant writes them to the spill and
/// gives the groups it gives within the grant, the side held in memory;
/// and one whose side alone passes the grant, of a group every four
/// rows, gives way, where the pair of parts spills and finishes.
#[test]
fn a_sided_pair_spills_its_partitions_and_gives_way_past_its_side() {
    let (db, a) = table();
    let [few, _, _, many] = SIDED;
    let held = paired(&a, few, true, &How::default());
    let wide = paired(&a, many, false, &How::default());
    set(&db, "MEMORY.WORK_BYTES", PASSED);
    let spilled = paired(&a, few, true, &How::default());
    assert!(spilled.side && spilled.spills > 0, "{spilled:?}");
    assert_eq!(spilled.rows, held.rows);
    let past = paired(&a, many, true, &How::default());
    assert_eq!((past.side, past.rows), (true, None), "the side passes the grant");
    let parts = paired(&a, many, false, &How::default());
    assert!(parts.spills > 0 && parts.rows == wide.rows, "{}", parts.spills);
}
