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

/// Verifies: [DESIGN] RULES.md D31, a column gathered by row, as a join
/// pairs a batch's rows: each kind gathered as one of its kind, a row taken
/// again as often as picked, in the order picked, a null where none is
/// picked or the row's is null; texts of a store's dictionaries keep their
/// entries; a vector of nulls is of values.
#[test]
fn columns_gathered_by_row_keep_their_kind() {
    let picks = [Some(2_usize), None, Some(0_usize), Some(2_usize)];
    let ints = Column::Ints(Ints::new(vec![150_i64, -3_i64, 7_i64], 2_u32, Some(vec![false, true, false])));
    let Column::Ints(gathered) = ints.gathered(&picks) else { panic!("integers") };
    assert_eq!(shown(gathered.values()), ["0.07", "NULL", "1.50", "0.07"]);
    let Column::Ints(whole) = ints.gathered(&[Some(0_usize), Some(2_usize)]) else { panic!("integers") };
    assert!(whole.nulls.is_none(), "no row null, no nulls kept");
    let Column::Floats(floats) = Column::Floats(Floats::new(vec![1.5_f64, 2.5_f64, -1.0_f64], None)).gathered(&picks) else {
        panic!("numbers")
    };
    assert_eq!(shown(floats.values()), ["-1E0", "NULL", "1.5E0", "-1E0"]);
    let texts = Texts::new("abcde".to_owned(), vec![1_usize, 3_usize, 5_usize], Some(vec![false, false, true]));
    let Column::Texts(plain) = Column::Texts(texts).gathered(&[Some(1_usize), None, Some(2_usize), Some(0_usize)]) else { panic!("texts") };
    assert_eq!((plain.get(0_usize), plain.get(1_usize), plain.get(2_usize), plain.get(3_usize)), (Some("bc"), None, None, Some("a")));
    let entries = Texts::new("xy".to_owned(), vec![1_usize, 2_usize], None);
    let coded = Column::Texts(Texts::coded(Coded { codes: vec![1_u32, 0_u32, 1_u32], entries }, None));
    let Column::Texts(recoded) = coded.gathered(&picks) else { panic!("texts") };
    assert!(recoded.coded.is_some(), "entries kept");
    assert_eq!(shown(recoded.values()), ["y", "NULL", "y", "y"]);
    let values = Column::from(vec![Value::Char("v".into()), Value::Null, exact(5_i128, 0_u32)]);
    assert_eq!(shown(values.gathered(&picks).values()), ["5", "NULL", "v", "5"]);
    let Column::Values(nulls) = Column::nulls(2_usize) else { panic!("values") };
    assert!(nulls.len() == 2_usize && nulls.iter().all(Value::is_null));
}
