//! Cartesian method requirements: independent coordinates, exact SQL
//! comparisons, format validation, counted versions and cursor lifetime.

use super::*;
use crate::index::ColumnComparison;
use crate::value::{Decimal, Spelled};

mod geometry;
mod nearest;

fn approximate() -> DataType {
    DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision }
}
fn columns(width: usize) -> Vec<DataType> {
    vec![approximate(); width]
}
fn key(coordinates: &[f64]) -> Key {
    Key(coordinates.iter().copied().map(Value::Approx).collect())
}
fn constraint(column: usize, op: CompOp, value: Value) -> ColumnComparison {
    ColumnComparison { column, op, value }
}
fn ids(index: &Arc<dyn Index>, probe: &Probe) -> Vec<RowId> {
    let mut ids: Vec<_> = Arc::clone(index).search(probe, KeyOrder::Ascending).map(|e| e.unwrap().1).collect();
    ids.sort_unstable();
    ids
}

/// Verifies: [DESIGN] Every supported independent-coordinate comparison
/// returns precisely the keys satisfying SQL comparison, even when
/// rectangle constraints cross (min X <= 3, max X >= 7). Boundary points
/// and equal min/max coordinates are retained. Each axis is independent.
#[test]
fn independent_comparisons_match_sql_for_points_and_rectangles() {
    for width in [2, 4] {
        let method = RTreeMethod::in_memory();
        let index: Arc<dyn Index> = Arc::from(method.create(&columns(width)).unwrap());
        let keys: Vec<_> = (0_i32..200_i32)
            .map(|n| {
                let (x, y) = (f64::from(n % 17_i32), f64::from(n % 23_i32));
                let values = [x, y, x + 10.0_f64, y + 10.0_f64];
                key(&values[..width])
            })
            .collect();
        for (id, k) in (0..).zip(&keys) {
            index.insert(k.clone(), id).unwrap();
        }
        for at in 0..width {
            for op in [CompOp::Eq, CompOp::Gt, CompOp::Ge, CompOp::Lt, CompOp::Le] {
                for value in [-1.0_f64, 0.0_f64, 3.0_f64, 7.0_f64, 30.0_f64] {
                    let other = (at + 1) % width;
                    let each = vec![constraint(at, op, Value::Approx(value)), constraint(other, CompOp::Ge, Value::Approx(7.0_f64))];
                    assert!(method.supports_columns(width, &each.iter().map(|c| (c.column, c.op)).collect::<Vec<_>>()));
                    let probe = Probe::Columns(each);
                    let expected: Vec<_> = (0..).zip(&keys).filter(|(_, k)| probe.answers(&k.0)).map(|(id, _)| id).collect();
                    assert_eq!(ids(&index, &probe), expected, "{width} {probe:?}");
                }
            }
        }
        let spanning = Probe::Columns(vec![
            constraint(0, CompOp::Le, Value::Approx(3.0_f64)),
            constraint(width - 2, CompOp::Ge, Value::Approx(7.0_f64)),
        ]);
        let expected: Vec<_> = (0..).zip(&keys).filter(|(_, k)| spanning.answers(&k.0)).map(|(id, _)| id).collect();
        assert_eq!(ids(&index, &spanning), expected);
    }
}

/// Verifies: [DESIGN] An exact decimal between adjacent floating-point
/// coordinates is compared by SQL's exact numeric rules, never rounded to
/// the stored coordinate. NULL and missing-column comparisons match none;
/// no comparisons impose no restriction, even on an empty key.
#[test]
fn exact_numeric_null_and_missing_column_probes_are_not_rounded() {
    let method = RTreeMethod::in_memory();
    let index: Arc<dyn Index> = Arc::from(method.create(&columns(2)).unwrap());
    let values = [0.1_f64.next_down(), 0.1_f64, 0.1_f64.next_up()];
    for (id, x) in (0..).zip(values) {
        index.insert(key(&[x, 0.0_f64]), id).unwrap();
    }
    let decimal = Value::Exact(Decimal::new(1, 1));
    for op in [CompOp::Eq, CompOp::Lt, CompOp::Le, CompOp::Gt, CompOp::Ge] {
        let probe = Probe::Columns(vec![constraint(0, op, decimal.clone())]);
        let expected: Vec<_> = (0..).zip(values).filter(|(_, x)| probe.answers(&key(&[*x, 0.0_f64]).0)).map(|(id, _)| id).collect();
        assert_eq!(ids(&index, &probe), expected);
    }
    for (column, value) in [(0, Value::Null), (2, Value::Approx(0.0_f64)), (0, Value::Char("x".into()))] {
        let probe = Probe::Columns(vec![constraint(column, CompOp::Eq, value)]);
        assert_eq!(ids(&index, &probe), []);
        assert!(!probe.answers(&key(&[0.0_f64, 0.0_f64]).0));
    }
    assert!(Probe::Columns(Vec::new()).answers(&Vec::<Value>::new()));
}

