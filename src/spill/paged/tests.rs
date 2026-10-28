//! Unit tests of the built-in spill provider and the spill byte forms
//! (RULES.md R-TEST-1). Split from spill/paged.rs (RULES.md R-SIZE-2).

use std::path::Path;
use std::sync::Arc;

use super::*;
use crate::disk::io::FileSystem;
use crate::disk::memory::MemoryFileSystem;
use crate::spill::{Area, DEFAULT_SPILL_BATCH, WorkAreas, bucket_key, norm_key, record_row, row_record};
use crate::value::{Bucket, Decimal, Value};

const AREA: usize = 1 << 20;

/// An area of `AREA` bytes.
fn asked() -> Area {
    Area::new(AREA, DEFAULT_SPILL_BATCH)
}

/// Temporary files in `/tmp` of a file system in memory.
fn temp() -> (Arc<dyn FileSystem>, Arc<TempFiles>) {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    (Arc::clone(&fs), Arc::new(TempFiles::new(fs, Path::new("/tmp"))))
}

fn files(fs: &Arc<dyn FileSystem>) -> usize {
    fs.read_dir(Path::new("/tmp")).map_or(0, |found| found.len())
}

/// Verifies: [DESIGN] RULES.md D38, `SpillTable`: a key's values come back
/// in the order added, a value too long for an entry kept in a page of its
/// own; removing takes the first equal value, and only one there is.
#[test]
fn tables_keep_values_of_any_length_in_order() {
    let (_, temp) = temp();
    let area = PagedSpill.area(&MemoryBudget::new(AREA << 2_i32), &temp, asked()).unwrap();
    let mut table = area.table().unwrap();
    let long = vec![7_u8; 20_000];
    table.insert(b"k", b"one").unwrap();
    table.insert(b"k", &long).unwrap();
    table.insert(b"k", b"three").unwrap();
    assert_eq!(table.get(b"k").unwrap(), vec![b"one".to_vec(), long.clone(), b"three".to_vec()]);
    assert_eq!(table.get(b"other").unwrap(), Vec::<Vec<u8>>::new());
    assert!(table.remove(b"k", &long).unwrap());
    assert!(!table.remove(b"k", &long).unwrap(), "only one was there");
    assert!(!table.remove(b"other", b"one").unwrap());
    assert_eq!(table.get(b"k").unwrap(), vec![b"one".to_vec(), b"three".to_vec()]);
}

/// Verifies: [DESIGN] RULES.md D38, `SpillRun`: records come back in the
/// order pushed, across many pages and of any length, an empty run none.
#[test]
fn runs_give_their_records_in_order() {
    let (_, temp) = temp();
    let area = PagedSpill.area(&MemoryBudget::new(AREA << 2_i32), &temp, asked()).unwrap();
    let mut run = area.run().unwrap();
    let pushed: Vec<Vec<u8>> = (0_u32..5000_u32).map(|n| n.to_le_bytes().repeat(usize::try_from(n % 50).unwrap())).collect();
    for record in &pushed {
        run.push(record).unwrap();
    }
    assert_eq!(run.records().unwrap().collect::<Result<Vec<_>>>().unwrap(), pushed);
    assert_eq!(area.run().unwrap().records().unwrap().count(), 0);
}

/// Verifies: [DESIGN] RULES.md D38, `Spill`: an area reserves its bytes of
/// the budget while it or anything made in it lives, its file removed
/// after; one the budget cannot grant is refused (SQLCODE -918), and leaves
/// no file.
#[test]
fn areas_reserve_the_budget_and_remove_their_files() {
    let (fs, temp) = temp();
    let memory = MemoryBudget::new(AREA + (AREA >> 1_i32));
    let area = PagedSpill.area(&memory, &temp, asked()).unwrap();
    let table = area.table().unwrap();
    assert_eq!((memory.reserved(), files(&fs)), (AREA, 1));
    let refused = PagedSpill.area(&memory, &temp, asked()).err().map(|e| e.code);
    assert_eq!((refused, files(&fs)), (Some(Code::Resources), 1));
    drop(area);
    assert_eq!(memory.reserved(), AREA, "its table holds it");
    drop(table);
    assert_eq!((memory.reserved(), files(&fs)), (0, 0));
}

