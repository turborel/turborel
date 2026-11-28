//! Unit tests of a LIKE judged of texts coded by symbols in place
//! (`coded.rs`, RULES.md D1251).

use std::borrow::Cow;
use std::sync::Arc;

use super::super::super::group::{Group, Pages, Seal};
use super::super::super::prepared::Prepared;
use super::super::super::{Chunk, Fields};
use super::super::Judgment;
use super::*;
use crate::disk::buffer::{BufferPool, FileId};
use crate::engine::{MEMORY_POOL_BYTES, memory_file};
use crate::error::Result;
use crate::index::Probe;
use crate::memory::MemoryBudget;
use crate::storage::{Condition, Conjunct, Conjunction, Row};
use crate::value::{CompOp, DataType, Decimal, LikePattern, Spelled, Value};

const INTEGER: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
const URL: DataType = DataType::varying(100);
const TYPES: [DataType; 2] = [INTEGER, URL];
const ROWS: u32 = 7000;

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
    memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 4_i32), "a test").unwrap()
}

/// The pages of `pool`'s `file`, chunks compressed if `compress`.
fn pages(pool: &BufferPool, file: FileId, compress: bool) -> Pages<'_, Plain> {
    Pages {
        pool,
        file,
        capacity: 8192,
        sealing: &Plain,
        compress,
        pax: false,
        imprints: false,
        kernels: &crate::kernels::Portable,
        base: 0,
        sink: None,
    }
}

/// A web address a row, every nineteenth null, every fourth one address.
fn url(n: u32) -> Value {
    if n.is_multiple_of(19) {
        return Value::Null;
    }
    if n.is_multiple_of(4) {
        return Value::Char("http://often.example/".into());
    }
    Value::Char(format!("http://site{}.example/page{}", n.wrapping_mul(7919) % 300, n.wrapping_mul(104_729) % 50_000).into())
}

/// One of five words a row, every nineteenth null: a dictionary's.
fn word(n: u32) -> Value {
    if n.is_multiple_of(19) {
        return Value::Null;
    }
    Value::Char(["red", "green", "blue", "a green one", "grey"][index(n % 5)].into())
}

/// A group of `ROWS` rows of a number and the text `text` gives, of `types`.
fn group(pages: &Pages<'_, Plain>, types: &[DataType], text: fn(u32) -> Value) -> (Group, Vec<Row>) {
    let ids: Vec<u64> = (1..=u64::from(ROWS)).collect();
    let rows: Vec<Row> = (0..ROWS).map(|n| vec![Value::Exact(Decimal::new(i128::from(n), 0)), text(n)]).collect();
    (Group::write(pages, types, (&ids, &rows), (&[], &[], &[])).unwrap(), rows)
}

/// `LIKE pattern` of column 1, or NOT LIKE if `not`, as a scan is told it.
fn like(pattern: &str, not: bool) -> Conjunct {
    Conjunct::Like { column: 1, like: Arc::new(Liked::of(&LikePattern::new(pattern, None).unwrap(), not).unwrap()) }
}

/// The rows each run of `group` keeps by `judgment`, of every row and of
/// every third, and the columns decoded to judge them, counted.
fn judged(judgment: &Judgment, group: &Group, pages: &Pages<'_, Plain>, types: &[DataType]) -> (Vec<Vec<u32>>, usize, Prepared) {
    let mut prepared = Prepared::default();
    let (mut out, mut decoded) = (Vec::new(), 0_usize);
    for from in (0..ROWS).step_by(2048) {
        let to = from.saturating_add(2048).min(ROWS);
        for given in [None, Some((0..to.saturating_sub(from)).step_by(3).collect::<Vec<u32>>())] {
            let run = judgment.judge(group, (pages, &mut prepared), (types, (from, to)), given).unwrap();
            decoded = decoded.saturating_add(run.decoded.len());
            out.push(run.kept);
        }
    }
    (out, decoded, prepared)
}

