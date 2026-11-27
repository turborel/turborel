//! Unit tests of a pipeline whose first filter hears what its scan judged
//! of each batch (`parallel/batched.rs`, RULES.md D1266): the rows it
//! gives are those it gives with the filter made deaf, which judges every
//! conjunct of every row given, as it did before the scan said anything.

use super::super::pipeline::{Pipeline, Step};
use super::super::{Held, Limit};
use super::by_batch;
use crate::db::{Database, Session};
use crate::exec::Db;
use crate::exec::batch::rows_of;
use crate::physical::{PhysicalPlanner, Plan};
use crate::txn::NO_TRANSACTION;
use crate::value::{Decimal, Value};

/// Rows of T written before the freeze, three groups of 2,048.
const ROWS: usize = 6144;

/// A row of T: K its key; N one of a hundred numbers, null every 7th
/// row; W one of five words, null every 17th; U a web address, null
/// every 11th.
fn values(k: usize) -> String {
    let null_or = |null: bool, v: String| if null { "NULL".to_owned() } else { v };
    let n = null_or(k.is_multiple_of(7), (k % 100).to_string());
    let w = null_or(k.is_multiple_of(17), format!("'{}'", ["red", "green", "blue", "a green one", "grey"][k % 5]));
    let u = null_or(
        k.is_multiple_of(11),
        format!("'http://site{}.example/page{}'", k.wrapping_mul(7919) % 300, k.wrapping_mul(104_729) % 50_000),
    );
    format!("({k}, {n}, {w}, {u})")
}

/// T, kept by column in groups of 2,048 rows, frozen; then rows written,
/// and rows of its first group changed and deleted, and left in its
/// tree, which its scan judges nothing of. So the first group's runs are
/// read at the places the tree has no entry for, and the other groups'
/// runs whole where their conjuncts keep most rows.
fn table() -> (Database, Session) {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE T (K INTEGER NOT NULL, N INTEGER, W VARCHAR(20), U VARCHAR(80))
             USING columnstore WITH (COLUMNSTORE.GROUP_ROWS = 2048)",
    )
    .unwrap();
    let a = db.session("A");
    for first in (0..ROWS).step_by(512) {
        let written: Vec<String> = (first..first.saturating_add(512)).map(values).collect();
        a.run(&format!("INSERT INTO T VALUES {}", written.join(", "))).unwrap();
    }
    a.commit().unwrap();
    db.maintain().unwrap();
    let written: Vec<String> = (20_000..20_200).map(values).collect();
    a.run(&format!("INSERT INTO T VALUES {}", written.join(", "))).unwrap();
    a.run("UPDATE T SET N = 99 - N, W = 'grey', U = 'http://site12.example/moved' WHERE MOD(K, 37) = 4 AND K < 2048").unwrap();
    a.run("DELETE FROM T WHERE MOD(K, 41) = 5 AND K < 2048").unwrap();
    a.commit().unwrap();
    db.set_setting("EXECUTOR.MORSEL_ROWS", &Value::Exact(Decimal::new(4096, 0))).unwrap();
    (db, a)
}

/// The filter over T's scan that `SELECT K, N, W, U FROM T WHERE
/// predicate` plans, under its projection if it has one.
fn filter_of(a: &Session, predicate: &str) -> (Db, Plan) {
    let (st, optimized) = a.optimized(&format!("SELECT K, N, W, U FROM T WHERE {predicate}")).unwrap();
    let mut plan = PhysicalPlanner::new(&st).plan(&optimized);
    while let Plan::Project { input, .. } = plan {
        plan = *input;
    }
    assert!(matches!(&plan, Plan::Filter { input, .. } if matches!(&**input, Plan::SeqScan { .. })), "a filter over a scan: {plan:?}");
    (Db::of(&st), plan)
}

/// The rows the pipeline of `filter` gives, each written out, in order
/// of their text, and whether its filter heard its scan: as it is made,
/// or, if `deaf`, with the scan's word unheard, every conjunct judged of
/// every row the scan gives.
fn kept(db: &Db, filter: &Plan, deaf: bool) -> (Vec<String>, bool) {
    let Held::Kept(mut pipeline) = Pipeline::of(filter, (db, NO_TRANSACTION), &Limit::of(db)).unwrap() else {
        panic!("the pipeline gave way")
    };
    if deaf {
        for branch in &mut pipeline.branches {
            branch.vets = false;
        }
        pipeline.batched = by_batch(&pipeline.branches, db);
    }
    let heard = pipeline.branches.iter().any(|branch| branch.told_filter().is_some());
    let by = pipeline.batched.as_ref().expect("read by batch");
    let mut rows = Vec::new();
    for n in 0..pipeline.count() {
        let read = rows_of(pipeline.morsel_kept(n, (db, NO_TRANSACTION)).unwrap(), by.layout.clone(), by.width);
        rows.extend(read.map(|row| format!("{:?}", row.unwrap())));
    }
    rows.sort_unstable();
    (rows, heard)
}

