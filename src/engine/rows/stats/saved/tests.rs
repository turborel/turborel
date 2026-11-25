//! Unit tests of a table's statistics kept with its pages (`saved.rs`,
//! `image.rs`): saved and read again, of a table of the earlier meta
//! page, sealed, freed, and refused where malformed (RULES.md R-TEST-1,
//! R-TEST-2).

use std::sync::Arc;

use super::super::image::{decoded, encoded};
use super::STATISTICS;
use crate::disk::buffer::{BufferPool, Clock};
use crate::disk::encoding::{Encoding, EncodingId, Encodings, Pipeline};
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::{PageId, Pager};
use crate::engine::record::RecordFormat;
use crate::engine::rows::meta::{meta_content, parse_meta};
use crate::engine::rows::sealing::Sealing;
use crate::engine::rows::{RowTable, TableFiles, TableOptions};
use crate::error::{Code, Result};
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::log::MemoryLog;
use crate::stats::TableStats;
use crate::storage::{Row, TableStorage};
use crate::txn::NO_TRANSACTION;
use crate::value::{CompOp, DataType, Decimal, Spelled, Value};

/// A number, a text null in every seventh row and of one value in every
/// second, and a date.
fn types() -> [DataType; 3] {
    [DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer }, DataType::varying(40), DataType::Date]
}

