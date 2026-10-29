//! Unit tests of frozen row groups (`group.rs`) and of reading them
//! (`scan.rs`).

use std::borrow::Cow;
use std::sync::Arc;

use super::*;
use crate::disk::buffer::BufferPool;
use crate::engine::columns::scan::{candidates, rows};
use crate::engine::{MEMORY_POOL_BYTES, memory_file};
use crate::index::Probe;
use crate::memory::MemoryBudget;
use crate::storage::Condition;
use crate::value::{CompOp, Decimal, Spelled};

const INTEGER: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
const TEXT: DataType = DataType::Varying { length: 100 };
const TYPES: [DataType; 3] = [INTEGER, TEXT, INTEGER];

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// Pages stored as they are given.
struct Plain;

impl Seal for Plain {
    fn seal(&self, _: u64, content: &[u8]) -> Result<Vec<u8>> {
        Ok(content.to_vec())
    }
    fn open<'a>(&self, _: u64, stored: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        Ok(Cow::Borrowed(stored))
    }
}

fn file() -> (Arc<BufferPool>, FileId) {
    memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES), "a test").unwrap()
}

/// Row `n`: K = n, a text of n's length with every seventh null, and
/// n / 100, so its chunks are in order of it.
fn row(n: i128) -> Row {
    let text = if n.checked_rem(7) == Some(0) {
        Value::Null
    } else {
        Value::Char("t".repeat(usize::try_from(n.checked_rem(13).unwrap()).unwrap()))
    };
    vec![int(n), text, int(n.checked_div(100).unwrap())]
}

/// Writes rows `0..count`, with ids from `first` by `step`, and checks
/// what is read back.
fn written(pages: &Pages<'_, Plain>, (first, step): (u64, u64), count: i128) -> Group {
    let ids: Vec<RowId> =
        (0..u64::try_from(count).unwrap()).map(|n| n.checked_mul(step).and_then(|d| d.checked_add(first)).unwrap()).collect();
    let all: Vec<Row> = (0..count).map(row).collect();
    let group = Group::write(pages, &TYPES, &ids, &all).unwrap();
    assert_eq!((group.first, group.last, group.rows), (first, *ids.last().unwrap(), u32::try_from(count).unwrap()));
    assert_eq!(group.row_ids(pages).unwrap(), ids);
    assert!(count < 1000 || group.columns.iter().all(|zones| zones.len() > 1), "columns span pages");
    for n in [0_usize, 1, 999, ids.len().saturating_sub(1)].into_iter().filter(|&n| n < ids.len()) {
        let at = group.position(pages, ids[n]).unwrap().unwrap();
        assert_eq!(usize::try_from(at).unwrap(), n);
        let k = group.value(pages, (0, &INTEGER), at).unwrap();
        assert_eq!(k.to_string(), n.to_string());
        let text = group.value(pages, (1, &TEXT), at).unwrap();
        assert_eq!(text.to_string(), row(i128::try_from(n).unwrap())[1].to_string());
    }
    if step > 1 {
        assert_eq!(group.position(pages, first.saturating_add(1)).unwrap(), None, "an id between");
    }
    assert_eq!(group.position(pages, ids.last().unwrap().saturating_add(1)).unwrap(), None, "past its last");
    group
}

/// Verifies: [DESIGN] RULES.md D31, frozen row groups: rows written read
/// back by position and by row id, every id between first and last its
/// own or not, a column on many pages, nulls and texts among them; and its
/// header read back from its pages gives the same group.
#[test]
fn groups_read_back() {
    let (pool, file) = file();
    let pages = Pages { pool: &pool, file, capacity: 1024, sealing: &Plain, compress: true };
    for (first, step) in [(100_u64, 1_u64), (7, 3)] {
        let group = written(&pages, (first, step), 4000);
        assert_eq!(group.ids.is_empty(), step == 1, "ids kept only if not every one");
        let back = read(&pages, group.header[0], &TYPES).unwrap();
        assert_eq!(
            (back.first, back.last, back.rows, back.ids.clone(), back.header.clone()),
            (group.first, group.last, group.rows, group.ids.clone(), group.header.clone())
        );
        let bounds = |g: &Group| g.columns.iter().flatten().map(|z| (z.run, z.nulls, format!("{:?}", z.bounds))).collect::<Vec<_>>();
        assert_eq!(bounds(&back), bounds(&group));
        assert!(group.pages().len() > group.header.len());
    }
}

