//! Unit tests of the rows a global index names, read from their
//! partitions (`dispatch.rs`).

use std::collections::HashMap;
use std::sync::Arc;

use super::super::global::packed;
use super::Dispatched;
use crate::disk::buffer::{BufferPool, Clock};
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::engine::rows::{RowTable, TableFiles, TableOptions};
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::log::MemoryLog;
use crate::storage::{DEFAULT_FETCH_COST, TableStorage};
use crate::txn::NO_TRANSACTION;
use crate::value::{DataType, Decimal, Spelled, Value};

const INT: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };

/// A partition in memory of one integer column, of the rows `values`.
fn partition(values: &[i128]) -> Arc<dyn TableStorage> {
    let fs = MemoryFileSystem::new();
    let pager = Arc::new(
        Pager::create(&fs, std::path::Path::new("/rows"), Arc::new(Crc::new(CRC32_ISCSI).unwrap()), Pipeline::none(), 4096).unwrap(),
    );
    let pool = Arc::new(BufferPool::new(64, Box::new(Clock::new(64))).unwrap());
    let file = pool.add_pager(Arc::clone(&pager));
    let files = TableFiles::new(pool, file, Arc::new(MemoryLog::new()));
    let table = RowTable::create(&files, &[INT], TableOptions::default()).unwrap();
    for &v in values {
        table.insert(NO_TRANSACTION, vec![Value::Exact(Decimal::new(v, 0))]).unwrap();
    }
    Arc::new(table)
}

/// Verifies: [EXTENSION] RULES.md D157, a global index's rows: each
/// packed row id is read from the partition it names, whole, of some
/// columns and as its newest version; an id of a partition no longer in
/// the tree is no row; the rows are counted over every partition and a
/// scan of the table itself gives none; every write through it is
/// refused with SQLCODE -910, its rows written to its partitions.
#[test]
fn rows_are_read_from_the_partition_their_id_names() {
    let (one, two) = (partition(&[10, 11]), partition(&[20]));
    let first_id = Arc::clone(&two).scan(NO_TRANSACTION, &[0]).next().unwrap().unwrap().0;
    let d = Arc::new(Dispatched { leaves: HashMap::from([(1, one), (2, two)]) });
    let id = packed(2, first_id).unwrap();
    let shown = |r: Option<Vec<Value>>| r.map(|r| r[0].to_string());
    assert_eq!(shown(d.get(NO_TRANSACTION, id).unwrap()), Some("20".to_owned()));
    assert_eq!(shown(d.get_columns(NO_TRANSACTION, id, &[0]).unwrap()), Some("20".to_owned()));
    assert_eq!(shown(d.newest(NO_TRANSACTION, id).unwrap().row), Some("20".to_owned()));
    let gone = packed(9, first_id).unwrap();
    assert!(d.get(NO_TRANSACTION, gone).unwrap().is_none());
    assert!(d.get_columns(NO_TRANSACTION, gone, &[0]).unwrap().is_none());
    assert!(d.newest(NO_TRANSACTION, gone).unwrap().row.is_none());
    assert_eq!(d.row_count(), 3_usize);
    assert_eq!(Arc::clone(&d).scan(NO_TRANSACTION, &[0]).count(), 0);
    assert!(d.fetch_cost() > 0.0_f64);
    assert_eq!(Dispatched { leaves: HashMap::new() }.fetch_cost().to_bits(), DEFAULT_FETCH_COST.to_bits());
    let row = || vec![Value::Exact(Decimal::new(1, 0))];
    let refusals = [
        d.insert(NO_TRANSACTION, row()).map(|_| ()),
        d.update(NO_TRANSACTION, id, row()),
        d.delete(NO_TRANSACTION, id),
        d.restore(NO_TRANSACTION, id, row()),
    ];
    for refused in refusals {
        let e = refused.unwrap_err();
        assert!(e.sqlcode() == -910_i32 && e.to_string().contains("written to its partitions"), "{e}");
    }
}