fn int(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

fn row(n: u64) -> Row {
    let word = if n.is_multiple_of(2) { 0 } else { n % 5 };
    let text = if n.is_multiple_of(7) { Value::Null } else { Value::Char(format!("secret {word}").into()) };
    let day = crate::value::cast(Value::Char(format!("2026-01-{:02}", (n % 28).wrapping_add(1)).into()), &DataType::Date).unwrap();
    vec![int(n), text, day]
}

/// The tables' files, in memory, and their pager.
fn paged() -> (Arc<Pager>, TableFiles) {
    let fs = MemoryFileSystem::new();
    let crc = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    let pager = Arc::new(Pager::create(&fs, std::path::Path::new("/rows"), crc, Pipeline::none(), 4096).unwrap());
    let pool = Arc::new(BufferPool::new(512, Box::new(Clock::new(512))).unwrap());
    let file = pool.add_pager(Arc::clone(&pager));
    (pager, TableFiles::new(pool, file, Arc::new(MemoryLog::new())))
}

/// A table of `rows` rows in `files`.
fn table_of(files: &TableFiles, rows: u64) -> RowTable {
    let table = RowTable::create(files, &types(), TableOptions::default()).unwrap();
    for n in 0..rows {
        table.insert(NO_TRANSACTION, row(n)).unwrap();
    }
    table
}

fn opened(files: &TableFiles, meta: PageId) -> RowTable {
    RowTable::open(files, meta, &types(), TableOptions::default()).unwrap()
}

/// What two statistics hold, as text, to compare them whole.
fn shown(stats: &TableStats) -> String {
    let sample = stats.sample.as_ref().map(|s| s.rows().to_vec());
    format!("{} {} {:?} {:?}", stats.rows, stats.width, stats.columns, sample)
}

/// Verifies: [DESIGN] RULES.md D1093, statistics kept: those a pass
/// gathered are saved by the next checkpoint, on a page the meta page
/// names, and are the table's as it is opened, every number, bound,
/// histogram bound, common value, frequency and sampled row as gathered,
/// no pass made by the plan that asks; a second checkpoint with no pass
/// since writes nothing. MC/DC of the pass made by the asking: none
/// saved (made, the test of D1092) against saved (not made).
#[test]
fn statistics_saved_are_a_table_openeds() {
    let (_, files) = paged();
    let table = table_of(&files, 1_000);
    let gathered = table.gather().unwrap();
    assert_eq!(table.gathered.page(), 0, "no page until a checkpoint");
    files.save_statistics().unwrap();
    let page = table.gathered.page();
    assert_ne!(page, 0);
    assert_eq!(parse_meta(&files.pool.read(files.file, table.meta().0).unwrap()).map(|m| m.statistics), Some(page));
    let version = files.pool.read(files.file, page).unwrap().to_vec();
    files.save_statistics().unwrap();
    assert_eq!(files.pool.read(files.file, page).unwrap().to_vec(), version, "nothing waits");
    let meta = table.meta();
    drop(table);
    let again = opened(&files, meta);
    let held = again.gathered.get().expect("read as it opens");
    assert_eq!(shown(&held), shown(&gathered));
    assert!(held.columns[1].nulls > 0.1_f64, "nulls");
    assert!(!held.columns[1].common.is_empty(), "common values");
    assert!(held.columns[0].histogram.bounds().len() > 2, "a histogram");
    assert!(held.columns[0].frequencies.is_some() && held.columns[2].min.is_some());
    assert_eq!(held.sample.as_ref().map(|s| s.rows().len()), Some(1_000));
    let asked = again.statistics().expect("the plan asking is given them");
    assert_eq!(again.gathered.passes(), 0, "no pass made by the asking");
    assert!(asked.selectivity(0, CompOp::Lt, &int(500)).is_some_and(|s| s > 0.2_f64 && s < 0.6_f64));
    assert!(!again.gather_wanted(), "as many rows as they found: not due");
    assert_eq!(again.gathered.page(), page, "the page kept");
}

/// Verifies: [DESIGN] RULES.md D1093, statistics kept of a table changed
/// since: the rows a table opened holds more than its statistics found
/// count as changes since their pass, so a table that grew past the
/// share after its last pass is due once a plan has read them, as it
/// would have been. MC/DC of the changes counted: rows written after
/// the pass (due) against none (not due, the test above).
#[test]
fn rows_written_after_the_pass_are_a_table_openeds_changes() {
    let (_, files) = paged();
    let table = table_of(&files, 1_000);
    table.gather().unwrap();
    files.save_statistics().unwrap();
    for n in 0..400 {
        table.insert(NO_TRANSACTION, row(n)).unwrap();
    }
    files.save_counts().unwrap();
    let meta = table.meta();
    drop(table);
    let again = opened(&files, meta);
    assert!(again.statistics().is_some() && again.gathered.passes() == 0);
    assert!(again.gather_wanted(), "400 rows more than the 1,000 they found: due");
}

/// Verifies: [DESIGN] RULES.md D1093, a table written before statistics
/// were kept: its meta page, ending at its large objects' tree, opens,
/// naming no statistics, so the plan first asking gathers them (D1092);
/// the next checkpoint gives it a page, and opened again it has them.
#[test]
fn a_meta_page_of_no_statistics_word_opens_with_none() {
    let (_, files) = paged();
    let table = table_of(&files, 300);
    let meta = table.meta();
    files.save_counts().unwrap();
    drop(table);
    {
        let mut page = files.pool.write(files.file, meta).unwrap();
        let named = parse_meta(&page).unwrap();
        let mut before = meta_content(named);
        before.truncate(before.len().saturating_sub(8));
        assert_eq!(parse_meta(&before), Some(named));
        *page = before;
    }
    let old = opened(&files, meta);
    assert!(old.gathered.get().is_none() && old.gathered.page() == 0);
    assert!(old.statistics().is_some());
    assert_eq!(old.gathered.passes(), 1, "gathered by the asking");
    files.save_statistics().unwrap();
    drop(old);
    let again = opened(&files, meta);
    assert!(again.gathered.get().is_some_and(|s| s.rows == 300));
    assert!(parse_meta(&[crate::engine::rows::TABLE_FORMAT; 50]).is_none(), "of neither length");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §11.18 GR2], RULES.md D1093:
/// a dropped table frees the page its statistics were saved in with the
/// rest, so every page it took is free once it goes, and a checkpoint
/// after saves nothing of it; one dropped with statistics no checkpoint
/// saved has no such page to free.
#[test]
fn a_dropped_tables_statistics_page_is_freed() {
    let (pager, files) = paged();
    let in_use = || pager.page_count() - pager.free_pages();
    let before = in_use();
    for saved in [true, false] {
        let table = table_of(&files, 2_000);
        table.gather().unwrap();
        let without = in_use();
        if saved {
            files.save_statistics().unwrap();
            assert!(in_use() > without, "the statistics page");
        }
        table.discard();
        drop(table);
        assert_eq!(in_use(), before, "every page freed, the statistics' among them");
        files.save_statistics().unwrap();
        assert_eq!(in_use(), before);
    }
}

/// An encryption, near enough: flips every byte.
struct Flip;

impl Encoding for Flip {
    fn id(&self) -> EncodingId {
        EncodingId(77)
    }
    fn name(&self) -> &'static str {
        "flip"
    }
    fn overhead(&self, _: &str) -> usize {
        0
    }
    fn confidential(&self) -> bool {
        true
    }
    fn encode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        out.extend(input.iter().map(|b| !b));
        Ok(())
    }
    fn decode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        out.extend(input.iter().map(|b| !b));
        Ok(())
    }
}

