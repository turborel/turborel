//! Unit tests of a batch's columns (`batch.rs`).

use super::*;

fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// `values` as they are written.
fn shown(values: &[Value]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

/// Verifies: [DESIGN] RULES.md D31, a batch's columns: integers give the
/// values of their units at their scale, nulls as nulls, made once; their
/// cells are read in place, a value's borrowed, an integer's its units,
/// past the end or of a column not there a null.
#[test]
fn columns_of_values_and_of_integers() {
    let ints = Ints::new(vec![150, -3, 0], 2, Some(vec![false, false, true]));
    assert_eq!(shown(ints.values()), ["1.50", "-0.03", "NULL"]);
    assert!(ints.is_null(2) && !ints.is_null(0) && !ints.is_null(9), "past the end, not null");
    let batch =
        Batch { columns: vec![Column::Ints(ints.clone()), Column::from(vec![Value::Char("a".into()), Value::Null, Value::Null])], rows: 3 };
    assert!(matches!(batch.cell(0, 0), Cell::Exact(150, 2)));
    assert!(matches!(batch.cell(0, 2), Cell::Null) && batch.cell(0, 2).is_null());
    assert!(matches!(batch.cell(1, 0), Cell::Value(Value::Char(_))) && !batch.cell(1, 0).is_null());
    assert!(batch.cell(1, 1).is_null(), "a value that is null");
    assert!(matches!(batch.cell(0, 7), Cell::Null) && matches!(batch.cell(5, 0), Cell::Null), "past the end, no column");
}

/// Verifies: [DESIGN] RULES.md D31, a batch's cells' values: an integer's
/// made of its units and scale, a value's cloned, a null's null; a value
/// of a vector of integers, and none of a column not there.
#[test]
fn cells_give_their_values() {
    let ints = Ints::new(vec![150, -3, 0], 2, Some(vec![false, false, true]));
    let batch = Batch { columns: vec![Column::Ints(ints), Column::from(vec![Value::Char("a".into()), Value::Null, Value::Null])], rows: 3 };
    assert_eq!(batch.cell(0, 1).to_value().to_string(), "-0.03");
    assert_eq!(batch.cell(1, 0).to_value().to_string(), "a");
    assert!(batch.cell(0, 2).to_value().is_null());
    assert_eq!(batch.value(0, 1).map(ToString::to_string), Some("-0.03".to_owned()));
    assert!(batch.values(9).is_empty(), "no column");
}

/// Verifies: [DESIGN] RULES.md D31, a batch's columns changed: a column of
/// integers changed becomes one of values, and a row pushed to a batch of
/// them is added to those values; a column's values owned are its own.
#[test]
fn columns_of_integers_changed_become_of_values() {
    let ints = Ints::new(vec![150, -3, 0], 2, Some(vec![false, false, true]));
    let mut batch =
        Batch { columns: vec![Column::Ints(ints.clone()), Column::from(vec![Value::Char("a".into()), Value::Null, Value::Null])], rows: 3 };
    batch.push(&vec![exact(7, 0), Value::Char("b".into())], &[0, 1]);
    assert!(matches!(batch.columns.first(), Some(Column::Values(_))), "of values once changed");
    assert_eq!(shown(batch.values(0)), ["1.50", "-0.03", "NULL", "7"]);
    assert_eq!(batch.rows, 4);
    assert_eq!(shown(&Column::Ints(ints).into_values()), ["1.50", "-0.03", "NULL"]);
    assert!(Column::default().into_values().is_empty());
    assert!(Ints::default().values().is_empty());
}

/// Verifies: [DESIGN] RULES.md D31, a batch's approximate numbers: their
/// values, nulls as nulls, made once; their cells their numbers, read in
/// place; changed, a column of them becomes one of values; owned, their
/// values are their own.
#[test]
fn columns_of_approximate_numbers() {
    let floats = Floats::new(vec![1.5_f64, -0.0_f64, 0.0_f64], Some(vec![false, false, true]));
    assert_eq!(shown(floats.values()), ["1.5E0", "-0E0", "NULL"]);
    assert_eq!((floats.get(0), floats.get(2), floats.get(9)), (Some(1.5_f64), None, None));
    let mut batch = Batch { columns: vec![Column::Floats(floats.clone())], rows: 3 };
    assert!(matches!(batch.cell(0, 0), Cell::Approx(f) if f.to_bits() == 1.5_f64.to_bits()) && !batch.cell(0, 0).is_null());
    assert!(batch.cell(0, 2).is_null());
    assert_eq!(batch.cell(0, 1).to_value().to_string(), "-0E0");
    batch.push(&vec![Value::Approx(2.0_f64)], &[0]);
    assert!(matches!(batch.columns.first(), Some(Column::Values(_))), "of values once changed");
    assert_eq!(shown(batch.values(0)), ["1.5E0", "-0E0", "NULL", "2E0"]);
    assert_eq!(shown(&Column::Floats(floats).into_values()), ["1.5E0", "-0E0", "NULL"]);
    assert!(Floats::default().values().is_empty());
}
