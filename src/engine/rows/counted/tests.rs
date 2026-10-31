//! Unit tests of a row store table's count of rows kept with its pages
//! (`counted.rs`).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::disk::buffer::{BufferPool, Clock};
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::Pager;
use crate::disk::shadow::ShadowFileSystem;
use crate::engine::rows::{RowTable, TableFiles, TableOptions};
use crate::error::Code;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::log::MemoryLog;
use crate::storage::{RowId, TableStorage};
use crate::txn::NO_TRANSACTION;
use crate::value::{DataType, Decimal, Spelled, Value};

const TYPES: [DataType; 1] = [DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }];

/// Tables' files in a shadowed file, in memory.
fn shadowed() -> TableFiles {
    let crc: Arc<dyn crate::hash::HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    let shadows = Arc::new(ShadowFileSystem::new(Arc::new(MemoryFileSystem::new()), Arc::clone(&crc)));
    let pager = Arc::new(Pager::create(&*shadows, std::path::Path::new("/rows"), crc, Pipeline::none(), 4096).unwrap());
    let pool = Arc::new(BufferPool::new(64, Box::new(Clock::new(64))).unwrap());
    let file = pool.add_pager(pager);
    TableFiles::new(pool, file, Arc::new(MemoryLog::new())).shadowed(shadows)
}

fn row(n: i128) -> Vec<Value> {
    vec![Value::Exact(Decimal::new(n, 0))]
}

/// Verifies: [DESIGN] RULES.md D31, D25, a table's count of rows kept with
/// its pages: saved, as a checkpoint saves it, a table of a shadowed file
/// opened takes it from its meta page with no pass over its rows, the
/// rows of a transaction open counted as its newest versions are; one of
/// a file not shadowed counts them by a pass, to the same count. A save
/// with the count unchanged writes nothing.
#[test]
fn a_saved_count_needs_no_pass() {
    let files = shadowed();
    let table = RowTable::create(&files, &TYPES, TableOptions::default()).unwrap();
    let ids: Vec<RowId> = (0..3000).map(|n| table.insert(NO_TRANSACTION, row(n)).unwrap()).collect();
    ids.iter().take(10).for_each(|&id| table.delete(NO_TRANSACTION, id).unwrap());
    let tx = 7_u64;
    table.begin(tx, 1).unwrap();
    (0..5).for_each(|n| assert!(table.insert(tx, row(n)).is_ok()));
    assert_eq!(table.row_count(), 2995);
    files.save_counts().unwrap();
    let meta = table.meta();
    let opened = RowTable::open(&files, meta, &TYPES, TableOptions::default()).unwrap();
    assert_eq!((opened.row_count(), files.passes.load(Ordering::Acquire)), (2995, 0), "no pass");
    assert_eq!(opened.count_newest().unwrap(), 2995, "as a pass counts");
    let plain = TableFiles { shadows: None, ..files.clone() };
    let counted = RowTable::open(&plain, meta, &TYPES, TableOptions::default()).unwrap();
    assert_eq!((counted.row_count(), files.passes.load(Ordering::Acquire)), (2995, 1), "a pass, of a file not shadowed");
    // Unchanged: the meta page is not written, so a page not a table's
    // meta page stays as it is.
    *files.pool.write(files.file, meta).unwrap() = vec![0_u8];
    files.save_counts().unwrap();
    assert_eq!(&*files.pool.read(files.file, meta.0).unwrap(), &[0_u8]);
}

/// Verifies: [DESIGN] RULES.md D31, a table's count of rows saved: a meta
/// page malformed is refused as the count is written (SQLCODE -916), the
/// checkpoint then failing; a table dropped frees its meta page, saves its
/// count no more, and writes its meta page no more (SQLCODE -910).
#[test]
fn a_count_is_saved_only_to_its_meta_page() {
    let files = shadowed();
    let table = RowTable::create(&files, &TYPES, TableOptions::default()).unwrap();
    table.insert(NO_TRANSACTION, row(1)).unwrap();
    *files.pool.write(files.file, table.meta()).unwrap() = vec![0_u8];
    assert_eq!(files.save_counts().err().map(|e| e.code), Some(Code::Corrupt));
    let counted = Arc::clone(&table.counted);
    table.discard();
    drop(table);
    files.save_counts().unwrap();
    assert_eq!(counted.holding(|_| Ok(())).err().map(|e| e.code), Some(Code::Api), "its meta page freed");
}
