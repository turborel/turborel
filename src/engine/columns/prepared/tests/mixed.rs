//! Unit tests of a column of texts whose chunks take several encodings,
//! read by runs and at positions across them (RULES.md D1252, D1316).

use super::super::*;
use super::{Plain, between, file, of_texts, shown};
use crate::engine::columns::encoded::{CONSTANT, DICTIONARY, FSST};
use crate::engine::columns::group::Group;
use crate::engine::columns::scan::{rows, texts, texts_at};
use crate::storage::Row;
use crate::value::{Decimal, Spelled, Value};

const KEY: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
const TEXT: DataType = DataType::varying(100);
const NAME: DataType = DataType::character(12);
/// K, a VARCHAR whose rows are by stretches few values, texts alike in
/// their parts, one value and texts apart, and a CHARACTER of few values.
const TYPES: [DataType; 3] = [KEY, TEXT, NAME];
const ROWS: u32 = 6000;
/// The rows of a stretch of one kind of text.
const STRETCH: u32 = 750;

/// The text of row `n` of the VARCHAR column.
fn text(n: u32) -> String {
    match n.checked_div(STRETCH).unwrap_or(0) % 4 {
        0 => format!("entrée {}", n.wrapping_mul(7) % 30),
        1 => format!("customer{n}@mail{}.example.com", n % 7),
        2 => "one value".to_owned(),
        // Six ideographs from U+4E00 on, apart from row to row.
        _ => (0..6_u32)
            .filter_map(|i| char::from_u32(0x4E00_u32.wrapping_add(n.wrapping_mul(2_654_435_761).rotate_left(i.wrapping_mul(5)) % 20_000)))
            .collect(),
    }
}

/// Row `n`, every eleventh text null, every seventeenth empty.
fn row(n: u32) -> Row {
    let of = |s: String| match (n % 11, n % 17) {
        (4, _) => Value::Null,
        (_, 6) => Value::Char(String::new().into()),
        _ => Value::Char(s.into()),
    };
    vec![Value::Exact(Decimal::new(i128::from(n), 0)), of(text(n)), of(format!("nom {}", n.checked_div(40).unwrap_or(0) % 9))]
}

/// A group of `ROWS` rows, its pages of 2,048 bytes, and its rows as they
/// read one at a time.
fn group(pool: &crate::disk::buffer::BufferPool, file: crate::disk::buffer::FileId) -> (Pages<'_, Plain>, Group, Vec<Row>) {
    let pages = Pages {
        pool,
        file,
        capacity: 2048,
        sealing: &Plain,
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
    let whole = rows(&group, &pages, &TYPES, &[0, 1, 2], (0, ROWS)).unwrap();
    (pages, group, whole)
}

/// Each chunk of the VARCHAR column: its first row, the row past its
/// last, and its encoding.
fn chunks(pages: &Pages<'_, Plain>, group: &Group) -> Vec<(u32, u32, u8)> {
    let zones = group.zones(1).unwrap().unwrap();
    zones
        .iter()
        .map(|z| (z.run.first, z.run.first.saturating_add(z.run.rows), pages.with_run(&z.run, |bytes| Ok(bytes[0])).unwrap()))
        .collect()
}

/// Whether a chunk of the encoding `kind` shares entries among its rows.
fn shares(kind: u8) -> bool {
    kind == DICTIONARY || kind == CONSTANT
}

/// Verifies: [DESIGN] RULES.md D1252, D1316, a run's texts across
/// chunks: a VARCHAR column whose chunks are dictionaries, texts coded
/// by symbols and others, and a CHARACTER column, nulls and empty
/// strings among their rows, read by runs of 100, 700 and 2,048 rows
/// through one cache, give the rows read one at a time; a run of chunks
/// that share no entry is texts, one that reads a dictionary or a chunk
/// of one value is codes of no more entries than rows, and so is a run
/// that reads the last rows of a chunk and the first of the next where
/// one of the two shares entries and the other does not.
#[test]
fn runs_across_chunks_of_several_encodings_read_as_their_rows() {
    let (pool, file) = file();
    let (pages, group, whole) = group(&pool, file);
    let chunks = chunks(&pages, &group);
    assert!(chunks.iter().any(|c| c.2 == DICTIONARY) && chunks.iter().any(|c| c.2 == FSST), "{chunks:?}");
    let mut prepared = Prepared::default();
    let (mut coded, mut plain) = (0_u32, 0_u32);
    for width in [100_u32, 700, 2048] {
        for from in (0..ROWS).step_by(usize::try_from(width).unwrap()) {
            let run = (from, (from + width).min(ROWS));
            for c in [1_usize, 2] {
                let made = texts(&group, (&pages, &mut prepared), (c, &TYPES[c]), run).unwrap().unwrap();
                assert_eq!(of_texts(&made), shown(c, between(&whole, run)), "column {c}, {run:?}");
                let read = |z: &&(u32, u32, u8)| z.0 < run.1 && z.1 > run.0 && shares(z.2);
                if c == 1 {
                    assert_eq!(made.coded.is_some(), chunks.iter().any(|z| read(&z)), "{run:?}: codes of a run that reads a dictionary");
                }
                assert!(made.coded.as_ref().is_none_or(|held| held.entries.len() <= made.len()), "no more entries than rows");
                coded += u32::from(made.coded.is_some());
                plain += u32::from(made.coded.is_none());
            }
        }
    }
    assert!(coded > 0 && plain > 0, "runs of codes ({coded}) and runs of texts ({plain})");
    let crossing = chunks.windows(2).find(|pair| shares(pair[0].2) != shares(pair[1].2)).unwrap();
    let run = (crossing[0].1 - 5, crossing[1].0 + 5);
    let made = texts(&group, (&pages, &mut prepared), (1, &TEXT), run).unwrap().unwrap();
    assert_eq!((of_texts(&made), made.coded.is_some()), (shown(1, between(&whole, run)), true), "across {crossing:?}");
}

/// Verifies: [DESIGN] RULES.md D1252, D1316, texts at positions across
/// chunks: rows at positions of the same columns, some one after
/// another and some apart, a vector of them at a time, give the rows
/// read one at a time, their entries no more than the rows asked, so a
/// dictionary asked for a few rows gives only the entries they read.
#[test]
fn positions_across_chunks_of_several_encodings_read_as_their_rows() {
    let (pool, file) = file();
    let (pages, group, whole) = group(&pool, file);
    let positions: Vec<u32> = (0..ROWS).filter(|n| n % 13 < 3 || (2000..2300).contains(n) || n % 97 == 0).collect();
    let mut prepared = Prepared::default();
    for these in positions.chunks(500) {
        let picked: Vec<Row> = these.iter().map(|&p| whole[usize::try_from(p).unwrap()].clone()).collect();
        for c in [1_usize, 2] {
            let made = texts_at(&group, (&pages, &mut prepared), (c, &TYPES[c]), these).unwrap().unwrap();
            assert_eq!(of_texts(&made), shown(c, &picked), "column {c}, from {:?}", these.first());
            assert!(made.coded.as_ref().is_none_or(|held| held.entries.len() <= made.len()), "no more entries than rows");
        }
    }
    let few = [3_u32, 4, 5, 700, 5999];
    let made = texts_at(&group, (&pages, &mut prepared), (1, &TEXT), &few).unwrap().unwrap();
    assert_eq!(of_texts(&made), few.map(|n| whole[usize::try_from(n).unwrap()][1].to_string()));
    assert!(texts_at(&group, (&pages, &mut prepared), (1, &TEXT), &[5, ROWS]).unwrap().is_none(), "a position past the group");
}