/// Verifies: [DESIGN] Unsupported widths, types and malformed coordinate
/// keys are refused without adding a reference. Reversed and nonfinite
/// rectangles retain the bounds API's SQLCODE -910; unsupported key shapes
/// and column types are SQLCODE -900.
#[test]
fn unsupported_columns_and_invalid_keys_leave_no_entries() {
    let method = RTreeMethod::in_memory();
    for width in [0, 1, 3, 5] {
        assert_eq!(method.create(&columns(width)).err().unwrap().code, Code::Unsupported);
    }
    let exact = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
    assert_eq!(method.create(&[exact, approximate()]).err().unwrap().code, Code::Unsupported);
    let index: Arc<dyn Index> = Arc::from(method.create(&columns(4)).unwrap());
    for coordinates in [[1.0_f64, 0.0_f64, 0.0_f64, 1.0_f64], [0.0_f64, 0.0_f64, f64::INFINITY, 1.0_f64]] {
        assert_eq!(index.insert(key(&coordinates), 0).unwrap_err().code, Code::Api);
    }
    for invalid in [Key(vec![Value::Null; 4]), key(&[1.0_f64, 2.0_f64])] {
        assert_eq!(index.insert(invalid, 0).unwrap_err().code, Code::Unsupported);
    }
    assert_eq!(ids(&index, &Probe::All), []);
    assert!(!method.supports_columns(2, &[]));
    assert!(!method.supports_columns(2, &[(2, CompOp::Eq)]));
    assert!(!method.supports_columns(2, &[(0, CompOp::Ne)]));
}

/// Verifies: [DESIGN] A checkpointed index reopens with its original
/// coordinates and reference counts. Locators reject unsupported versions,
/// a changed key width, trailing bytes and truncation with SQLCODE -916.
#[test]
fn method_locators_preserve_counts_and_refuse_incompatible_formats() {
    let method = RTreeMethod::in_memory();
    let index: Arc<dyn Index> = Arc::from(method.create(&columns(2)).unwrap());
    index.insert(key(&[2.0_f64, 3.0_f64]), 7).unwrap();
    index.insert(key(&[2.0_f64, 3.0_f64]), 7).unwrap();
    let locator = index.locator();
    method.checkpoint().unwrap();
    drop(index);
    let opened: Arc<dyn Index> = Arc::from(method.open(&columns(2), &locator).unwrap().unwrap());
    opened.remove(&key(&[2.0_f64, 3.0_f64]), 7).unwrap();
    assert_eq!(ids(&opened, &Probe::All), [7]);
    drop(opened);
    for length in 0..locator.len() {
        assert_eq!(method.open(&columns(2), &locator[..length]).err().unwrap().code, Code::Corrupt);
    }
    let mut version = locator.clone();
    version[0] += 1;
    assert_eq!(method.open(&columns(2), &version).err().unwrap().code, Code::Corrupt);
    assert_eq!(method.open(&columns(4), &locator).err().unwrap().code, Code::Corrupt);
    let mut trailing = locator;
    trailing.push(0);
    assert_eq!(method.open(&columns(2), &trailing).err().unwrap().code, Code::Corrupt);
}

/// Verifies: [DESIGN] A cursor holds a dropped index's pages until it
/// ends. Cancelling that cursor releases the last index reference and
/// frees its pages; a point-probe cursor has the same lifetime guarantee.
#[test]
fn discarded_index_pages_outlive_both_cursor_kinds() {
    for exact in [false, true] {
        let method = RTreeMethod::in_memory();
        let index: Arc<dyn Index> = Arc::from(method.create(&columns(2)).unwrap());
        index.insert(key(&[2.0_f64, 3.0_f64]), 7).unwrap();
        let mut cursor: RowIds = if exact {
            Arc::clone(&index).key_rows(&key(&[2.0_f64, 3.0_f64]))
        } else {
            Box::new(Arc::clone(&index).search(&Probe::All, KeyOrder::Ascending).map(|e| e.map(|e| e.1)))
        };
        let (pool, file) = method.files().unwrap();
        index.discard();
        drop(index);
        assert!(pool.pages_in_use(file).unwrap() > 0);
        assert_eq!(cursor.next().unwrap().unwrap(), 7);
        drop(cursor);
        assert_eq!(pool.pages_in_use(file).unwrap(), 0);
    }
}

/// Verifies: [DESIGN] Estimates stop after a bounded number of candidate
/// entries, including when every rectangle candidate fails the exact key
/// comparison. Planning cannot scan the whole index to establish zero.
#[test]
fn estimating_bounds_the_candidates_not_only_matching_results() {
    let method = RTreeMethod::in_memory();
    let index: Arc<dyn Index> = Arc::from(method.create(&columns(4)).unwrap());
    for row in 0..2_000 {
        index.insert(key(&[-10.0_f64, -10.0_f64, 10.0_f64, 10.0_f64]), row).unwrap();
    }
    let probe = Probe::Columns(vec![constraint(0, CompOp::Ge, Value::Approx(1.0_f64))]);
    assert_eq!(index.estimate(&probe), None);
    assert_eq!(ids(&index, &probe), []);
}

/// Verifies: [DESIGN] The numeric R-tree method does not opt into named
/// function lookups. Direct requests fail with -900 and no estimate,
/// rather than treating a function predicate as an unfiltered window.
#[test]
fn named_function_probes_are_explicitly_unsupported() {
    use crate::functions::Registry;
    use crate::index::Predicate;
    let method = RTreeMethod::in_memory();
    let index: Arc<dyn Index> = Arc::from(method.create(&columns(2)).unwrap());
    let function = Registry::with_set_functions().scalar("UPPER").unwrap();
    let predicate = Predicate { function, column: 0, argument: 0 };
    assert!(!method.supports_predicate(&predicate, &columns(2)));
    let probe = Probe::Predicate { predicate: Box::new(predicate), value: Value::Null };
    assert_eq!(index.estimate(&probe), None);
    let mut cursor = index.search(&probe, KeyOrder::Ascending);
    assert_eq!(cursor.next().unwrap().unwrap_err().code, Code::Unsupported);
    assert!(cursor.next().is_none());
}