/// Verifies: [DESIGN] RULES.md D1266, D15 a pipeline whose first filter
/// passes over the conjuncts its scan judged exactly gives the rows it
/// gives with that filter made deaf, which judges every conjunct of
/// every row: of comparisons, IS NULL, IN and NOT IN, an OR, a LIKE that
/// keeps few rows and LIKEs that keep most, whose judging stops part way
/// through each run with nulls and texts they are false of left unjudged, of conjuncts the scan is not told beside them, and
/// of a table with nulls in each column judged and with rows written,
/// changed and deleted since it was frozen, which the scan gives from
/// its tree and says nothing of. The filter that hears is the scan's
/// own, the first step after any counting, unless its condition can
/// raise an error.
#[test]
fn a_pipeline_gives_the_same_rows_with_its_filter_hearing_and_deaf() {
    let (_db, a) = table();
    let predicates = [
        "N < 50",
        "N = 7",
        "N IS NULL",
        "N IS NOT NULL AND W <> 'red'",
        "N IN (1, 2, 3, 50, 51) AND W IS NOT NULL",
        "N NOT IN (1, 2, 3)",
        "N < 5 OR N > 95 OR N IS NULL",
        "W = 'grey'",
        "W IN ('red', 'grey') AND N >= 10",
        "U LIKE '%site12%'",
        "U LIKE 'http://%'",
        "U LIKE '%http%'",
        "U LIKE '%site1%'",
        "U NOT LIKE '%site1%' AND N < 80",
        "W LIKE '%e%'",
        "N + K > 3000 AND N < 50",
        "N < K AND N < 50 AND U LIKE 'http://%'",
        "N < 50 AND W LIKE 'g%' AND U LIKE '%page1%'",
    ];
    let mut dropped = 0_usize;
    for predicate in predicates {
        let (db, filter) = filter_of(&a, predicate);
        let (hearing, heard) = kept(&db, &filter, false);
        let (deaf, unheard) = kept(&db, &filter, true);
        let Plan::Filter { cond, .. } = &filter else { panic!("not a filter") };
        assert_eq!((heard, unheard), (!cond.can_raise(), false), "{predicate}: a filter that cannot raise hears, none once deaf");
        assert_eq!(hearing, deaf, "{predicate}");
        assert!(!hearing.is_empty(), "{predicate}: some row is kept");
        dropped = dropped.saturating_add(usize::from(hearing.len() < ROWS));
    }
    assert_eq!(dropped, predicates.len(), "each predicate leaves rows out");
}

/// Verifies: [DESIGN] RULES.md D1266, D15 (MC/DC) the filter that hears
/// a branch's scan is the first of its steps past any counting, and only
/// of a branch whose scan was told that filter's conjuncts: none of a
/// branch not so told, none if a step that is no count comes before the
/// filter, and the filter after a count. A filter whose condition can
/// raise an error hears nothing, and is given every row its scan gives.
#[test]
fn the_filter_that_hears_is_the_scans_own() {
    let (_db, a) = table();
    let (db, filter) = filter_of(&a, "N < 50");
    let Held::Kept(mut pipeline) = Pipeline::of(&filter, (&db, NO_TRANSACTION), &Limit::of(&db)).unwrap() else { panic!("gave way") };
    let branch = &mut pipeline.branches[0];
    assert_eq!(branch.told_filter(), Some(0));
    branch.vets = false;
    assert_eq!(branch.told_filter(), None, "a scan not told the filter's conjuncts");
    branch.vets = true;
    branch.steps.insert(0, Step::Project(Vec::new()));
    assert_eq!(branch.told_filter(), None, "a step before the filter that is no count");
    branch.steps.remove(0);
    let Plan::Filter { cond, .. } = &filter else { panic!("not a filter") };
    branch.steps.push(Step::Filter(cond.clone()));
    assert_eq!(branch.told_filter(), Some(0), "the first filter, not one after it");
    branch.steps.clear();
    assert_eq!(branch.told_filter(), None, "no step at all");
    let (raising_db, raising) = filter_of(&a, "K / (N + 1000) >= 0 AND N < 50");
    let Held::Kept(of_raising) = Pipeline::of(&raising, (&raising_db, NO_TRANSACTION), &Limit::of(&raising_db)).unwrap() else {
        panic!("gave way")
    };
    assert_eq!(of_raising.branches[0].told_filter(), None, "a filter that can raise an error");
}
