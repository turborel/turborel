// Unit tests: a pipeline over a hash join built in parts past the grant,
// probed in passes (RULES.md R-TEST-1, R-TEST-8).

use std::sync::Arc;

use super::super::pipeline::{Pipeline, Step};
use super::super::rows::morsel_rows;
use super::super::{Held, Limit};
use crate::db::Database;
use crate::exec::{Db, Runner};
use crate::physical::{JoinKey, Parallel, Plan};
use crate::value::{Decimal, Value};

/// A scan of `columns` of table `ident` of schema A.
fn scan(ident: &str, columns: Vec<usize>) -> Plan {
    Plan::SeqScan {
        table: crate::catalog::QualifiedName { auth: "A".into(), ident: ident.into() },
        columns,
        rows: 0,
        cost: 0.0_f64,
        order: Vec::new(),
        backward: false,
        most: None,
        located: false,
    }
}

/// F joined with D on F's G and D's G, D read by its morsels on 4
/// threads.
fn joined() -> Plan {
    let right = Plan::Parallel(Box::new(Parallel { degree: 4, input: scan("D", vec![0, 1, 2]) }));
    let keys = vec![JoinKey { left: 1, right: 1, nulls: false }];
    Plan::HashJoin { left: Box::new(scan("F", vec![0, 1])), right: Box::new(right), keys, detail: String::new(), est: 0.0_f64 }
}

/// A database of D, 8000 rows of K, G one of 2000, null every 97th, and a
/// text of 120 characters, and F, 6000 rows of K and G one of 2500; in
/// morsels of 256 rows.
fn database() -> Database {
    database_of(|k| if k % 97 == 0 { "NULL".to_owned() } else { k.saturating_mul(7).rem_euclid(2_000).to_string() })
}

/// As `database`, D's G of each row's K as `key` gives it.
fn database_of(key: fn(i64) -> String) -> Database {
    let db = Database::new();
    db.set_setting("EXECUTOR.MORSEL_ROWS", &Value::Exact(Decimal::new(256, 0))).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION A
           CREATE TABLE D (K INTEGER, G INTEGER, T VARCHAR(200))
           CREATE TABLE F (K INTEGER, G INTEGER)",
    )
    .unwrap();
    let a = db.session("A");
    for from in (0_i64..8_000).step_by(500) {
        let d: Vec<String> =
            (from..from.saturating_add(500)).map(|k| format!("({k}, {}, '{}')", key(k), format!("{k:08}").repeat(15))).collect();
        a.run(&format!("INSERT INTO D VALUES {}", d.join(", "))).unwrap();
    }
    let f: Vec<String> = (0_i64..6_000).map(|k| format!("({k}, {})", k.saturating_mul(3).rem_euclid(2_500))).collect();
    a.run(&format!("INSERT INTO F VALUES {}", f.join(", "))).unwrap();
    a.commit().unwrap();
    db
}

/// The rows of every morsel of `pipeline`, as text, sorted.
fn read(pipeline: Pipeline, state: &Db) -> Vec<String> {
    let (count, pipeline) = (pipeline.count(), Arc::new(pipeline));
    let mut out = Vec::new();
    for n in 0..count {
        for row in morsel_rows(&pipeline, n, Runner::new(state, crate::txn::NO_TRANSACTION)).unwrap() {
            out.push(format!("{:?}", row.unwrap()));
        }
    }
    out.sort();
    out
}

/// The pipeline of `joined` in `db` at a grant of `bytes`, by batch if
/// `batches`: its passes and rows, or none if it gave way; and the bytes
/// the budget held besides, once it is read and let go.
fn run(db: &Database, bytes: i128, batches: bool) -> (Option<(usize, Vec<String>)>, usize) {
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(bytes, 0))).unwrap();
    db.set_setting("EXECUTOR.BATCHES", &Value::Char(if batches { "ON" } else { "OFF" }.into())).unwrap();
    let state = Db::from(db.snapshot().unwrap());
    let before = state.work.budget().reserved();
    let limit = Limit::of(&state);
    let made = match Pipeline::of(&joined(), (&state, crate::txn::NO_TRANSACTION), &limit).unwrap() {
        Held::Kept(pipeline) => {
            let passes = pipeline.passes.as_ref().map_or(1, |p| p.count());
            Some((passes, read(pipeline, &state)))
        }
        Held::GaveWay => None,
    };
    drop(limit);
    (made, state.work.budget().reserved().saturating_sub(before))
}

