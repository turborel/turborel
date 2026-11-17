//! The defaults of the storage foundation's table trait (RULES.md D30,
//! R-SAFE-3), as an engine keeping neither versions, order, statistics
//! nor large objects of its own has them: checked on the tests'
//! `MemoryTable`, which overrides none of them.

use std::sync::Arc;

use turborel::error::Code;
use turborel::storage::TableStorage;
use turborel::txn::NO_TRANSACTION;
use turborel::value::Value;
use turborel::value::lob::Lob;

use crate::common::MemoryTable;

/// Verifies: [DESIGN] RULES.md D30: by default a table gives no ordered
/// scan, rows or batches; writes its newest version as an update or a
/// delete, whatever was seen; analyzes, preserves and rolls back to a
/// savepoint as nothing to do; has no row every snapshot sees, and takes
/// back none.
#[test]
fn a_tables_defaults_do_nothing_of_their_own() {
    let t = Arc::new(MemoryTable::new());
    let id = t.insert(1, vec![Value::Bool(true)]).unwrap();
    assert!(Arc::clone(&t).scan_ordered(1, &[0], (&[0], false, None), None).is_none());
    assert!(Arc::clone(&t).scan_ordered_batches(1, &[0], (&[0], true, Some(1)), None).is_none());
    t.write_newest(1, id, Some(vec![Value::Bool(false)]), 0).unwrap();
    assert_eq!(t.get(1, id).unwrap().map(|r| r[0].to_string()).as_deref(), Some("FALSE"), "an update");
    t.write_newest(1, id, None, 0).unwrap();
    assert!(t.get(1, id).unwrap().is_none(), "a delete");
    t.analyze().unwrap();
    t.preserving(1).unwrap();
    t.rollback_to(1, 0).unwrap();
    assert!(!t.visible_to_all(id));
    t.taken_back(id);
    t.rollback(1).unwrap();
}

/// Verifies: [DESIGN] RULES.md D30, D79: by default a table keeps a row's
/// large objects as they are, logging none; refuses an object the log
/// gives it (SQLCODE -910), as it keeps none; notes no stale object; ends
/// recovery with nothing to free and has none awaiting collection.
#[test]
fn a_table_keeps_no_large_objects_by_default() {
    let t = MemoryTable::new();
    let mut row = vec![Value::Lob(Lob::of_text("kept as it is"))];
    let mut logged = 0_usize;
    let mut log = |_: u64, _: &[u8], _: &mut dyn FnMut() -> turborel::error::Result<()>| -> turborel::error::Result<()> {
        logged += 1;
        Ok(())
    };
    t.keep_lobs(NO_TRANSACTION, None, &mut row, &mut log).unwrap();
    assert_eq!(logged, 0);
    assert_eq!(row[0].to_string(), "kept as it is");
    assert_eq!(t.apply_lob(7, b"body").unwrap_err().code, Code::Api);
    t.note_stale_lobs(&[1, 2]);
    t.recovered().unwrap();
    assert_eq!(t.stale_lobs().unwrap(), Vec::<u64>::new());
}
