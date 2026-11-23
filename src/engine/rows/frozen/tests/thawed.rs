//! Unit tests of a frozen row changed, as its zone holds it (RULES.md
//! D31, `rows/zones.rs`).

use super::*;

/// Verifies: [DESIGN] RULES.md D31, zone maps of a table of the column
/// store: a frozen row updated in a column other than the one a scan's
/// condition reads is held by its zone in that column too, though the
/// zone, of the tree's rows alone, did not hold the value it kept, so a
/// scan told the condition reads the zone, and the row. A regression test
/// (R-TEST-7): it fails if a write takes the value a frozen row keeps as
/// lying within its zone's bounds.
#[test]
fn a_frozen_row_changed_is_held_by_its_zone_in_the_columns_it_keeps() {
    let (_, files) = files();
    let (table, ids) = table(&files, 0, 2148);
    ended(&table, 1);
    assert_eq!(table.frozen.directory().unwrap().groups.len(), 1);
    table.begin(5, 4).unwrap();
    // Its zone holds a row of the tree, of another key.
    table.update(5, ids[0], row(-7)).unwrap();
    // Its key kept, its text changed.
    table.update(5, ids[1], vec![Value::Exact(Decimal::new(1, 0)), Value::Char("changed".into())]).unwrap();
    table.validate(5, 5).unwrap();
    table.commit(5, 5);
    let one =
        Conjunction::of(Condition { columns: vec![0], probe: Probe::Compare { op: CompOp::Eq, value: Value::Exact(Decimal::new(1, 0)) } });
    let held = |id: RowId| table.zones.ranges(Some(&one)).iter().any(|&(first, end)| first <= id && end.is_none_or(|e| id < e));
    assert!(held(ids[1]), "its zone holds the key it kept");
    assert!(!held(ids[2047]), "a zone of frozen rows alone holds none");
    let read: Vec<String> = Arc::clone(&table).scan_where(NO_TRANSACTION, &[0, 1], &one).map(|r| r.unwrap().1[1].to_string()).collect();
    assert!(read.contains(&"changed".to_owned()), "{read:?}");
}
