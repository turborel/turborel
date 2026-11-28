//! Unit tests of a comparison a run's parts prove (`proved.rs`, RULES.md
//! D1293).

use std::borrow::Cow;
use std::sync::Arc;

use super::super::super::group::{Group, Pages, Seal, Zone};
use super::super::super::prepared::Prepared;
use super::super::Judgment;
use super::*;
use crate::disk::buffer::{BufferPool, FileId};
use crate::engine::{MEMORY_POOL_BYTES, memory_file};
use crate::index::Probe;
use crate::memory::MemoryBudget;
use crate::storage::mask::index;
use crate::storage::{Column, Condition, Conjunct, Conjunction, Datetimes, Row};
use crate::value::datetime::Moment;
use crate::value::{Decimal, Spelled};

const INTEGER: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
/// K, a number of its row; V, the same, null at row `NULL_ROW`; C, one
/// number; D, a day every hundred rows; T, a text.
const TYPES: [DataType; 5] = [INTEGER, INTEGER, INTEGER, DataType::Date, DataType::varying(20)];
const ROWS: u32 = 20_000;
const NULL_ROW: u32 = 6000;
const ONE: i128 = 7;
const OTHER: i128 = 8;

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

fn pages(pool: &BufferPool, file: FileId) -> Pages<'_, Plain> {
    Pages {
        pool,
        file,
        capacity: 8192,
        sealing: &Plain,
        compress: true,
        pax: false,
        imprints: false,
        kernels: &crate::kernels::Portable,
        base: 0,
        sink: None,
    }
}

