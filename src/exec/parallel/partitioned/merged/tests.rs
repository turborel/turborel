use std::sync::Arc;

use super::*;
use crate::spill::testing::{Held, areas};
use crate::spill::{ListSpace, row_bytes};
use crate::value::types::TypeRegistry;
use crate::value::{Decimal, Value};

fn number(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// A row of key `key`, the `n`th of partition `part`.
fn row(key: u64, (part, n): (u64, u64)) -> Row {
    vec![number(key), Value::Char(format!("part {part} row {n}").into())]
}

/// A list holding about four rows before it spills.
fn space() -> ListSpace {
    ListSpace {
        limit: row_bytes(&row(0, (0, 0))).saturating_mul(4),
        batch: 4096,
        areas: Some(areas(Some(Arc::new(Held { broken: false })), usize::MAX)),
        types: TypeRegistry::default(),
    }
}

/// Verifies: [DESIGN] (RULES.md D1227) partitions' rows, each sorted by
/// its key, held in lists spilled past their share, are given merged by
/// the key as they are read back, a tie taken from the earlier partition
/// and within it in its order, as the rows of the lists held in memory
/// sorted stably.
#[test]
fn lists_spilled_are_merged_as_read_back() {
    let keys = [SortKey::ascending(0)];
    let mut sorted_parts: Vec<Vec<Row>> = (0_u64..3)
        .map(|p| (0_u64..40).map(|n| row(n.div_euclid(3).saturating_mul(p.saturating_add(1)).rem_euclid(50), (p, n))).collect())
        .collect();
    for rows in &mut sorted_parts {
        rows.sort_by(|a, b| compare_rows(&keys, a, b));
    }
    let mut expected: Vec<Row> = sorted_parts.concat();
    expected.sort_by(|a, b| compare_rows(&keys, a, b));
    let mut lists = Vec::new();
    for rows in sorted_parts {
        let mut list = HeldRows::new(space());
        for r in rows {
            list.push(r).unwrap();
        }
        assert!(list.spilled(), "each list past its share in the spill");
        lists.push(list);
    }
    let merged: Vec<Row> = Merged::of(lists, &keys).unwrap().collect::<Result<_>>().unwrap();
    assert_eq!(format!("{merged:?}"), format!("{expected:?}"));
}
