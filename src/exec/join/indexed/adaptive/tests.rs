//! Adaptive joins choose scans only with useful estimates and affordable
//! bounded input; every fallback keeps all previously consumed rows.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::catalog::QualifiedName;
use crate::db::Database;
use crate::exec::execute;
use crate::functions::ScalarRef;
use crate::geometry::{Geometry, GeometryFunction, GeometryType};
use crate::index::{Entries, Index, Key, KeyOrder, KeyProjection, Predicate, Probe};
use crate::logical::{Cond, Expr};
use crate::memory::MemoryBudget;
use crate::physical::{JoinProbe, Plan};
use crate::storage::{Row, RowId};
use crate::txn::NO_TRANSACTION;
use crate::value::{CompOp, Decimal, Value};

struct Counted {
    inner: Arc<dyn Index>,
    searches: Arc<AtomicUsize>,
    estimates: bool,
    estimates_made: Arc<AtomicUsize>,
}

impl Index for Counted {
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.inner.insert(key, row)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.inner.remove(key, row)
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        self.inner.rows(key)
    }
    fn key_projection(&self) -> Option<Arc<dyn KeyProjection>> {
        self.inner.key_projection()
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        self.estimates_made.fetch_add(1, Ordering::Relaxed);
        self.estimates.then(|| self.inner.estimate(probe)).flatten()
    }
    fn search(self: Arc<Self>, probe: &Probe, order: KeyOrder) -> Entries {
        self.searches.fetch_add(1, Ordering::Relaxed);
        Arc::clone(&self.inner).search(probe, order)
    }
}