/// Verifies: [DESIGN] RULES.md D1240: a hash join run in parallel whose
/// built operand outgrows the grant is built in parts and probed in
/// passes, its rows those of the table built whole, by batch and row by
/// row; and of a grant so small its passes would be more than the most
/// the probing side's cost allows, it gives way while its operand is
/// read, nothing left held of the budget, the spill areas of its parts
/// let go.
#[test]
fn a_build_past_the_grant_is_probed_in_passes_or_gives_way() {
    let db = database();
    for batches in [true, false] {
        let (whole, _) = run(&db, 1 << 30, batches);
        let (one, rows) = whole.unwrap();
        assert_eq!(one, 1, "within the grant, one table");
        assert!(rows.len() > 10_000, "{}", rows.len());
        let (passed, _) = run(&db, 1 << 20, batches);
        let (count, given) = passed.unwrap();
        assert!((2..=16).contains(&count), "{count} passes");
        assert_eq!(given, rows, "of batches {batches}");
        let (gave_way, left) = run(&db, 64 << 10, batches);
        assert!(gave_way.is_none(), "too many passes");
        assert_eq!(left, 0, "nothing held");
    }
}

/// The bounds of the runtime filter of the first join key of the table
/// the probe of `pipeline`'s first branch reads, as text.
fn bounds(pipeline: &Pipeline) -> Option<(String, String)> {
    let table = pipeline.branches.first()?.steps.iter().find_map(|s| if let Step::Probe(t) = s { Some(Arc::clone(t)) } else { None })?;
    let told = table.shared.filters(table.keys()).ok()?;
    told.first().and_then(|t| t.filter.bounds()).map(|(a, b)| (format!("{a:?}"), format!("{b:?}")))
}

/// The pipeline of `joined` in `db` at a grant of `bytes`, kept.
fn kept(db: &Database, bytes: i128) -> Pipeline {
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(bytes, 0))).unwrap();
    let state = Db::from(db.snapshot().unwrap());
    let limit = Limit::of(&state);
    let Held::Kept(pipeline) = Pipeline::of(&joined(), (&state, crate::txn::NO_TRANSACTION), &limit).unwrap() else {
        panic!("the pipeline gave way")
    };
    pipeline
}

/// Verifies: [DESIGN] RULES.md D1240: the first pass's table, whose
/// runtime filters the probing side's scans are told, carries the filters
/// of the whole operand, not of its own rows alone: of keys 1 to 8, each
/// pass holding one or two, the bounds of the table built whole.
#[test]
fn the_first_pass_carries_the_whole_operands_filters() {
    let db = database_of(|k| k.rem_euclid(8).saturating_add(1).to_string());
    let whole = kept(&db, 1 << 30);
    assert!(whole.passes.is_none() && bounds(&whole).is_some());
    let passed = kept(&db, 1 << 20);
    assert!(passed.passes.as_ref().is_some_and(|p| p.count() > 2), "built in passes");
    assert_eq!(bounds(&passed), bounds(&whole));
}

/// Verifies: [DESIGN] RULES.md D1240, D1241: a later pass's pipeline
/// leaves late the columns the first pass's leaves, and leaves them so
/// as the first's does: of the pipeline told the keys of a sort of its
/// first rows that decodes them, the columns no step and no key reads, not
/// every column no step reads, which its own steps would leave.
#[test]
fn a_later_pass_leaves_late_what_the_first_leaves() {
    let db = database();
    db.set_setting("EXECUTOR.BATCHES", &Value::Char("ON".into())).unwrap();
    let topped = kept(&db, 1 << 20).topped((&[0], true), &None);
    let passes = topped.passes.as_ref().map_or(1, |p| p.count());
    assert!(passes > 1, "built in passes");
    let base = topped.count().checked_div(passes).unwrap();
    let first = topped.batched.as_ref().unwrap().late_given();
    assert!(first.1, "left late for the sort");
    let Some((pass, _)) = super::passed(&topped, base).unwrap() else { panic!("a morsel of a later pass") };
    assert_eq!(pass.batched.as_ref().unwrap().late_given(), first);
}