/// The rows of each run, of every row and of every third, of `rows`
/// that `keeps` is true of.
fn expected(rows: &[Row], keeps: &dyn Fn(&Row) -> bool) -> Vec<Vec<u32>> {
    let mut out = Vec::new();
    for from in (0..ROWS).step_by(2048) {
        let to = from.saturating_add(2048).min(ROWS);
        let kept = |r: &u32| keeps(&rows[index(from.saturating_add(*r))]);
        out.push((0..to.saturating_sub(from)).filter(kept).collect());
        out.push((0..to.saturating_sub(from)).step_by(3).filter(kept).collect());
    }
    out
}

/// Asserts what a LIKE kept of each run, `kept`, against the rows the
/// predicate is true of, `wanted`, each list of a run of `ROWS` rows read
/// 2,048 at a time, of every row and then of every third. Of a run that
/// keeps fewer than one row in sixteen the two are the same. Of any
/// other the rows kept hold every row wanted, as a run seen to be given
/// whole keeps rows unjudged, and of one that keeps a row in four or
/// more they hold more, if `nulls` says the run has nulls to keep so.
fn kept_as_wanted(kept: &[Vec<u32>], wanted: &[Vec<u32>], nulls: bool, what: &str) {
    assert_eq!(kept.len(), wanted.len(), "{what}");
    for (n, (given, true_of)) in kept.iter().zip(wanted).enumerate() {
        let from = u32::try_from(n.checked_div(2).unwrap()).unwrap().saturating_mul(2048);
        let rows = index(ROWS.saturating_sub(from).min(2048));
        if true_of.len().saturating_mul(16) < rows {
            assert_eq!(given, true_of, "{what}, run {n}: few rows kept, each judged");
            continue;
        }
        assert!(true_of.iter().all(|r| given.binary_search(r).is_ok()), "{what}, run {n}: every row wanted is kept");
        assert!(given.is_sorted() && given.windows(2).all(|pair| pair[0] != pair[1]), "{what}, run {n}: ascending, each once");
        if nulls && true_of.len().saturating_mul(4) >= rows {
            assert!(given.len() > true_of.len(), "{what}, run {n}: a run given whole is not judged to its end");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1251 (MC/DC): a run is seen to be given
/// whole once at least one in eight of the rows asked are judged and
/// the rate kept of them, carried to all the rows asked, is one row in
/// eight of the run or more. Fewer rows judged tell nothing, though
/// every one was kept; a rate under the share does not, at the share it
/// does; and of rows an earlier conjunct left, the share is of the run,
/// so nearly every row asked must be kept.
#[test]
fn a_run_is_seen_whole_by_the_rate_kept_of_a_share_of_it() {
    assert!(!whole_by((255, 255), (2048, 2048)), "under one in eight judged, all kept");
    assert!(whole_by((256, 256), (2048, 2048)), "one in eight judged, all kept");
    assert!(!whole_by((31, 256), (2048, 2048)), "one in eight judged, under one in eight of them kept");
    assert!(whole_by((32, 256), (2048, 2048)), "one in eight of them kept");
    assert!(whole_by((300, 2048), (2048, 2048)) && !whole_by((200, 2048), (2048, 2048)), "judged to the end");
    assert!(whole_by((32, 32), (256, 2048)), "an eighth of the run asked, every row of an eighth of those kept");
    assert!(!whole_by((31, 32), (256, 2048)), "one short of it");
}

/// Verifies: [X3.135-1986 §5.14 GR4, GR5] (RULES.md D1251) a scan
/// judges a LIKE and a NOT LIKE of a group of texts coded by symbols in
/// place: of each run that keeps few rows, of every row and of every
/// third, it keeps the rows whose text the predicate is true of and no
/// null, of a piece between, a first piece, a last, several pieces, a
/// pattern of no `%`, one nothing matches, `%` alone, and the empty
/// pattern, which the empty string alone matches and so no null, whose
/// codes are none; and of a run
/// that keeps one row in eight or more, which is given whole, it stops
/// judging and keeps the rows left, nulls among them. No column is
/// decoded to judge them, and the steps are made once for the group,
/// whose chunks are many and coded by one table. By its zones a LIKE
/// passes over no part of the group, whatever its pattern.
#[test]
fn a_like_is_judged_of_texts_coded_by_symbols_in_place() {
    let (pool, file) = file();
    let pages = pages(&pool, file, true);
    let (group, rows) = group(&pages, &TYPES, url);
    assert!(group.zones(1).unwrap().unwrap().len() > 8, "many chunks");
    let told = Conjunction(vec![like("zz%", false)]);
    assert_eq!(super::super::super::scan::candidates(&group, Some(&told)).unwrap(), [(0, ROWS)], "a LIKE passes over no part");
    let patterns =
        ["%site12%", "http://site1%", "%page4", "%site1%page%77", "%often%", "http://often.example/", "%nothing%", "%", "h%e%77", ""];
    for (pattern, not) in patterns.into_iter().flat_map(|p| [(p, false), (p, true)]) {
        let conjunct = like(pattern, not);
        let judgment = Judgment::of(Some(&Conjunction(vec![conjunct.clone()])), &TYPES);
        let (kept, decoded, prepared) = judged(&judgment, &group, &pages, &TYPES);
        let wanted = expected(&rows, &|row| conjunct.matches_key(&[&row[1]]));
        kept_as_wanted(&kept, &wanted, true, &format!("{pattern:?}, NOT {not}"));
        assert_eq!((decoded, prepared.stepped.made), (0, 1), "{pattern:?}: nothing decoded, the steps made once");
    }
}

/// Verifies: [DESIGN] RULES.md D1251: a LIKE is judged after the scan's
/// other conjuncts, whatever their order as written or as their zones
/// order them, of the rows they kept, and its column is not decoded for
/// it.
#[test]
fn a_like_is_judged_after_the_other_conjuncts() {
    let (pool, file) = file();
    let pages = pages(&pool, file, true);
    let (group, rows) = group(&pages, &TYPES, url);
    let below = Conjunct::Condition(Condition {
        columns: vec![0],
        probe: Probe::Compare { op: CompOp::Lt, value: Value::Exact(Decimal::new(3000, 0)) },
    });
    let conjunct = like("%site1%", false);
    let mut judgment = Judgment::of(Some(&Conjunction(vec![conjunct.clone(), below.clone()])), &TYPES);
    assert_eq!(judgment.preds.iter().map(|p| p.column).collect::<Vec<_>>(), [0, 1], "the LIKE last");
    judgment.order_by(&[0, 1]);
    assert_eq!(judgment.preds.iter().map(|p| p.column).collect::<Vec<_>>(), [0, 1], "and last however zones order them");
    let (kept, _, prepared) = judged(&judgment, &group, &pages, &TYPES);
    let wanted = expected(&rows, &|row| below.matches_key(&[&row[0]]) && conjunct.matches_key(&[&row[1]]));
    kept_as_wanted(&kept, &wanted, false, "after a comparison");
    assert_eq!(prepared.stepped.made, 1);
    // Of a run the comparison cuts: one whose every row it keeps is
    // proved by its parts' bounds and decodes nothing (RULES.md D1293).
    let run = judgment.judge(&group, (&pages, &mut Prepared::default()), (&TYPES, (2048, 4096)), None).unwrap();
    assert_eq!(run.decoded.iter().map(|(column, _)| *column).collect::<Vec<_>>(), [0], "the number's column alone is decoded");
    let none = judgment.judge(&group, (&pages, &mut Prepared::default()), (&TYPES, (4096, 6144)), None).unwrap();
    assert!(none.kept.is_empty(), "no row at or past 3000 is given the LIKE");
}

/// The one text of every row not null, every nineteenth null.
fn same(n: u32) -> Value {
    if n.is_multiple_of(19) { Value::Null } else { Value::Char("a green one, the same in every row".into()) }
}

/// A text of its own a row, each as long as the others, none null.
fn of_one_length(n: u32) -> Value {
    Value::Char(format!("item {n:05} is {}", ["green", "black", "white"][index(n % 3)].chars().rev().collect::<String>()).into())
}

/// A table of texts not coded by symbols: what it is, whether its chunks
/// are compressed, and its texts.
type Case = (&'static str, bool, fn(u32) -> Value);

/// Verifies: [X3.135-1986 §5.14 GR4, GR5] (RULES.md D1251) a scan
/// judges a LIKE and a NOT LIKE of a VARCHAR column's chunks not coded
/// by symbols in place too, by the octets of each text as it was
/// written: of a dictionary's chunks, of chunks not compressed, of
/// texts of one length, and of a chunk of one text. Of a run that keeps
/// few rows it keeps the rows the predicate is true of and no null, and
/// of any other every one of them; it decodes no column and makes no
/// steps.
#[test]
fn a_like_is_judged_of_texts_kept_as_written() {
    let (pool, file) = file();
    let cases: [Case; 5] = [
        ("a dictionary", true, word),
        ("not compressed", false, url),
        ("few strings not compressed", false, word),
        ("of one length", false, of_one_length),
        ("one text", true, same),
    ];
    for (what, compress, text) in cases {
        let pages = pages(&pool, file, compress);
        let (group, rows) = group(&pages, &TYPES, text);
        for conjunct in [like("%green%", false), like("%site1%", true), like("item 0%neerg", false), like("%one%same%", true)] {
            let judgment = Judgment::of(Some(&Conjunction(vec![conjunct.clone()])), &TYPES);
            let (kept, decoded, prepared) = judged(&judgment, &group, &pages, &TYPES);
            let wanted = expected(&rows, &|row| conjunct.matches_key(&[&row[1]]));
            kept_as_wanted(&kept, &wanted, false, &format!("{what}: {conjunct:?}"));
            assert_eq!((decoded, prepared.stepped.made), (0, 0), "{what}: nothing decoded, no steps made");
        }
    }
}

/// Verifies: [DESIGN] RULES.md D1251: a LIKE of a CHARACTER column,
/// whose strings are padded as they are read, is not judged by the
/// scan, which gives every row for the filter to judge. A chunk coded
/// by symbols whose codes are malformed where they are read is not
/// judged either, though it is where they are not read, and every row
/// asked of it is then kept, once, in the place of those judged before
/// the malformed text.
#[test]
fn a_like_gives_way_of_padded_strings_and_malformed_codes() {
    let (pool, file) = file();
    let pages = pages(&pool, file, true);
    let types = [INTEGER, DataType::character(30)];
    let (group, _) = group(&pages, &types, word);
    let judgment = Judgment::of(Some(&Conjunction(vec![like("%green", false)])), &types);
    assert_eq!(judgment.preds.len(), 1, "told and taken");
    let (kept, decoded, prepared) = judged(&judgment, &group, &pages, &types);
    let every: Vec<u32> = (0..2048).collect();
    assert_eq!(kept[0], every, "every row given");
    assert_eq!(kept[1], every.iter().copied().step_by(3).collect::<Vec<_>>(), "every row asked for given");
    assert_eq!((decoded, prepared.stepped.made), (0, 0), "nothing decoded, no steps made");
    let texts: Vec<Value> = (0..300_u32).map(|k| Value::Char(format!("customer{k}@mail{}.example.com", k % 7).into())).collect();
    let fields = Fields::of(&URL, texts.iter().cloned()).unwrap();
    let mut bytes = fields.chunk(&URL, 0, (fields.len(), true)).unwrap();
    let last = bytes.len() - 1;
    bytes[last] = 255;
    let chunk = Chunk::parse(&bytes).unwrap();
    assert!(matches!(chunk.body, Body::Fsst(_)), "coded by symbols");
    let Conjunct::Like { like: within, .. } = like("%mail3%", false) else { panic!("not a LIKE") };
    let (mut stepped, mut out) = (Stepped::default(), vec![5]);
    assert_eq!(liked_of(&chunk, (0, &within, &mut stepped), (0, 0), &[297, 298], &mut out), Some(()));
    assert_eq!(out, [5, 297], "customer297@mail3, judged before the malformed text");
    let gave = liked_of(&chunk, (0, &within, &mut stepped), (0, 0), &[297, 298, 299], &mut out);
    assert_eq!((gave, out.as_slice()), (None, [5, 297, 297].as_slice()), "an escape at the end, a row pushed before it");
    or_all(gave, &[297, 298, 299], (2, &mut out));
    assert_eq!(out, [5, 297, 297, 298, 299], "every row of the chunk, once");
    or_all(Some(()), &[300], (5, &mut out));
    assert_eq!(out, [5, 297, 297, 298, 299], "a chunk judged is as it was judged");
}