fn number(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

fn populated() -> Arc<State> {
    populated_rows(100)
}

fn populated_rows(count: i32) -> Arc<State> {
    let db = Database::new();
    db.register_type(Box::new(GeometryType)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE POINTS (ID INTEGER, G GEOMETRY(0))").unwrap();
    let a = db.session("A");
    let insert = a.prepare("INSERT INTO POINTS VALUES (?, ?)").unwrap();
    for n in 0_i32..count {
        a.execute_prepared(&insert, &[number(i128::from(n)), Geometry::point(f64::from(n), f64::from(n), 0).unwrap().to_value()]).unwrap();
    }
    a.commit().unwrap();
    db.create_index("POINTS_G", "A.POINTS", &["G"], "rtree").unwrap();
    db.snapshot().unwrap()
}

fn plan(regions: Vec<Row>, argument: usize) -> Plan {
    let width = regions.first().map_or(2, Vec::len);
    let function = ScalarRef(Arc::new(GeometryFunction::Intersects));
    let args = if argument == 0 {
        vec![Expr::Column(width.saturating_add(1)), Expr::Column(1)]
    } else {
        vec![Expr::Column(1), Expr::Column(width.saturating_add(1))]
    };
    Plan::IndexJoin {
        outer: false,
        spatial: true,
        left: Box::new(Plan::Values {
            rows: regions.into_iter().map(|row| row.into_iter().map(Expr::Const).collect()).collect(),
            detail: String::new(),
        }),
        table: QualifiedName { auth: "A".into(), ident: "POINTS".into() },
        index: "POINTS_G".into(),
        method: "rtree".into(),
        probe: JoinProbe::Predicate { predicate: Box::new(Predicate { function: function.clone(), column: 0, argument }), outer: 1 },
        cond: Some(Cond::Compare(Expr::Call(function, args), CompOp::Eq, Expr::Const(number(1)))),
        width: 2,
        columns: vec![0, 1],
        detail: String::new(),
        est: 100.0,
        cost: 1000.0,
        located: false,
    }
}

fn run(state: &Arc<State>, plan: Plan, (budget, estimates): (usize, bool)) -> (usize, Vec<Row>) {
    let (searches, _, rows) = measured(state, plan, (budget, estimates));
    (searches, rows)
}

fn measured(state: &Arc<State>, plan: Plan, (budget, estimates): (usize, bool)) -> (usize, usize, Vec<Row>) {
    let mut state = (**state).clone();
    let searches = Arc::new(AtomicUsize::new(0));
    let estimates_made = Arc::new(AtomicUsize::new(0));
    let inner = Arc::clone(&state.indexes["POINTS_G"]);
    state.indexes.insert(
        "POINTS_G".into(),
        Arc::new(Counted { inner, searches: Arc::clone(&searches), estimates, estimates_made: Arc::clone(&estimates_made) }),
    );
    state.memory = MemoryBudget::new(budget);
    let memory = Arc::clone(&state.memory);
    let rows = execute(plan, &Arc::new(state), NO_TRANSACTION).collect::<Result<Vec<_>>>().unwrap();
    assert_eq!(memory.reserved(), 0);
    (searches.load(Ordering::Relaxed), estimates_made.load(Ordering::Relaxed), rows)
}

/// Verifies: [DESIGN] D82: selective probes need no extra cardinality
/// estimates. A broad probe later in the input still triggers adaptation,
/// without repeating or losing any already-emitted outer row's pairs.
#[test]
fn observed_probes_stay_lazy_and_adapt_after_a_selective_prefix() {
    let state = populated_rows(10_000);
    let narrow = Geometry::point(7.0, 7.0, 0).unwrap().to_value();
    let regions = vec![vec![number(1), narrow.clone()], vec![number(2), narrow.clone()], vec![number(3), narrow.clone()]];
    let (searches, estimates, rows) = measured(&state, plan(regions, 0), (usize::MAX, true));
    assert_eq!((searches, estimates, rows.len()), (3, 0, 3));
    let broad = Geometry::rectangle([-1.0_f64, -1.0_f64], [10_000.0_f64, 10_000.0_f64], 0).unwrap().to_value();
    let changed = vec![vec![number(1), narrow], vec![number(2), broad.clone()], vec![number(3), broad]];
    let (later_searches, later_estimates, later_rows) = measured(&state, plan(changed, 0), (usize::MAX, true));
    assert_eq!((later_searches, later_estimates), (2, 1));
    let mut pairs: Vec<_> = later_rows.iter().map(|row| (row[0].to_string(), row[2].to_string())).collect();
    let mut expected = vec![("1".to_owned(), "7".to_owned())];
    expected
        .extend([2_i32, 3_i32].into_iter().flat_map(|outer| (0_i32..10_000_i32).map(move |inner| (outer.to_string(), inner.to_string()))));
    pairs.sort();
    expected.sort();
    assert_eq!(pairs, expected);
}

/// Verifies: [DESIGN] D82: setup is amortized over observed candidate
/// work, without extra estimates for a wholly selective join. Several
/// moderate probes can jointly justify adaptation; no one broad probe
/// is required, and their already-emitted pairs are not replayed.
#[test]
fn completed_probes_accumulate_before_pricing_the_remaining_input() {
    let state = populated_rows(10_000);
    let narrow = Geometry::rectangle([0.0_f64, 0.0_f64], [99.0_f64, 99.0_f64], 0).unwrap().to_value();
    let selective = (0_i32..40_i32).map(|n| vec![number(i128::from(n)), narrow.clone()]).collect();
    let (searches, estimates, rows) = measured(&state, plan(selective, 0), (usize::MAX, true));
    assert_eq!((searches, estimates, rows.len()), (40, 0, 4_000));
    let moderate = Geometry::rectangle([0.0_f64, 0.0_f64], [2499.0_f64, 2499.0_f64], 0).unwrap().to_value();
    let repeated = (0_i32..6_i32).map(|n| vec![number(i128::from(n)), moderate.clone()]).collect();
    let (repeated_searches, repeated_estimates, repeated_rows) = measured(&state, plan(repeated, 0), (usize::MAX, true));
    assert_eq!((repeated_searches, repeated_estimates, repeated_rows.len()), (4, 2, 15_000));
    let mut actual: Vec<_> = repeated_rows.iter().map(|row| (row[0].to_string(), row[2].to_string())).collect();
    let mut expected: Vec<_> =
        (0_i32..6_i32).flat_map(|outer| (0_i32..2500_i32).map(move |inner| (outer.to_string(), inner.to_string()))).collect();
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
}

/// Verifies: [DESIGN] D82: after one broad probe, spatial joins scan the
/// remaining outer input in either
/// argument order, while selective probes, unavailable estimates and
/// refused memory retain index access. The result pairs are unchanged.
/// Disabling the adaptive choice alone fails the access-count assertion.
#[test]
fn broad_joins_scan_and_selective_or_unaffordable_joins_probe() {
    let state = populated();
    let broad = Geometry::rectangle([-1.0_f64, -1.0_f64], [100.0_f64, 100.0_f64], 0).unwrap().to_value();
    let narrow = Geometry::point(7.0, 7.0, 0).unwrap().to_value();
    for argument in [0_usize, 1] {
        let regions = vec![vec![number(10), broad.clone()], vec![number(20), broad.clone()]];
        for (budget, estimates, searches) in [(usize::MAX, true, 1_usize), (0, true, 2), (usize::MAX, false, 2)] {
            let (actual, rows) = run(&state, plan(regions.clone(), argument), (budget, estimates));
            assert_eq!(actual, searches);
            let mut pairs: Vec<_> = rows.iter().map(|row| (row[0].to_string(), row[2].to_string())).collect();
            pairs.sort();
            let mut expected: Vec<_> = [10_i32, 20_i32]
                .into_iter()
                .flat_map(|outer| (0_i32..100_i32).map(move |inner| (outer.to_string(), inner.to_string())))
                .collect();
            expected.sort();
            assert_eq!(pairs, expected);
        }
        let (searches, rows) = run(&state, plan(vec![vec![number(10), narrow.clone()]], argument), (usize::MAX, true));
        assert_eq!((searches, rows.len()), (1, 1));
    }
    let (searches, rows) = run(&state, plan(Vec::new(), 0), (usize::MAX, true));
    assert_eq!((searches, rows.len()), (0, 0));
}

/// Verifies: [DESIGN] D38, D82: exceeding the build memory limit or running
/// out of memory after some rows were buffered resumes every row exactly
/// once. Reservations are released after either execution path is dropped.
#[test]
fn adaptive_fallback_keeps_the_buffer_and_first_overflow_row() {
    let state = populated();
    let point = Geometry::point(7.0, 7.0, 0).unwrap().to_value();
    let regions: Vec<_> = (0..66_i128).map(|n| vec![number(n), point.clone()]).collect();
    let first = Held::new(&state).unwrap();
    let initial = first.reservation.bytes();
    drop(first);
    let budget = initial.saturating_add(crate::spill::row_bytes(&regions[0]).saturating_mul(64));
    let (searches, rows) = run(&state, plan(regions, 0), (budget, true));
    assert_eq!((searches, rows.len()), (66, 66));
    let partial = vec![vec![number(1), point.clone()], vec![number(2), point.clone()], vec![number(3), point]];
    let held = Held::new(&state).unwrap();
    let partial_budget = held.reservation.bytes().saturating_add(crate::spill::row_bytes(&partial[0]));
    drop(held);
    let (partial_searches, partial_rows) = run(&state, plan(partial, 0), (partial_budget, true));
    assert_eq!((partial_searches, partial_rows.len()), (3, 3));
    assert_eq!(partial_rows.iter().map(|row| row[0].to_string()).collect::<Vec<_>>(), ["1", "2", "3"]);
}

/// Verifies: [DESIGN] D82: a broad build larger than one mask word can
/// scan once, with each original outer row's full set of matches. The
/// memory-bounded hierarchy preserves duplicates across every group.
#[test]
fn adaptive_spatial_builds_extend_beyond_one_mask_word() {
    let state = populated();
    let broad = Geometry::rectangle([-1.0_f64, -1.0_f64], [100.0_f64, 100.0_f64], 0).unwrap().to_value();
    let regions: Vec<_> = (0..257_i128).map(|n| vec![number(n), broad.clone()]).collect();
    let (searches, rows) = run(&state, plan(regions, 0), (usize::MAX, true));
    assert_eq!(searches, 1);
    let mut actual: Vec<_> = rows.iter().map(|row| (row[0].to_string(), row[2].to_string())).collect();
    let mut expected: Vec<_> =
        (0..257_i128).flat_map(|outer| (0..100_i128).map(move |inner| (outer.to_string(), inner.to_string()))).collect();
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
}

/// Verifies: [DESIGN] D38: build rows are charged before retention. A
/// failed growth stops reading immediately, preserving the overflow row
/// for fallback, and dropping the buffer releases its reservation.
#[test]
fn adaptive_build_stops_when_its_reservation_cannot_grow() {
    let mut state = (*populated()).clone();
    state.memory = MemoryBudget::new(usize::MAX);
    let state = Arc::new(state);
    let mut held = Held::new(&state).unwrap();
    let row = vec![number(1), Value::Char("a retained payload".into())];
    let allowed = held.reservation.bytes().saturating_add(crate::spill::row_bytes(&row));
    state.memory.set_limit(allowed);
    let read = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&read);
    let mut input: RowIter = Box::new(vec![row.clone(); 3].into_iter().map(move |row| {
        counted.fetch_add(1, Ordering::Relaxed);
        Ok(row)
    }));
    assert!(!held.read(&mut input).unwrap());
    assert_eq!(held.rows.len(), 1);
    assert_eq!(read.load(Ordering::Relaxed), 2);
    assert_eq!(state.memory.reserved(), allowed);
    drop(held);
    assert_eq!(state.memory.reserved(), 0);
    assert!(input.next().is_some());
    assert!(input.next().is_none());
}

/// Verifies: [DESIGN] D31, D82: projecting inside either spatial join
/// path preserves every selected value, including geometry values,
/// reordered duplicates and zero-column rows. Memory refusal and missing
/// estimates retain that projection when they resume ordinary probes.
#[test]
fn spatial_join_projections_survive_scan_and_probe_fallbacks() {
    let state = populated();
    let broad = Geometry::rectangle([-1.0_f64, -1.0_f64], [100.0_f64, 100.0_f64], 0).unwrap().to_value();
    let regions = vec![vec![number(10), broad.clone()], vec![number(20), broad], vec![number(30), Value::Null]];
    for argument in [0_usize, 1] {
        for (budget, estimates) in [(usize::MAX, true), (0, true), (usize::MAX, false)] {
            for columns in [vec![], vec![2, 0], vec![3, 1, 3, 0, 2]] {
                let (_, original) = run(&state, plan(regions.clone(), argument), (budget, estimates));
                let mut expected: Vec<_> =
                    original.iter().map(|row| columns.iter().map(|&at| row[at].to_string()).collect::<Vec<_>>()).collect();
                let projection = Plan::Project {
                    exprs: columns.into_iter().map(Expr::Column).collect(),
                    input: Box::new(plan(regions.clone(), argument)),
                    detail: String::new(),
                };
                let (_, actual) = run(&state, projection, (budget, estimates));
                let mut actual: Vec<_> = actual.iter().map(|row| row.iter().map(ToString::to_string).collect::<Vec<_>>()).collect();
                actual.sort();
                expected.sort();
                assert_eq!(actual, expected);
            }
        }
    }
}
