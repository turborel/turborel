//! Unit tests of the chunks a scan opens once (`prepared.rs`).

use std::borrow::Cow;

use super::*;
use crate::disk::buffer::{BufferPool, FileId};
use crate::engine::columns::group::Group;
use crate::engine::columns::scan::{floats, ints, ints_at, rows, texts, texts_at, values, values_at};
use crate::engine::{MEMORY_POOL_BYTES, memory_file};
use crate::memory::MemoryBudget;
use crate::storage::{Row, Texts};
use crate::value::{Decimal, Spelled, Value};

const INTEGER: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
const TEXT: DataType = DataType::varying(100);
const DOUBLE: DataType = DataType::Approximate { precision: 53, spelled: Spelled::DoublePrecision };
/// K, a number of runs, a text of 30 values, a price, a text alike in its
/// parts, and a frame of reference of 24 bits.
const TYPES: [DataType; 6] = [INTEGER, INTEGER, TEXT, DOUBLE, TEXT, INTEGER];
const ROWS: u32 = 5000;

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

/// Row `n`, every eleventh value of each column but K null.
fn row(n: u32) -> Row {
    let i = |v: u32| Value::Exact(Decimal::new(i128::from(v), 0));
    let null = |v: Value| if n % 11 == 4 { Value::Null } else { v };
    vec![
        i(n),
        null(i(n.checked_div(700).unwrap())),
        null(Value::Char(format!("entry {}", n.wrapping_mul(7) % 30))),
        null(Value::Approx(f64::from(n.wrapping_mul(37) % 10_000) / 100.0)),
        null(Value::Char(format!("customer{n}@mail{}.example.com", n % 7))),
        null(i(n.wrapping_mul(3001) % 16_000_000)),
    ]
}

fn file() -> (std::sync::Arc<BufferPool>, FileId) {
    memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 4_i32), "a test").unwrap()
}

/// A group of `ROWS` rows, its pages of 2,048 bytes.
fn group(pool: &BufferPool, file: FileId) -> (Pages<'_, Plain>, Group) {
    let pages = Pages { pool, file, capacity: 2048, sealing: &Plain, compress: true, pax: false, imprints: true };
    let ids: Vec<u64> = (1..=u64::from(ROWS)).collect();
    let all: Vec<Row> = (0..ROWS).map(row).collect();
    let group = Group::write(&pages, &TYPES, (&ids, &all), (&[], &[], &[])).unwrap();
    (pages, group)
}

/// Column `c` of `rows`, each value shown.
fn shown(c: usize, rows: &[Row]) -> Vec<String> {
    rows.iter().map(|r| r[c].to_string()).collect()
}

fn of_texts(v: &Texts) -> Vec<String> {
    (0..v.len()).map(|n| v.get(n).map_or_else(|| "NULL".to_owned(), str::to_owned)).collect()
}

fn of_values(v: &[Value]) -> Vec<String> {
    v.iter().map(ToString::to_string).collect()
}

/// Column `c` of the rows `from` to `to` as the batch scan reads it,
/// through `prepared`.
fn read(pages: &Pages<'_, Plain>, group: &Group, prepared: &mut Prepared, (c, from, to): (usize, u32, u32)) -> Vec<String> {
    let t = &TYPES[c];
    if let Some(v) = ints(group, (pages, &mut *prepared), (c, t), (from, to)).unwrap() {
        return of_values(v.values());
    }
    if let Some(v) = floats(group, (pages, &mut *prepared), (c, t), (from, to)).unwrap() {
        return of_values(v.values());
    }
    if let Some(v) = texts(group, (pages, &mut *prepared), (c, t), (from, to)).unwrap() {
        return of_texts(&v);
    }
    of_values(&values(group, (pages, prepared), (c, t), (from, to)).unwrap())
}

/// The rows from `from` to `to` of `whole`.
fn between(whole: &[Row], (from, to): (u32, u32)) -> &[Row] {
    &whole[usize::try_from(from).unwrap()..usize::try_from(to).unwrap()]
}

/// Verifies: [DESIGN] RULES.md D352, the chunks a scan opens once: every
/// column, of frames, runs, a dictionary of texts, decimals and texts
/// coded by a table, read by runs of 100 rows through one cache, reads
/// as the rows read without it; each chunk is opened once for all its
/// runs, and two columns' chunks of the same first row are each their
/// own; after `clear`, the chunks are opened again, and runs read
/// backward read alike.
#[test]
fn chunks_are_opened_once_for_their_runs() {
    let (pool, file) = file();
    let (pages, group) = group(&pool, file);
    let whole = rows(&group, &pages, &TYPES, &[0, 1, 2, 3, 4, 5], (0, ROWS)).unwrap();
    let chunks: usize = group.columns.iter().map(Vec::len).sum();
    assert!(group.columns.iter().filter(|zones| zones.len() > 1).count() >= 4, "most columns span chunks");
    let mut prepared = Prepared::default();
    let runs: Vec<(u32, u32)> = (0..ROWS).step_by(100).map(|f| (f, (f + 100).min(ROWS))).collect();
    for &run in &runs {
        for c in 0..TYPES.len() {
            assert_eq!(read(&pages, &group, &mut prepared, (c, run.0, run.1)), shown(c, between(&whole, run)), "column {c}, {run:?}");
        }
    }
    assert_eq!(prepared.opened, chunks, "each chunk opened once");
    prepared.clear();
    for &run in runs.iter().rev() {
        for c in 0..TYPES.len() {
            assert_eq!(read(&pages, &group, &mut prepared, (c, run.0, run.1)), shown(c, between(&whole, run)), "backward, column {c}");
        }
    }
    assert!(prepared.opened > chunks, "opened again after clear");
}

/// Verifies: [DESIGN] RULES.md D352, the chunks a scan opens once: rows
/// at positions across chunks, of a frame, a dictionary of texts, texts
/// coded by a table and decimals, read through the cache twice, read as
/// the rows read without it.
#[test]
fn positions_read_through_the_cache() {
    let (pool, file) = file();
    let (pages, group) = group(&pool, file);
    let whole = rows(&group, &pages, &TYPES, &[0, 1, 2, 3, 4, 5], (0, ROWS)).unwrap();
    let positions: Vec<u32> = (0..ROWS).filter(|n| n % 13 < 3).collect();
    let picked: Vec<Row> = positions.iter().map(|&p| whole[usize::try_from(p).unwrap()].clone()).collect();
    let mut prepared = Prepared::default();
    for _ in 0_u32..2_u32 {
        let made = ints_at(&group, (&pages, &mut prepared), (5, &INTEGER), &positions).unwrap().unwrap();
        assert_eq!(of_values(made.values()), shown(5, &picked));
        let coded = texts_at(&group, (&pages, &mut prepared), (2, &TEXT), &positions).unwrap().unwrap();
        assert_eq!(of_texts(&coded), shown(2, &picked));
        let symbols = texts_at(&group, (&pages, &mut prepared), (4, &TEXT), &positions).unwrap().unwrap();
        assert_eq!(of_texts(&symbols), shown(4, &picked));
        let values = values_at(&group, (&pages, &mut prepared), (3, &DOUBLE), &positions).unwrap();
        assert_eq!(of_values(&values), shown(3, &picked));
    }
}
