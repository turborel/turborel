//! Unit tests of the fulltext method: what it supports, its options, bulk
//! loads, locators on a file pager, threads inserting at once, and a
//! dropped index's pages (RULES.md R-TEST-1, R-TEST-2).

use std::path::Path;
use std::thread;

use super::*;
use crate::catalog::QualifiedName;
use crate::disk::buffer::Clock;
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::{DEFAULT_PAGE_SIZE, Pager};
use crate::engine::rows::ROW_STORE;
use crate::fulltext::matches::TextMatches;
use crate::functions::ScalarRef;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::spill::testing::{Held, areas};
use crate::value::lob::LobKind;
use crate::value::{CharUnits, Spelled};

/// A pager file in memory, in a pool of its own.
fn pager() -> (Arc<Pager>, Arc<BufferPool>, FileId) {
    let hash: Arc<dyn crate::hash::HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    let pager = Arc::new(Pager::create(&MemoryFileSystem::new(), Path::new("/p"), hash, Pipeline::none(), DEFAULT_PAGE_SIZE).unwrap());
    let pool = Arc::new(BufferPool::new(64, Box::new(Clock::new(64))).unwrap());
    let file = pool.add_pager(Arc::clone(&pager));
    (pager, pool, file)
}

/// The descriptor of an index setting `options`.
pub(super) fn described(options: Vec<(String, Value)>) -> (TableDesc, IndexDesc) {
    let name = QualifiedName { auth: "A".into(), ident: "T".into() };
    let table = TableDesc {
        periods: Vec::new(),
        typed: None,
        name: name.clone(),
        columns: vec![],
        constraints: vec![],
        engine: ROW_STORE.into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    let index = IndexDesc {
        name: "T_D".into(),
        table: name,
        columns: vec![0],
        method: FULLTEXT_METHOD.into(),
        unique: false,
        options,
        collations: Vec::new(),
        nulls: false,
        expressions: Vec::new(),
    };
    (table, index)
}

/// Documents of rows, some of two versions, one null, one long.
fn documents() -> Vec<(Key, RowId)> {
    let mut out = Vec::new();
    for row in 0..400_u64 {
        out.push((doc(&format!("word{} shared the text{} shared", row % 37, row % 11)), row));
        if row % 7 == 0 {
            out.push((doc(&format!("word{} shared the text{} shared", row % 37, row % 11)), row));
        }
    }
    out.push((doc(&"many ".repeat(20_000)), 500));
    out.push((Key(vec![Value::Null]), 501));
    out.push((doc("later version"), 3));
    out
}

/// Verifies: [DESIGN] RULES.md D86, D31, `Index::load`: an index built
/// whole from entries in any order, its records spilled and sorted, holds
/// exactly the tree an index inserted one entry at a time in row order
/// holds, documents numbered alike, versions counted, full blocks, chunks,
/// terms, stems and totals alike; and searches it alike.
#[test]
fn a_load_equals_inserting_one_by_one() {
    let (loaded, inserted) = (index(), index());
    let mut ordered = documents();
    let hashed = |key: &Key| document::hash_in(key, &english()).unwrap();
    ordered.sort_by_key(|(key, row)| (*row, hashed(key)));
    for (key, row) in ordered {
        inserted.insert(key, row).unwrap();
    }
    let work = areas(Some(Arc::new(Held { broken: false })), MEMORY_POOL_BYTES << 4_i32);
    let mut entries = documents().into_iter().rev().map(Ok);
    loaded.load(&mut entries, (&work, Area::new(4_096, 64))).unwrap();
    assert_eq!(dump(&loaded), dump(&inserted));
    assert_eq!(loaded.totals().unwrap().documents, 402);
    let empty = index();
    empty.load(&mut std::iter::empty(), (&work, Area::new(4_096, 64))).unwrap();
    assert_eq!(left(&empty), Vec::new());
}

/// Verifies: [DESIGN] RULES.md D86, `Index::load` failing: an entry's
/// error and a spill area's are given, not swallowed.
#[test]
fn a_load_gives_its_errors() {
    let work = areas(Some(Arc::new(Held { broken: false })), MEMORY_POOL_BYTES << 4_i32);
    let failing = SqlError::new(Code::Io, "[DESIGN] told to fail");
    let mut entries = vec![Ok((doc("apple"), 1)), Err(failing)].into_iter();
    assert_eq!(index().load(&mut entries, (&work, Area::new(4_096, 64))).unwrap_err().code, Code::Io);
    let broken = areas(Some(Arc::new(Held { broken: true })), MEMORY_POOL_BYTES << 4_i32);
    let mut all = documents().into_iter().map(Ok);
    assert_eq!(index().load(&mut all, (&broken, Area::new(1_024, 8))).unwrap_err().code, Code::Io);
}

/// A function of another name, which the method does not answer.
struct Other;

impl crate::functions::ScalarFunction for Other {
    fn name(&self) -> &'static str {
        "OTHER_MATCHES"
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }
    fn call(&self, _args: &[Value]) -> Result<Value> {
        Ok(Value::Null)
    }
}