/// Verifies: [DESIGN] RULES.md D31, zone maps: a condition on one column
/// reads only the runs of the chunks it may match; the rows of a run hold
/// the columns asked for, the others null; with no condition, or one of
/// two columns, every row is a candidate.
#[test]
fn scans_skip_chunks_a_condition_cannot_match() {
    let (pool, file) = file();
    let pages = Pages { pool: &pool, file, capacity: 512, sealing: &Plain, compress: true };
    let group = written(&pages, (1, 1), 4000);
    let between = Condition {
        columns: vec![2],
        probe: Probe::Range { equal: Vec::new(), low: Box::new((CompOp::Ge, int(10))), high: Box::new((CompOp::Le, int(11))) },
    };
    let runs = candidates(&group, Some(&between));
    let read: u32 = runs.iter().map(|(from, to)| to.saturating_sub(*from)).sum();
    assert!((200..600).contains(&read), "about the 200 rows of 10 and 11: {read}");
    let found: Vec<Row> =
        runs.iter().flat_map(|&run| rows(&group, &pages, &TYPES, &[0, 2], run).unwrap()).filter(|r| between.matches(r)).collect();
    assert_eq!(found.len(), 200);
    assert!(found.iter().all(|r| r[1].is_null()), "a column not asked for is null");
    assert_eq!(candidates(&group, None), vec![(0, 4000)]);
    let two = Condition { columns: vec![0, 2], probe: Probe::Prefix { equal: vec![int(1)], then: None } };
    assert_eq!(candidates(&group, Some(&two)), vec![(0, 4000)]);
    let none = Condition { columns: vec![2], probe: Probe::Compare { op: CompOp::Gt, value: int(100) } };
    assert!(candidates(&group, Some(&none)).is_empty(), "no chunk may match");
}

/// Verifies: [DESIGN] RULES.md D31, frozen row groups: a group of no rows,
/// or of rows not of its types, is refused, and a header of another
/// format, or of other columns, or cut short, is refused with SQLCODE
/// -916.
#[test]
fn what_groups_refuse() {
    let (pool, file) = file();
    let pages = Pages { pool: &pool, file, capacity: 1024, sealing: &Plain, compress: true };
    assert_eq!(Group::write(&pages, &TYPES, &[], &[]).err().map(|e| e.code), Some(Code::Api));
    let wrong = vec![vec![Value::Char("x".into()), Value::Null, int(1)]];
    assert!(Group::write(&pages, &TYPES, &[1], &wrong).is_err(), "not of its types");
    let group = written(&pages, (1, 1), 10);
    assert_eq!(read(&pages, group.header[0], &TYPES[..2]).err().map(|e| e.code), Some(Code::Corrupt), "other columns");
    let mut written_pages = Vec::new();
    let other = put_chain(&pages, &[9, 1, 2], &mut written_pages).unwrap();
    assert_eq!(read(&pages, other[0], &TYPES).err().map(|e| e.code), Some(Code::Corrupt), "another format");
    let short = put_chain(&pages, &[GROUP_FORMAT, 1, 2], &mut written_pages).unwrap();
    assert_eq!(read(&pages, short[0], &TYPES).err().map(|e| e.code), Some(Code::Corrupt), "cut short");
    pages.free(&written_pages);
}

/// Verifies: [DESIGN] RULES.md D31, COLUMNSTORE.ORDER: a group of rows in
/// another order than their ids', every id between its least and greatest
/// among them, keeps its ids in its rows' order and its rows' positions by
/// id: each row found by its id, an id not its found none, its least and
/// greatest ids its range; read back from its header alike; and a header
/// of the format before, with no positions by id, is read as of rows in
/// order.
#[test]
fn groups_of_rows_out_of_order_read_back() {
    let (pool, file) = file();
    let pages = Pages { pool: &pool, file, capacity: 1024, sealing: &Plain, compress: true };
    let count = 4000_u64;
    let ids: Vec<RowId> = (0..count).map(|n| n.checked_mul(7).unwrap().checked_rem(count).unwrap().checked_add(100).unwrap()).collect();
    let all: Vec<Row> = (0..4000).map(row).collect();
    let group = Group::write(&pages, &TYPES, &ids, &all).unwrap();
    assert_eq!((group.first, group.last), (100, 4099));
    assert!(group.sorted() && !group.ids.is_empty(), "its ids kept, and positions by id");
    assert_eq!(group.row_ids(&pages).unwrap(), ids);
    for n in [0_usize, 1, 2, 1500, 3999] {
        assert_eq!(group.position(&pages, ids[n]).unwrap(), Some(u32::try_from(n).unwrap()));
    }
    assert_eq!(group.position(&pages, 4100).unwrap(), None);
    let back = read(&pages, group.header[0], &TYPES).unwrap();
    assert_eq!((back.ids.clone(), back.by_id.clone()), (group.ids.clone(), group.by_id.clone()));
    // The format before: no count of runs by id after the ids'.
    let in_order = written(&pages, (7, 3), 100);
    let mut header = header::encode(&in_order, &TYPES).unwrap();
    let at = 25_usize.checked_add(in_order.ids.len().checked_mul(16).unwrap()).unwrap();
    header.drain(at..at.checked_add(4).unwrap());
    header[0] = 1;
    let mut chain = Vec::new();
    let old = put_chain(&pages, &header, &mut chain).unwrap();
    let before = read(&pages, old[0], &TYPES).unwrap();
    assert!(!before.sorted());
    assert_eq!(before.position(&pages, 10).unwrap(), Some(1));
    pages.free(&chain);
}