/// Verifies: [DESIGN] RULES.md D38, `TempFiles::clear`: the files of spill
/// areas left in a directory are removed, others kept; a directory not
/// made is nothing to clear. `WorkAreas` makes its areas there once told.
#[test]
fn files_left_are_cleared() {
    let (fs, temp) = temp();
    temp.clear().unwrap();
    for name in ["/tmp/a.spill", "/tmp/b.spill", "/tmp/keep"] {
        fs.create_dir_all(Path::new("/tmp")).unwrap();
        fs.open(Path::new(name), crate::disk::io::OpenMode::CreateNew).unwrap();
    }
    let work = WorkAreas::new(MemoryBudget::new(AREA << 1_i32));
    work.set_temp(TempFiles::new(Arc::clone(&fs), Path::new("/tmp"))).unwrap();
    assert_eq!(fs.read_dir(Path::new("/tmp")).unwrap(), vec![Path::new("/tmp/keep").to_path_buf()]);
    let area = work.area(asked()).unwrap();
    assert_eq!(files(&fs), 2);
    drop(area);
    assert!(format!("{work:?}").contains("/tmp"));
}

/// Verifies: [DESIGN] RULES.md D38: a row's record gives the row back; keys
/// are the same bytes exactly when their buckets or values are identical;
/// a malformed record or page is SQLCODE -916.
#[test]
fn byte_forms_round_trip_and_tell_keys_apart() {
    let row = vec![Value::Null, Value::Char("AB".into()), Value::Exact(Decimal::new(15, 1)), Value::Approx(2.5_f64)];
    let literals = |r: &[Value]| r.iter().map(Value::to_literal).collect::<Vec<_>>();
    assert_eq!(literals(&record_row(&row_record(&row)).unwrap()), literals(&row));
    assert_eq!(record_row(b"\xff").err().map(|e| e.code), Some(Code::Corrupt));
    let norms = |r: &[Value]| norm_key(&r.iter().map(Value::norm).collect::<Vec<_>>());
    assert_eq!(norms(&row), norms(&row));
    assert_ne!(norms(&[Value::Char("A".into()), Value::Char("B".into())]), norms(&[Value::Char("AB".into())]));
    assert_ne!(norms(&[Value::Exact(Decimal::new(1, 0))]), norms(&[Value::Approx(1.0_f64)]));
    assert_ne!(bucket_key(&[Bucket::Char("A".into())]), bucket_key(&[Bucket::Number(0)]));
    assert_eq!(split(&[1, 0, 0, 0, 0, 0, 0, 0, 9]).unwrap(), vec![vec![9_u8]]);
    assert_eq!(split(&[2, 0, 0, 0, 0, 0, 0, 0, 9]).err().map(|e| e.code), Some(Code::Corrupt));
    assert_eq!(split(&[1, 0]).err().map(|e| e.code), Some(Code::Corrupt));
}

/// The bytes of the one spill file in `/tmp` of `fs`.
fn file_bytes(fs: &Arc<dyn FileSystem>) -> u64 {
    let files = fs.read_dir(Path::new("/tmp")).unwrap();
    let [only] = files.as_slice() else { panic!("{files:?}") };
    fs.open(only, crate::disk::io::OpenMode::ReadOnly).unwrap().size().unwrap()
}

/// Verifies: [DESIGN] RULES.md D38, `SpillRun::into_records`: a run read
/// once gives its pages back as they are read, so the next run of as many
/// records is written in them and the area's file grows no more; a run
/// read so that it may be read again keeps its pages, and the next grows
/// the file.
#[test]
fn a_run_read_once_gives_its_pages_back() {
    let (fs, temp) = temp();
    let area = PagedSpill.area(&MemoryBudget::new(AREA << 2_i32), &temp, Area::new(64 << 10, 4 << 10)).unwrap();
    let record = vec![7_u8; 1000];
    let written = || {
        let mut run = area.run().unwrap();
        for _ in 0_u32..500_u32 {
            run.push(&record).unwrap();
        }
        run
    };
    assert_eq!(written().into_records().unwrap().count(), 500);
    let once = file_bytes(&fs);
    assert_eq!(written().into_records().unwrap().count(), 500);
    assert_eq!(file_bytes(&fs), once, "written in the pages given back");
    let mut kept = written();
    assert_eq!(kept.records().unwrap().count(), 500);
    assert_eq!(kept.records().unwrap().count(), 500, "read again");
    assert_eq!(written().into_records().unwrap().count(), 500);
    assert!(file_bytes(&fs) > once, "the kept run's pages are its own");
}