/// Verifies: [EXTENSION] RULES.md D34, D86, what the method supports:
/// TEXT_MATCHES of one character string column as its first argument, a
/// CLOB too; not its second argument, another function, another type, two
/// columns, nor any comparison; it is not ordered and restores no key.
#[test]
fn the_method_supports_text_matches_of_its_column() {
    let method = FullTextMethod::in_memory();
    let predicate = |argument: usize| Predicate { function: ScalarRef(Arc::new(TextMatches)), column: 0, argument: Some(argument) };
    let clob = DataType::Lob { kind: LobKind::Character, length: 1_000, units: CharUnits::Characters };
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    assert!(method.supports_predicate(&predicate(0), &[DataType::varying(10)]));
    assert!(method.supports_predicate(&predicate(0), std::slice::from_ref(&clob)));
    assert!(method.supports_predicate(&predicate(0), &[DataType::character(4)]));
    assert!(!method.supports_predicate(&predicate(1), &[DataType::varying(10)]));
    assert!(!method.supports_predicate(&predicate(0), std::slice::from_ref(&integer)));
    assert!(!method.supports_predicate(&predicate(0), &[clob.clone(), clob]));
    let other = Predicate { function: ScalarRef(Arc::new(Other)), column: 0, argument: Some(0) };
    assert!(!method.supports_predicate(&other, &[DataType::varying(10)]));
    assert!([CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Gt, CompOp::Le, CompOp::Ge].iter().all(|op| !method.supports(*op)));
    assert!(!method.ordered() && !method.restores_keys());
    assert_eq!(method.name(), FULLTEXT_METHOD);
    assert_eq!(method.create(&[integer]).err().unwrap().code, Code::Unsupported);
    assert_eq!(method.create(&[]).err().unwrap().code, Code::Unsupported);
}

/// Verifies: [EXTENSION] RULES.md D41, its options: FULLTEXT.K1 and
/// FULLTEXT.B, numbers in range, change in place (`reconfigure`);
/// FULLTEXT.LANGUAGE by a rebuild, an unknown one refused as a bad
/// option's value is, SQLCODE -103, naming those known; defaults 1.2, 0.75 and ENGLISH; NULLS NOT DISTINCT refused.
#[test]
fn options_set_the_weights_and_language() {
    let method = FullTextMethod::in_memory();
    let declared = method.index_options();
    let names: Vec<&str> = declared.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, [K1_OPTION, B_OPTION, RANKING_OPTION, LANGUAGE_OPTION, STOP_WORDS_OPTION]);
    let column = [DataType::varying(100)];
    let (table, desc) = described(Vec::new());
    let made: Arc<dyn Index> = Arc::from(method.create_on(&column, (&table, &desc)).unwrap());
    let set = vec![(B_OPTION.to_owned(), Value::Approx(0.5_f64)), (K1_OPTION.to_owned(), Value::Approx(2.0_f64))];
    made.reconfigure((&table, &described(set.clone()).1));
    let (_, none) = described(vec![(LANGUAGE_OPTION.to_owned(), Value::Char("none".into()))]);
    let plain = method.create_on(&column, (&table, &none)).unwrap();
    let (_, unknown) = described(vec![(LANGUAGE_OPTION.to_owned(), Value::Char("KLINGON".into()))]);
    let refused = method.create_on(&column, (&table, &unknown)).err().unwrap();
    assert_eq!(refused.code, Code::SyntaxRule, "a bad option's value, not a runtime language");
    assert!(refused.msg.contains("NONE, ENGLISH, ARABIC"), "{}", refused.msg);
    let (_, wide) = described(vec![(B_OPTION.to_owned(), Value::Approx(2.0_f64))]);
    assert_eq!(method.create_on(&column, (&table, &wide)).err().unwrap().code, Code::SyntaxRule);
    let (_, mut nulls) = described(Vec::new());
    nulls.nulls = true;
    assert_eq!(method.create_on(&column, (&table, &nulls)).err().unwrap().code, Code::Unsupported);
    let index = index();
    assert_eq!(index.weights(), Weights { k1: DEFAULT_K1, b: DEFAULT_B });
    index.reconfigure((&table, &described(set).1));
    assert_eq!(index.weights(), Weights { k1: 2.0_f64, b: 0.5_f64 });
    assert_eq!((index.language(), located(&plain.locator()).unwrap().1.language == Language::None), (Language::English, true));
}

