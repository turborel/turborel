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

mod wide;

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
        null(Value::Char(format!("entry {}", n.wrapping_mul(7) % 30).into())),
        null(Value::Approx(f64::from(n.wrapping_mul(37) % 10_000) / 100.0)),
        null(Value::Char(format!("customer{n}@mail{}.example.com", n % 7).into())),
        null(i(n.wrapping_mul(3001) % 16_000_000)),
    ]
}

fn file() -> (std::sync::Arc<BufferPool>, FileId) {
    memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 4_i32), "a test").unwrap()
}

/// Pages opened by an encoding: each byte stored flipped.
struct Flipped;

impl Seal for Flipped {
    fn seal(&self, _: u64, content: &[u8]) -> Result<Vec<u8>> {
        Ok(content.iter().map(|b| !b).collect())
    }
    fn open<'a>(&self, _: u64, stored: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        Ok(Cow::Owned(stored.iter().map(|b| !b).collect()))
    }
}

/// A group of `ROWS` rows, its pages of 2,048 bytes, sealed by `sealing`.
fn group<'a, S: Seal>(pool: &'a BufferPool, file: FileId, sealing: &'a S) -> (Pages<'a, S>, Group) {
    let pages = Pages {
        pool,
        file,
        capacity: 2048,
        sealing,
        compress: true,
        pax: false,
        imprints: true,
        kernels: &crate::kernels::Portable,
        base: 0,
        sink: None,
    };
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
fn read<S: Seal>(pages: &Pages<'_, S>, group: &Group, prepared: &mut Prepared, (c, from, to): (usize, u32, u32)) -> Vec<String> {
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
    let (pages, group) = group(&pool, file, &Plain);
    let whole = rows(&group, &pages, &TYPES, &[0, 1, 2, 3, 4, 5], (0, ROWS)).unwrap();
    let chunks: usize = group.columns.iter().flatten().map(Vec::len).sum();
    assert!(group.columns.iter().flatten().filter(|zones| zones.len() > 1).count() >= 4, "most columns span chunks");
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
    let (pages, group) = group(&pool, file, &Plain);
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

/// Verifies: [DESIGN] RULES.md D1094, a chunk read in place: of a group
/// whose pages passed through no encoding no chunk's bytes are kept, each
/// run reading them in the pool, and every column reads as its rows; of
/// a group whose pages an encoding opens, each chunk's bytes are kept,
/// opened once, and read alike. MC/DC of the bytes kept: a page as
/// stored (none kept) against one opened by an encoding (kept).
#[test]
fn a_chunk_stored_as_it_is_is_read_in_the_pool() {
    let (pool, file) = file();
    let runs: Vec<(u32, u32)> = (0..ROWS).step_by(700).map(|f| (f, (f + 700).min(ROWS))).collect();
    let kept = |prepared: &Prepared| -> Vec<(bool, bool)> {
        prepared.columns.iter().flatten().map(|held| (held.in_place, held.bytes.is_empty())).collect()
    };
    let (pages, plain) = group(&pool, file, &Plain);
    let whole = rows(&plain, &pages, &TYPES, &[0, 1, 2, 3, 4, 5], (0, ROWS)).unwrap();
    let mut prepared = Prepared::default();
    for &run in &runs {
        for c in 0..TYPES.len() {
            assert_eq!(read(&pages, &plain, &mut prepared, (c, run.0, run.1)), shown(c, between(&whole, run)), "column {c}, {run:?}");
        }
        assert!(!kept(&prepared).is_empty() && kept(&prepared).iter().all(|&held| held == (true, true)), "read in the pool");
    }
    let (sealed_pages, sealed) = group(&pool, file, &Flipped);
    let mut opened = Prepared::default();
    for &run in &runs {
        for c in 0..TYPES.len() {
            let got = read(&sealed_pages, &sealed, &mut opened, (c, run.0, run.1));
            assert_eq!(got, shown(c, between(&whole, run)), "sealed, column {c}, {run:?}");
        }
        assert!(!kept(&opened).is_empty() && kept(&opened).iter().all(|&held| held == (false, false)), "opened once and kept");
    }
}

/// Verifies: [DESIGN] RULES.md D1298, a dictionary's entries made text
/// on first use: a dictionary of texts whose values are read alone is
/// opened and makes no text; read as text it gives its rows' texts as
/// the rows hold them, its entries made text once a chunk; a second
/// reader of the chunks still held, the same run and positions among
/// it, opens none and makes none again. MC/DC of the text made: a chunk
/// never asked for text (none) against one asked (made), and one asked
/// once against one asked again (still once).
#[test]
fn a_dictionarys_entries_are_made_text_when_first_asked_for() {
    let (pool, file) = file();
    let (pages, group) = group(&pool, file, &Plain);
    let whole = rows(&group, &pages, &TYPES, &[0, 1, 2, 3, 4, 5], (0, ROWS)).unwrap();
    // Rows of the column's first chunk, so a second reader finds it held.
    let run = (0, 600);
    let mut prepared = Prepared::default();
    let alone = values(&group, (&pages, &mut prepared), (2, &TEXT), run).unwrap();
    assert_eq!(of_values(&alone), shown(2, between(&whole, run)));
    let opened = prepared.opened;
    assert!(opened > 0, "the chunks are opened");
    assert_eq!(prepared.texts_made(), 0, "values read alone ask for no text");
    let first = texts(&group, (&pages, &mut prepared), (2, &TEXT), run).unwrap().unwrap();
    assert!(first.coded.is_some(), "a run of dictionaries is read as codes");
    assert_eq!(of_texts(&first), shown(2, between(&whole, run)));
    assert_eq!((prepared.opened, prepared.texts_made()), (opened, opened), "each chunk held made text once, none opened again");
    let again = texts(&group, (&pages, &mut prepared), (2, &TEXT), run).unwrap().unwrap();
    assert_eq!(of_texts(&again), of_texts(&first));
    let some = texts_at(&group, (&pages, &mut prepared), (2, &TEXT), &[3, 4, 300, 599]).unwrap().unwrap();
    assert_eq!(of_texts(&some), [3_usize, 4, 300, 599].map(|n| whole[n][2].to_string()));
    assert_eq!((prepared.opened, prepared.texts_made()), (opened, opened), "a second reader makes none again");
}

/// A dictionary chunk of 40 values of the three `entries`, and its bytes.
fn dictionary_of(entries: [&[u8]; 3]) -> Vec<u8> {
    let written: Vec<Option<Vec<u8>>> = (0..40).map(|n| Some(entries[n % 3].to_vec())).collect();
    let records = crate::engine::columns::tests::records_of(&written);
    let fields = crate::engine::columns::Fields::from_records(&TEXT, Cow::Owned(records));
    let bytes = fields.chunk(&TEXT, 0, (40, true)).unwrap();
    assert_eq!(bytes.first(), Some(&super::super::encoded::DICTIONARY));
    bytes
}

/// Verifies: [DESIGN] RULES.md D1298, a dictionary's entries made text
/// on first use: the entries are judged UTF-8 as they are made, before
/// any text is given, so a chunk with an entry that is not UTF-8 gives
/// no codes and no text, the first time and again, judged once; one of
/// UTF-8 entries gives each row's text; and of a type that is no
/// character string no text is made. Robustness: octets no text holds.
#[test]
fn entries_not_text_give_none_however_late_they_are_made() {
    let kernels = &crate::kernels::Portable;
    let coded = |bytes: &[u8], made: &Made, t: &DataType| {
        let chunk = Chunk::parse(bytes).unwrap();
        let (mut text, mut ends, mut codes, mut nulls) = (String::new(), Vec::new(), Vec::new(), Mask::default());
        let held = chunk.coded_made(&[0, 1, 2, 39], (&mut text, &mut ends, &mut codes, &mut nulls), (made, t, kernels));
        held.map(|()| {
            codes
                .iter()
                .map(|&c| Texts::new(text.clone(), ends.clone(), None).get(usize::try_from(c).unwrap()).unwrap().to_owned())
                .collect::<Vec<_>>()
        })
    };
    let count = |made: &Made| made.text_count.load(std::sync::atomic::Ordering::Relaxed);
    let (good, bad) = (
        dictionary_of([b"alpha entry", b"beta entry", b"gamma entry"]),
        dictionary_of([b"alpha entry", b"\xffbeta entry", b"gamma entry"]),
    );
    let made = Made::default();
    assert_eq!(coded(&good, &made, &TEXT).unwrap(), ["alpha entry", "beta entry", "gamma entry", "alpha entry"]);
    assert_eq!(coded(&good, &made, &TEXT).unwrap().len(), 4);
    assert_eq!(count(&made), 1, "made once for both readers");
    let of_bad = Made::default();
    assert_eq!((coded(&bad, &of_bad, &TEXT), coded(&bad, &of_bad, &TEXT)), (None, None), "octets not UTF-8 are no text");
    assert_eq!(count(&of_bad), 1, "judged once");
    assert_eq!(coded(&good, &Made::default(), &INTEGER), None, "no text of a type not of character strings");
}