fn number(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The day `n` days after the first, as a DATE column holds it.
fn day(n: u32) -> Value {
    let days = Datetimes::new(vec![i64::from(n).saturating_mul(86_400_000_000)], None, (Moment::Date, false, 0), None);
    Column::Datetimes(days).values()[0].clone()
}

/// `a + b` and `a - b` of rows' places.
fn add(a: u32, b: u32) -> u32 {
    a.checked_add(b).unwrap()
}

fn sub(a: u32, b: u32) -> u32 {
    a.checked_sub(b).unwrap()
}

/// The hundred row `n` is in.
fn hundreds(n: u32) -> u32 {
    n.checked_div(100).unwrap()
}

/// A group of `ROWS` rows of `TYPES`.
fn group(pages: &Pages<'_, Plain>) -> (Group, Vec<Row>) {
    let ids: Vec<u64> = (1..=u64::from(ROWS)).collect();
    let rows: Vec<Row> = (0..ROWS)
        .map(|n| {
            let v = if n == NULL_ROW { Value::Null } else { number(i128::from(n)) };
            vec![number(i128::from(n)), v, number(ONE), day(hundreds(n)), Value::Char(format!("text {n:05}").into())]
        })
        .collect();
    (Group::write(pages, &TYPES, (&ids, &rows), (&[], &[], &[])).unwrap(), rows)
}

/// `column op value`, as a scan is told it.
fn compared(column: usize, op: CompOp, value: Value) -> Conjunct {
    Conjunct::Condition(Condition { columns: vec![column], probe: Probe::Compare { op, value } })
}

/// The conjunct as the scan judges it.
fn pred(conjunct: &Conjunct) -> Pred {
    let column = conjunct.columns()[0];
    Pred { at: 0, column, conjunct: conjunct.clone(), of_null: false }
}

/// Whether the run `run` of `group` proves `column op value`.
fn proves(group: &Group, (column, op, value): (usize, CompOp, Value), run: (u32, u32)) -> bool {
    proved(group, (&pred(&compared(column, op, value)), &TYPES[column]), run).unwrap()
}

/// The first row of the second chunk of `column`, which holds two parts
/// or more.
fn second_chunk(group: &Group, column: usize) -> u32 {
    let zone: &Zone = &group.zones(column).unwrap().unwrap()[1];
    assert!(zone.run.rows >= add(PART_ROWS, PART_ROWS), "a chunk of two parts");
    zone.run.first
}

/// The rows of the part of its chunk that row `row` of `column` is in.
fn part_holding(group: &Group, column: usize, row: u32) -> (u32, u32) {
    let zones = group.zones(column).unwrap().unwrap();
    let zone = zones.iter().find(|z| z.run.first <= row && row < add(z.run.first, z.run.rows)).unwrap();
    let part = sub(row, zone.run.first).checked_div(PART_ROWS).unwrap();
    let from = add(zone.run.first, part.checked_mul(PART_ROWS).unwrap());
    (from, add(from, PART_ROWS).min(add(zone.run.first, zone.run.rows)))
}

/// Verifies: [DESIGN] RULES.md D1293 (MC/DC): of the run of the first two
/// parts of a chunk, which hold K from `b` to `b + 2047`, each comparison
/// is proved at the bound the parts reach and not one unit past it: `>=`
/// at their least and `>` only below it, `<=` at their greatest and `<`
/// only above it; `<` of a constant the first part alone lies below is
/// not proved, as the second does not; `<>` of a constant below the least
/// or above the greatest and of none from one to the other; `=` of a
/// column of one value, by both ends equal, and not of another constant;
/// a constant of another scale compares as its value. A comparison
/// proved is not judged, its column not decoded, and one not proved is
/// judged of the rows as before.
#[test]
fn a_comparison_is_proved_at_the_bounds_its_parts_reach_and_not_past_them() {
    let (pool, file) = file();
    let pages = pages(&pool, file);
    let (group, _) = group(&pages);
    let b = second_chunk(&group, 0);
    let (run, least, greatest) = ((b, add(b, 2048)), i128::from(b), i128::from(add(b, 2047)));
    let (below, above, inside) = (i128::from(sub(b, 1)), i128::from(add(b, 2048)), i128::from(add(b, 700)));
    let k = |op, n: i128| proves(&group, (0, op, number(n)), run);
    assert!(k(CompOp::Ge, least) && !k(CompOp::Ge, i128::from(add(b, 1))), "at least the least");
    assert!(k(CompOp::Gt, below) && !k(CompOp::Gt, least), "above only what lies below the least");
    assert!(k(CompOp::Le, greatest) && !k(CompOp::Le, i128::from(add(b, 2046))), "at most the greatest");
    assert!(k(CompOp::Lt, above) && !k(CompOp::Lt, greatest), "below only what lies above the greatest");
    assert!(!k(CompOp::Lt, i128::from(add(b, 1024))), "the first part lies below it, the second does not");
    assert!(k(CompOp::Ne, below) && k(CompOp::Ne, above), "outside them");
    assert!(!k(CompOp::Ne, least) && !k(CompOp::Ne, greatest) && !k(CompOp::Ne, inside), "from one to the other");
    assert!(!k(CompOp::Eq, inside) && !k(CompOp::Eq, least));
    let one = |op, n: i128| proves(&group, (2, op, number(n)), (0, 2048));
    assert!(one(CompOp::Eq, ONE) && !one(CompOp::Eq, OTHER) && !one(CompOp::Ne, ONE) && one(CompOp::Ne, OTHER));
    let tenths = least.checked_mul(10).unwrap();
    assert!(proves(&group, (0, CompOp::Ge, Value::Exact(Decimal::new(tenths, 1))), run), "the least, of scale 1");
    assert!(!proves(&group, (0, CompOp::Ge, Value::Exact(Decimal::new(tenths.checked_add(1).unwrap(), 1))), run), "a tenth above it");
    let judged = |op, n: i128| {
        let told = Conjunction(vec![compared(0, op, number(n))]);
        let given = Judgment::of(Some(&told), &TYPES).judge(&group, (&pages, &mut Prepared::default()), (&TYPES[..], run), None).unwrap();
        (given.kept, given.decoded.len())
    };
    assert_eq!(judged(CompOp::Ge, least), ((0..2048).collect(), 0), "proved: every row, no column decoded");
    assert_eq!(judged(CompOp::Gt, least), ((1..2048).collect(), 1), "not proved: judged a row");
}

/// Verifies: [DESIGN] RULES.md D1293, [X3.135-1986 §5.11 GR2]: a part
/// that holds a null proves no comparison, as none is true of a null, nor
/// does a run that reaches it; the part before it proves its own; the
/// same rows of a column of no null are proved; and the part of the null
/// is judged, the null's row left out.
#[test]
fn a_part_that_holds_a_null_proves_nothing() {
    let (pool, file) = file();
    let pages = pages(&pool, file);
    let (group, _) = group(&pages);
    let (from, to) = part_holding(&group, 1, NULL_ROW);
    let before = part_holding(&group, 1, sub(from, 1));
    let any = |rows: (u32, u32)| proves(&group, (1, CompOp::Ge, number(0)), rows);
    assert!(!any((from, to)), "the part of the null");
    assert!(!any((before.0, to)), "a run that reaches it");
    assert!(any(before), "the part before it");
    assert!(proves(&group, (0, CompOp::Ge, number(0)), part_holding(&group, 0, NULL_ROW)), "a column of no null");
    let told = Conjunction(vec![compared(1, CompOp::Ge, number(0))]);
    let given =
        Judgment::of(Some(&told), &TYPES).judge(&group, (&pages, &mut Prepared::default()), (&TYPES[..], (from, to)), None).unwrap();
    assert_eq!(given.kept, (0..sub(to, from)).filter(|&r| r != sub(NULL_ROW, from)).collect::<Vec<u32>>());
}

/// Verifies: [DESIGN] RULES.md D1293: whatever run is asked, of whole
/// parts or not, within a chunk or across two, a comparison proved of it
/// is true of every row of it, and the rows the scan keeps are those the
/// comparison is true of, proved or judged, of every row and of rows
/// given; some are proved and some are not; a day is proved as a number
/// is, at its bounds; and a column of texts is never proved, whatever its
/// bounds say.
#[test]
fn what_is_proved_of_a_run_is_true_of_each_of_its_rows() {
    let (pool, file) = file();
    let pages = pages(&pool, file);
    let (group, rows) = group(&pages);
    let b = second_chunk(&group, 0);
    let null = part_holding(&group, 1, NULL_ROW);
    let runs = [
        (b, add(b, 2048)),
        (sub(b, 1024), add(b, 1024)),
        (add(b, 500), add(b, 2547)),
        (0, 2048),
        null,
        (sub(ROWS, 1), ROWS),
        (b, add(b, 1)),
    ];
    let ops = [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Le, CompOp::Gt, CompOp::Ge];
    let near = |n: u32| [i128::from(n).saturating_sub(1), i128::from(n), i128::from(add(n, 1))];
    let places = [0, sub(b, 1024), b, add(b, 1024), add(b, 2047), null.0, NULL_ROW, sub(ROWS, 1)];
    let constants: Vec<i128> = places.iter().flat_map(|&n| near(n)).collect();
    let mut cases = Vec::new();
    for (&run, &op) in runs.iter().flat_map(|r| ops.iter().map(move |o| (r, o))) {
        cases.extend(constants.iter().flat_map(|&c| [(run, op, c, 0_usize), (run, op, c, 1_usize)]));
    }
    let (mut proofs, mut others) = (0_usize, 0_usize);
    for (run, op, c, column) in cases {
        let conjunct = compared(column, op, number(c));
        let keeps = |r: &u32| conjunct.matches_key(&[&rows[index(add(run.0, *r))][column]]);
        let all: Vec<u32> = (0..sub(run.1, run.0)).collect();
        if proves(&group, (column, op, number(c)), run) {
            proofs = proofs.saturating_add(1);
            assert!(all.iter().all(keeps), "{conjunct:?} proved of {run:?}");
        } else {
            others = others.saturating_add(1);
        }
        let judgment = Judgment::of(Some(&Conjunction(vec![conjunct.clone()])), &TYPES);
        for given in [None, Some(all.iter().copied().step_by(3).collect::<Vec<u32>>())] {
            let wanted: Vec<u32> = given.as_ref().unwrap_or(&all).iter().copied().filter(keeps).collect();
            let run_kept = judgment.judge(&group, (&pages, &mut Prepared::default()), (&TYPES[..], run), given).unwrap().kept;
            assert_eq!(run_kept, wanted, "{conjunct:?} of {run:?}");
        }
    }
    assert!(proofs > 0 && others > 0, "{proofs} proved, {others} not");
    let d = part_holding(&group, 3, 5000);
    let (first, last) = (day(hundreds(d.0)), day(hundreds(sub(d.1, 1))));
    assert!(proves(&group, (3, CompOp::Ge, first.clone()), d) && !proves(&group, (3, CompOp::Gt, first), d));
    assert!(proves(&group, (3, CompOp::Le, last.clone()), d) && !proves(&group, (3, CompOp::Lt, last), d));
    assert!(!proves(&group, (4, CompOp::Ge, Value::Char("text".into())), part_holding(&group, 4, 5000)), "a text's bounds prove nothing");
}

/// Verifies: [DESIGN] RULES.md D1293: a run proves a comparison only if
/// chunks whose parts prove it hold every row of it. Of a header that
/// lacks a chunk, a run that reaches the rows it held is not proved,
/// though the chunk before proves its own rows, nor is one that ends
/// past the last chunk.
#[test]
fn rows_no_chunk_holds_prove_nothing() {
    let (pool, file) = file();
    let pages = pages(&pool, file);
    let (whole, _) = group(&pages);
    let b = second_chunk(&whole, 0);
    let across = (sub(b, 1024), add(b, 1024));
    assert!(proves(&whole, (0, CompOp::Ge, number(0)), across), "every chunk there");
    let mut gapped = whole.clone();
    gapped.columns[0].as_mut().unwrap().remove(1);
    assert!(proves(&gapped, (0, CompOp::Ge, number(0)), (sub(b, 1024), b)), "the chunk before the gap");
    assert!(!proves(&gapped, (0, CompOp::Ge, number(0)), across), "a run into the gap");
    let mut short = whole.clone();
    let last = short.columns[0].as_mut().unwrap().pop().unwrap();
    assert!(!proves(&short, (0, CompOp::Ge, number(0)), (sub(last.run.first, 10), add(last.run.first, 10))), "past the last chunk");
}