/// Verifies: [DESIGN] RULES.md D86, `IndexMethod::open`: a checkpointed
/// index on a file pager opens by its locator with its entries, counts
/// and language; a locator cut short, of another version, of an unknown
/// language or with bytes after is refused, SQLCODE -916 or 2FF02.
#[test]
fn an_index_opens_by_its_locator() {
    let (_, pool, file) = pager();
    let method = FullTextMethod::new(pool, file);
    let column = [DataType::varying(100)];
    let (table, desc) = described(vec![(LANGUAGE_OPTION.to_owned(), Value::Char("NONE".into()))]);
    let made: Arc<dyn Index> = Arc::from(method.create_on(&column, (&table, &desc)).unwrap());
    made.insert(doc("the apple"), 1).unwrap();
    made.insert(doc("the apple"), 1).unwrap();
    let locator = made.locator();
    method.checkpoint().unwrap();
    drop(made);
    let opened: Arc<dyn Index> = Arc::from(method.open_on(&column, &locator, (&table, &desc)).unwrap().unwrap());
    opened.remove(&doc("the apple"), 1).unwrap();
    assert_eq!(opened.rows(&doc("the apple")).unwrap(), [1], "one version left");
    assert_eq!(opened.estimate(&Probe::All), Some(1));
    let reopened = method.open(&column, &locator).unwrap().unwrap();
    assert_eq!(reopened.locator(), locator, "its language kept by the locator");
    for length in 0..locator.len() {
        assert!(method.open(&column, &locator[..length]).is_err(), "cut at {length}");
    }
    let mut version = locator.clone();
    version[0] += 1;
    assert_eq!(method.open(&column, &version).err().unwrap().code, Code::Corrupt);
    let mut trailing = locator.clone();
    trailing.push(0);
    assert_eq!(method.open(&column, &trailing).err().unwrap().code, Code::Corrupt);
    assert_eq!(method.open(&[DataType::Boolean], &locator).err().unwrap().code, Code::Unsupported);
}

/// The work of thread `t` of `threads_inserting_at_once_lose_no_count`:
/// its rows' documents inserted, or removed if `remove`.
fn changes(index: Arc<FullTextIndex>, t: u64, remove: bool) -> impl FnOnce() + Send {
    move || {
        for row in t.saturating_mul(EACH)..t.saturating_add(1).saturating_mul(EACH) {
            let key = doc(&format!("common word{row}"));
            let changed = if remove { index.remove(&key, row) } else { index.insert(key, row) };
            changed.unwrap();
        }
    }
}

/// Threads changing an index at once.
const THREADS: u64 = 8;
/// The rows each changes.
const EACH: u64 = 150;

/// Verifies: [DESIGN] RULES.md D86, threads: eight threads inserting
/// documents of different rows at once, every one holding a shared word,
/// lose no count: the word's documents, its stem's count, and the totals
/// are all of them; then all removed at once leave nothing.
#[test]
fn threads_inserting_at_once_lose_no_count() {
    let index = index();
    let work = |remove: bool| {
        let handles: Vec<_> = (0..THREADS).map(|t| thread::spawn(changes(Arc::clone(&index), t, remove))).collect();
        for handle in handles {
            handle.join().unwrap();
        }
    };
    work(false);
    let all = THREADS * EACH;
    assert_eq!(index.statistics("common").unwrap(), TermStatistics { documents: all, most: 1 });
    assert_eq!(index.totals().unwrap(), Totals { documents: all, length: 2 * all });
    let stem = stem_key(&index.stem_of(b"common").unwrap(), b"common");
    assert_eq!(keys::count_of(&index.tree.get(&stem).unwrap().unwrap()).unwrap(), all);
    work(true);
    assert_eq!(left(&index), Vec::new());
}

