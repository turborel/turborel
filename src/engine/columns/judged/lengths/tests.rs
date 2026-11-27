//! Unit tests of a comparison with the empty string judged of a measured
//! column's lengths (`lengths.rs`, RULES.md D1267), and the group a
//! scan's tests of octets judge.

use std::borrow::Cow;
use std::sync::Arc;

use super::super::super::group::{Group, Pages, Seal};
use super::super::super::prepared::Prepared;
use super::super::{Judgment, Pred};
use crate::disk::buffer::{BufferPool, FileId};
use crate::engine::{MEMORY_POOL_BYTES, memory_file};
use crate::error::Result;
use crate::memory::MemoryBudget;
use crate::storage::mask::index;
use crate::storage::{Conjunct, Conjunction, Row};
use crate::value::{CompOp, DataType, Decimal, Spelled, Value};

pub(in crate::engine::columns::judged) const INTEGER: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
pub(in crate::engine::columns::judged) const TEXT: DataType = DataType::varying(100);
pub(in crate::engine::columns::judged) const OPS: [CompOp; 6] = [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Le, CompOp::Gt, CompOp::Ge];

/// `column <op> text` by octets, unpadded, as a scan is told it.
pub(in crate::engine::columns::judged) fn told(op: CompOp, text: &str) -> Pred {
    Pred { at: 0, column: 1, conjunct: Conjunct::Unpadded { column: 1, op, octets: text.as_bytes().to_vec() }, of_null: false }
}

/// One of six words a row, the empty string and a word with a space
/// after among them, every seventh null.
pub(in crate::engine::columns::judged) fn word(n: usize) -> Value {
    if n % 7 == 6 {
        return Value::Null;
    }
    Value::Char(["a", "a ", "", "ab", "green", "a"][n % 6].into())
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
    memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 4_i32), "a test").unwrap()
}

/// Rows of a scan's test group.
pub(in crate::engine::columns::judged) const ROWS: usize = 5000;

/// The word of row `n` of a scan's test group: every twenty-third
/// `rare`, else as `word`.
fn scan_word(n: usize) -> Value {
    if n.is_multiple_of(23) { Value::Char("rare".into()) } else { word(n) }
}

/// How a scan's test reads its group: chunks compressed or plain, and
/// the text column one the scan measures.
#[derive(Clone, Copy)]
pub(in crate::engine::columns::judged) struct Read {
    pub(in crate::engine::columns::judged) compress: bool,
    pub(in crate::engine::columns::judged) measured: bool,
}

/// The rows each run of 2,048 rows of a group of `ROWS` rows, a number
/// and `scan_word`'s text of type `t`, keeps under `pred`, of every row
/// and of every third; how many columns were decoded to judge them; and
/// whether lengths were held after each judging.
pub(in crate::engine::columns::judged) fn scanned(t: &DataType, pred: &Pred, read: Read) -> (Vec<Vec<u32>>, usize, Vec<bool>) {
    let (pool, file) = file();
    let pages = Pages {
        pool: &pool,
        file,
        capacity: 8192,
        sealing: &Plain,
        compress: read.compress,
        pax: false,
        imprints: false,
        kernels: &crate::kernels::Portable,
        base: 0,
        sink: None,
    };
    let types = [INTEGER, t.clone()];
    let rows: Vec<Row> = (0..ROWS).map(|n| vec![Value::Exact(Decimal::new(i128::try_from(n).unwrap(), 0)), scan_word(n)]).collect();
    let ids: Vec<u64> = (1..=u64::try_from(ROWS).unwrap()).collect();
    let group = Group::write(&pages, &types, (&ids, &rows), (&[], &[], &[])).unwrap();
    let judgment = Judgment::of(Some(&Conjunction(vec![pred.conjunct.clone()])), &types);
    let (mut prepared, mut out, mut decoded, mut held) = (Prepared::default(), Vec::new(), 0_usize, Vec::new());
    let text = |asked: bool| if asked { vec![1] } else { Vec::new() };
    prepared.measure(&text(read.measured));
    let count = u32::try_from(ROWS).unwrap();
    for from in (0..count).step_by(2048) {
        let to = from.saturating_add(2048).min(count);
        for given in [None, Some((0..to.saturating_sub(from)).step_by(3).collect::<Vec<u32>>())] {
            let whole = given.is_none();
            let run = judgment.judge(&group, (&pages, &mut prepared), (&types, (from, to)), given).unwrap();
            decoded = decoded.saturating_add(run.decoded.len());
            out.push(run.kept);
            let places: Vec<u32> = (from..to).collect();
            let lengths = prepared.take_lengths(1, &places);
            assert_eq!(lengths.is_some(), whole && lengths.as_ref().is_some_and(|l| l.units.len() == places.len()), "of a whole run alone");
            held.push(lengths.is_some());
            assert!(prepared.take_lengths(1, &places).is_none(), "given once");
            if whole && read.measured {
                // What the next judging holds is no other column's.
                judgment.judge(&group, (&pages, &mut prepared), (&types, (from, to)), None).unwrap();
                assert!(prepared.take_lengths(0, &places).is_none(), "another column's");
                assert!(prepared.take_lengths(1, &places).is_some(), "its own, still held");
            }
        }
    }
    (out, decoded, held)
}

/// The rows each run of a scan's test group keeps, of every row and of
/// every third, as `keeps` tells of each value not null.
pub(in crate::engine::columns::judged) fn expected(keeps: &dyn Fn(&str) -> bool) -> Vec<Vec<u32>> {
    let values: Vec<Value> = (0..ROWS).map(scan_word).collect();
    let kept = |part: &[Value], r: &u32| if let Value::Char(s) = &part[index(*r)] { keeps(s.as_str()) } else { false };
    let mut out = Vec::new();
    for from in (0..ROWS).step_by(2048) {
        let part = &values[from..from.saturating_add(2048).min(ROWS)];
        let all: Vec<u32> = (0..u32::try_from(part.len()).unwrap()).collect();
        out.push(all.iter().copied().filter(|r| kept(part, r)).collect());
        out.push(all.iter().copied().step_by(3).filter(|r| kept(part, r)).collect());
    }
    out
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.3.2] RULES.md D1267: of a column
/// the scan gives measured, a comparison unpadded with the empty string
/// is judged of the lengths the scan takes, for each operator, a null
/// never kept, and no column is decoded; the lengths of a run judged of
/// every row are held for its batch and given once, those of a run
/// judged of some rows are not. A comparison with another constant is
/// not judged of lengths, nor is one of a column not measured, and
/// neither holds any. MC/DC of the lengths judged: the empty constant
/// against another, and measured against not.
#[test]
fn a_measured_columns_comparison_with_the_empty_string_is_judged_of_its_lengths() {
    let measured = Read { compress: true, measured: true };
    for op in OPS {
        let wanted = expected(&|s| op.holds(s.len().cmp(&0)));
        assert_eq!(scanned(&TEXT, &told(op, ""), measured), (wanted, 0, [true, false].repeat(3)), "{op:?}");
    }
    let (kept, decoded, held) = scanned(&TEXT, &told(CompOp::Ne, "a"), measured);
    assert_eq!((kept, decoded, held), (expected(&|s| s != "a"), 0, vec![false; 6]), "another constant is compared in place");
    let unmeasured = Read { measured: false, ..measured };
    assert_eq!(scanned(&TEXT, &told(CompOp::Ne, ""), unmeasured).2, vec![false; 6], "not measured, none held");
}