/// Whether `bytes` hold `text`.
fn holds(bytes: &[u8], text: &str) -> bool {
    bytes.windows(text.len()).any(|w| w == text.as_bytes())
}

/// Verifies: [DESIGN] RULES.md D1093, D48, the statistics of a table
/// whose rows are sealed: the page holds a sample of its rows, so it is
/// sealed by the table's encodings, no value of a row readable in it,
/// where a table not sealed stores them as they are; the table opened
/// with its encodings reads them; one whose encodings are not registered
/// saves the page empty, and opened has none, and gathers them as asked.
/// MC/DC of the sealing: sealed (not readable) against plain (readable),
/// encodings registered (saved) against not (saved empty).
#[test]
fn a_sealed_tables_statistics_are_sealed() {
    let (_, files) = paged();
    let encodings = Arc::new(Encodings::default());
    encodings.add(Arc::new(Flip)).unwrap();
    let sealing = || Sealing::named(Arc::clone(&encodings), "flip");
    let plain = table_of(&files, 500);
    plain.gather().unwrap();
    let sealed = RowTable::create(&files, &types(), TableOptions::default()).unwrap().sealed(sealing());
    for n in 0..500 {
        sealed.insert(NO_TRANSACTION, row(n)).unwrap();
    }
    let gathered = sealed.gather().unwrap();
    files.save_statistics().unwrap();
    let stored = |table: &RowTable| files.pool.read(files.file, table.gathered.page()).unwrap().to_vec();
    assert!(holds(&stored(&plain), "secret 3"), "a table not sealed");
    assert!(!holds(&stored(&sealed), "secret") && !stored(&sealed).is_empty(), "sealed as its rows are");
    let meta = sealed.meta();
    drop(sealed);
    let again = RowTable::open_with(&files, (meta, &types()), TableOptions::default(), sealing()).unwrap();
    assert_eq!(again.gathered.get().map(|s| shown(&s)), Some(shown(&gathered)));
    // Its encodings gone from the registry: nothing of its rows is saved.
    again.gathered.seal_with(&Arc::new(Sealing::named(Arc::default(), "flip")));
    again.gather().unwrap();
    files.save_statistics().unwrap();
    assert!(stored(&again).is_empty(), "saved empty");
    drop(again);
    let unknown = RowTable::open_with(&files, (meta, &types()), TableOptions::default(), sealing()).unwrap();
    assert!(unknown.gathered.get().is_none());
    assert!(unknown.statistics().is_some() && unknown.gathered.passes() == 1, "gathered as asked");
}

/// Verifies: [DESIGN] RULES.md D1093, a statistics page that is not what
/// was saved: one of another format, of other columns than the table's,
/// cut short, with bytes left over, or sealed under another page's
/// context, is refused (SQLCODE -916), and statistics of other columns
/// are not written (SQLCODE -910); the table then opens with no
/// statistics, and gathers them as asked, no failure of statistics
/// failing its opening.
#[test]
fn a_malformed_statistics_page_is_read_as_none() {
    let (_, files) = paged();
    let table = table_of(&files, 200);
    let stats = table.gather().unwrap();
    let format = RecordFormat::new(&types());
    let content = encoded(&stats, &format).unwrap();
    assert_eq!(decoded(&content, &format).map(|s| shown(&s)).unwrap(), shown(&stats));
    let code = |bytes: &[u8], of: &RecordFormat| decoded(bytes, of).err().map(|e| e.code);
    let mut other = content.clone();
    other[0] = 9;
    assert_eq!(code(&other, &format), Some(Code::Corrupt), "another format");
    let fewer = RecordFormat::new(&types()[..2]);
    assert_eq!(code(&content, &fewer), Some(Code::Corrupt), "other columns");
    assert_eq!(code(&content[..content.len().saturating_sub(3)], &format), Some(Code::Corrupt), "cut short");
    assert_eq!(code(&[content.as_slice(), &[0]].concat(), &format), Some(Code::Corrupt), "bytes left over");
    assert_eq!(encoded(&stats, &fewer).err().map(|e| e.code), Some(Code::Api));
    files.save_statistics().unwrap();
    let (meta, page) = (table.meta(), PageId(table.gathered.page()));
    drop(table);
    assert_ne!(page.0 | STATISTICS, page.0, "a context of its own");
    for stored in [other, vec![255_u8], Vec::new()] {
        *files.pool.write(files.file, page).unwrap() = stored;
        let malformed = opened(&files, meta);
        assert!(malformed.gathered.get().is_none());
        assert!(malformed.statistics().is_some() && malformed.gathered.passes() == 1);
    }
}
