//! Unit tests of the rows a scan holds and the judged columns held of
//! them (`pending.rs`).

use super::*;
use crate::value::{Decimal, Value};

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// A vector of integers of scale 0, null where `nulls` says.
fn ints(units: &[i64], nulls: &[usize]) -> Column {
    let mut mask = Mask::none(units.len());
    for &n in nulls {
        mask.set(n);
    }
    Column::Ints(Ints::new(units.to_vec(), 0, (!nulls.is_empty()).then_some(mask)))
}

/// Verifies: [DESIGN] RULES.md D366 a judged column held of the rows kept
/// of several runs gives each row's value, nulls among them, in the order
/// of the places held, from the first run on.
#[test]
fn judged_columns_are_held_of_the_rows_kept() {
    let mut pending = Pending::default();
    let first = [(1, ints(&[10, 11, 12, 13], &[2]))];
    pending.add(&[100, 102], (&[0, 2], &first));
    let second = [(1, ints(&[20, 21, 22], &[]))];
    pending.add(&[201], (&[1], &second));
    let third = [(1, ints(&[30, 31], &[0]))];
    pending.add(&[300, 301], (&[0, 1], &third));
    let (places, held) = pending.take();
    assert_eq!(places, [100, 102, 201, 300, 301]);
    let [(column, made)] = held.as_slice() else { panic!("one column held: {held:?}") };
    assert_eq!(*column, 1);
    let want = [int(10), Value::Null, int(21), Value::Null, int(31)];
    assert_eq!(format!("{:?}", made.values()), format!("{want:?}"), "each row's value, nulls kept");
    assert!(pending.is_empty(), "taken");
}
