// Unit tests: a build split into parts, given way as its morsels project
// too many passes (RULES.md R-TEST-1, R-TEST-8).

use super::super::super::Limit;
use super::Spilling;
use crate::db::Database;
use crate::exec::Db;
use crate::physical::JoinKey;
use crate::storage::{Batch, Column, Ints};
use crate::value::{Decimal, Value};

/// A batch of `rows` rows of a key, its row's number from `from`, and the
/// number again.
fn batch(from: i64, rows: usize) -> Batch {
    let numbers: Vec<i64> = (from..).take(rows).collect();
    let columns = vec![Column::Ints(Ints::new(numbers.clone(), 0, None)), Column::Ints(Ints::new(numbers, 0, None))];
    Batch { columns, rows }
}

/// Verifies: [DESIGN] RULES.md D1240 (MC/DC): a build in parts goes on
/// while the passes its rows so far project of all its morsels are
/// within the most, and stops at the first morsel that projects more,
/// long before its last. Of 100 morsels, the first goes on at a most of
/// a billion passes and stops at a most of one.
#[test]
fn a_build_stops_once_its_morsels_project_too_many_passes() {
    let db = Database::new();
    db.set_setting("MEMORY.WORK_BYTES", &Value::Exact(Decimal::new(1 << 20, 0))).unwrap();
    let state = Db::from(db.snapshot().unwrap());
    let keys = [JoinKey { left: 0, right: 0, nulls: false }];
    let limit = Limit::of(&state);
    let added = |most: f64| {
        let mut spilling = Spilling::of(&state, (&keys, &[0, 1]), (&limit, 0), most).unwrap();
        spilling.add(&batch(0, 2_000), 0, (1, 100)).unwrap()
    };
    assert!(added(1e9_f64), "within many passes");
    assert!(!added(1.0_f64), "past one pass, at the first morsel of 100");
}
