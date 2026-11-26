//! Unit tests of a load's group given by column (`records.rs`,
//! `stamped.rs`, `loaded.rs`, RULES.md D1148): it reads back as the rows,
//! and its extent is the one of the same rows given as rows.

use super::*;
use crate::engine::rows::SegmentPlace;
use crate::engine::rows::options::LoadTo;
use crate::storage::lanes::Records;
use crate::storage::{Loaded, Loading};

/// A pager file in memory and the tables' files in it, their loads
/// writing their groups to segments of their own.
fn segmented() -> TableFiles {
    let fs = Arc::new(MemoryFileSystem::new());
    let hash = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    let pager = Arc::new(Pager::create(&*fs, std::path::Path::new("/rows"), hash.clone(), Pipeline::none(), 4096).unwrap());
    let pool = Arc::new(BufferPool::new(256, Box::new(Clock::new(256))).unwrap());
    let file = pool.add_pager(pager);
    let place = SegmentPlace::new(fs, std::path::PathBuf::from("/segments"), hash).unwrap();
    TableFiles::new(pool, file, Arc::new(MemoryLog::new())).segmented(Arc::new(place))
}

/// Row `n`: a key of fifty, every ninth null, so sorted by it rows tie
/// and nulls sort high, and a text of its own.
fn tied(n: i128) -> Row {
    let k = if n % 9 == 4 { Value::Null } else { Value::Exact(Decimal::new(n % 50, 0)) };
    vec![k, Value::Char(format!("v{n}").into())]
}

/// The records of each column of `rows`, as a load gives them.
fn columns_of(rows: &[Row]) -> Vec<Records> {
    let mut out = vec![Records::default(); TYPES.len()];
    for (n, row) in rows.iter().enumerate() {
        for ((records, v), t) in out.iter_mut().zip(row).zip(&TYPES) {
            if n % 8 == 0 {
                records.nulls.push(0);
            }
            if v.is_null() {
                records.nulls[n >> 3_u32] |= 1 << (n % 8);
            } else {
                records.bytes.extend_from_slice(&t.encode_record(v).unwrap());
            }
            records.ends.push(u32::try_from(records.bytes.len()).unwrap());
        }
    }
    out
}

/// A table of files of its own, loading straight to groups of 2048 rows,
/// sorted by its key if `sorted`, `rows` written by transaction 10 as one
/// group, of their records if `by_records`, as an extent if `whole`: the
/// table and what the load gave back.
fn loaded(rows: &[Row], (sorted, by_records, whole): (bool, bool, bool)) -> (Arc<RowTable>, Loaded) {
    let mut options = options(0);
    if let Some(columns) = options.columns.as_mut() {
        columns.load = LoadTo::Groups;
        columns.order = Places::of(if sorted { &[0] } else { &[] });
    }
    let table = Arc::new(RowTable::create(&segmented(), &TYPES, options).unwrap());
    table.begin(10, 1).unwrap();
    assert_eq!(table.begin_load(10, true).unwrap(), Some(2048));
    let made = if by_records { table.insert_records(10, &columns_of(rows), whole) } else { table.insert_group(10, rows, whole) };
    (table, made.unwrap().unwrap())
}

/// The rows `table` gives transaction 10, each with its id, by id.
fn read(table: &Arc<RowTable>) -> Vec<(RowId, String)> {
    let mut all: Vec<(RowId, String)> =
        Arc::clone(table).scan(10, &[0, 1]).map(|r| r.unwrap()).map(|(id, r)| (id, format!("{r:?}"))).collect();
    all.sort();
    all
}

/// Verifies: [DESIGN] RULES.md D1148, D1145, D1146: a load's group given
/// by column reads back as its rows, each by its id, its ids those of the
/// rows in order; its extent is the extent of the same rows given as
/// rows, byte for byte, sorted by a column of ties and nulls or not; and
/// a group written to pages, not whole, gives no extent and reads back
/// alike.
#[test]
fn a_group_of_records_reads_back_and_is_logged_as_its_rows() {
    let rows: Vec<Row> = (0..2000).map(tied).collect();
    for sorted in [false, true] {
        let (of_rows, by_rows) = loaded(&rows, (sorted, false, true));
        let (of_records, by_records) = loaded(&rows, (sorted, true, true));
        assert_eq!(by_records.ids, by_rows.ids, "sorted {sorted}");
        let extent = by_records.extent.as_ref().unwrap();
        assert_eq!(Some(extent), by_rows.extent.as_ref(), "sorted {sorted}: the extent of its rows");
        let expected: Vec<(RowId, String)> = by_rows.ids.iter().zip(&rows).map(|(&id, r)| (id, format!("{r:?}"))).collect();
        assert_eq!(read(&of_records), expected, "sorted {sorted}");
        assert_eq!(read(&of_rows), expected);
        let (on_pages, by_pages) = loaded(&rows, (sorted, true, false));
        assert!(by_pages.extent.is_none() && by_pages.held.is_none(), "written to pages");
        assert_eq!(read(&on_pages), expected, "sorted {sorted}, on pages");
    }
}

/// Verifies: [DESIGN] RULES.md D1148, R-ERR-2: columns given that are not
/// one of each column of the table are refused with SQLCODE -910; a
/// transaction not loading straight to groups is given none.
#[test]
fn records_not_of_the_table_are_refused() {
    let rows: Vec<Row> = (0..10).map(tied).collect();
    let (table, _) = loaded(&rows, (false, true, true));
    let columns = columns_of(&rows);
    assert_eq!(table.insert_records(10, &columns[..1], true).err().map(|e| e.code), Some(Code::Api));
    table.begin(11, 1).unwrap();
    assert!(table.insert_records(11, &columns, true).unwrap().is_none(), "not loading");
}