/// Verifies: [DESIGN] RULES.md D86, D34, a dropped index: its pages are
/// freed when the last reference to it goes, not when it is discarded, so
/// a search under way reads on; an index not discarded keeps them.
#[test]
fn a_discarded_index_frees_its_pages_when_it_goes() {
    let (pages, pool, file) = pager();
    let in_use = || pages.page_count() - pages.free_pages();
    let before = in_use();
    let insert_rows = |index: &FullTextIndex| (0..800).for_each(|row| index.insert(doc(&format!("apple pear{row}")), row).unwrap());
    let kept = FullTextIndex::with(BLinkTree::create(Arc::clone(&pool), file).unwrap(), (english(), Weights::default()), spill(TEST_AREA));
    insert_rows(&kept);
    let used = in_use();
    drop(kept);
    assert_eq!(in_use(), used, "not discarded: kept");
    let dropped = Arc::new(FullTextIndex::with(
        BLinkTree::create(Arc::clone(&pool), file).unwrap(),
        (english(), Weights::default()),
        spill(TEST_AREA),
    ));
    insert_rows(&dropped);
    assert!(dropped.tree.height().unwrap() > 1);
    let full = in_use();
    let mut cursor = Arc::clone(&dropped).search(&Probe::All, KeyOrder::Ascending);
    dropped.discard();
    drop(dropped);
    assert_eq!(in_use(), full, "a search still holds it");
    assert_eq!(cursor.by_ref().count(), 800);
    drop(cursor);
    assert_eq!(full - in_use(), used - before, "every page of the dropped tree freed");
}

/// Verifies: [EXTENSION] RULES.md D85, `FullTextMethod::projection`: the
/// projection a descriptor gives before its index is made projects every
/// document as the index made from it does, of its language: "the apple"
/// projects alike for one language and not across ENGLISH and NONE, as
/// "the" is a stop word of one; an unknown language is refused.
#[test]
fn a_descriptor_projects_as_its_index_does() {
    let method = FullTextMethod::in_memory();
    let column = [DataType::varying(100)];
    let keys = [doc("the apple"), doc("Apple pie"), Key(vec![Value::Null]), Key(vec![Value::Lob(Lob::of_text("the apple"))])];
    let mut projected = Vec::new();
    for options in [Vec::new(), vec![(LANGUAGE_OPTION.to_owned(), Value::Char("NONE".into()))]] {
        let (table, desc) = described(options);
        let early = FullTextMethod::projection(&desc).unwrap();
        let made = method.create_on(&column, (&table, &desc)).unwrap().key_projection().unwrap();
        for key in &keys {
            assert_eq!(early.project(key).unwrap(), made.project(key).unwrap(), "{key:?}");
        }
        projected.push(early.project(&keys[0]).unwrap());
    }
    assert_ne!(projected[0], projected[1], "the languages' projections differ");
    let (_, unknown) = described(vec![(LANGUAGE_OPTION.to_owned(), Value::Char("KLINGON".into()))]);
    assert!(FullTextMethod::projection(&unknown).is_err());
}

/// Verifies: [DESIGN] RULES.md D38, `FullTextMethod::with_work`: a method
/// never attached, given work areas, spills its documents' places to
/// them; its indexes post and find documents as an attached method's do.
#[test]
fn a_method_with_work_areas_needs_no_attach() {
    let (_, pool, file) = pager();
    let work = areas(Some(Arc::new(Held { broken: true })), MEMORY_POOL_BYTES);
    let method = FullTextMethod::with_work(pool, file, work);
    let column = [DataType::varying(100)];
    let (table, desc) = described(Vec::new());
    let made: Arc<dyn Index> = Arc::from(method.create_on(&column, (&table, &desc)).unwrap());
    made.insert(doc("apple pie"), 1).unwrap();
    assert_eq!(made.rows(&doc("apple pie")).unwrap(), [1]);
    let long = doc(&"apple ".repeat(1_000_000));
    assert_eq!(made.insert(long, 2).unwrap_err().code, Code::Io, "a document past the area spills to the areas given");
    assert_eq!(made.rows(&doc(&"apple ".repeat(1_000_000))).unwrap(), Vec::<RowId>::new(), "a failed insert leaves nothing");
    assert_eq!(made.estimate(&Probe::All), Some(1));
}

/// Verifies: [EXTENSION] RULES.md D86: an index on a column with a
/// collation, keyed by its sort key, not its text, is refused, SQLCODE
/// -900, the message naming why and the remedy.
#[test]
fn a_collated_column_is_refused_with_its_remedy() {
    let method = FullTextMethod::in_memory();
    let (table, mut desc) = described(Vec::new());
    let collation = crate::value::collation::builtin().remove(0);
    desc.collations = vec![Some(crate::value::collation::CollationRef { collation, pad: crate::value::collation::Pad::Space })];
    let error = method.create_on(&[DataType::VarBinary { length: 100 }], (&table, &desc)).err().unwrap();
    assert_eq!(error.code, Code::Unsupported);
    assert!(error.msg.contains("sort key") && error.msg.contains("without a COLLATE clause"), "{}", error.msg);
}
