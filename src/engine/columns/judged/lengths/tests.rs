//! Unit tests of a comparison with the empty string judged of a measured
//! column's lengths (`lengths.rs`, RULES.md D1267), and the group a
//! scan's tests of octets judge.

use std::borrow::Cow;
use std::sync::Arc;

use super::super::super::group::{Group, Pages, Seal};
use super::super::super::prepared::Prepared;
use super::super::{Judgment, Pred};
use super::proved;
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

/// How a scan's test reads its group: chunks compressed or plain, the
/// text column one the scan measures, and its lengths lane kept or not.
#[derive(Clone, Copy)]
pub(in crate::engine::columns::judged) struct Read {
    pub(in crate::engine::columns::judged) compress: bool,
    pub(in crate::engine::columns::judged) measured: bool,
    /// Whether the group keeps the text's lengths lane (RULES.md D1319).
    pub(in crate::engine::columns::judged) lane: bool,
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
    let mut group = Group::write(&pages, &types, (&ids, &rows), (&[], &[], &[])).unwrap();
    if !read.lane {
        group.lanes = vec![None; types.len()];
    }
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
            if held.last() == Some(&true) {
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
/// is judged of the lengths the scan takes, of the column's lane or of
/// its text's chunks, for each operator, a null never kept, and no
/// column is decoded; the lengths of a run judged of every row are held
/// for its batch and given once, those of a run judged of some rows are
/// not. A comparison with another constant is not judged of lengths.
/// MC/DC of the lengths judged: the empty constant against another.
#[test]
fn a_measured_columns_comparison_with_the_empty_string_is_judged_of_its_lengths() {
    for lane in [true, false] {
        let measured = Read { compress: true, measured: true, lane };
        for op in OPS {
            let wanted = expected(&|s| op.holds(s.len().cmp(&0)));
            assert_eq!(scanned(&TEXT, &told(op, ""), measured), (wanted, 0, [true, false].repeat(3)), "{op:?}, lane {lane}");
        }
        let (kept, _, held) = scanned(&TEXT, &told(CompOp::Ne, "a"), measured);
        assert_eq!((kept, held), (expected(&|s| s != "a"), vec![false; 6]), "another constant is not judged of lengths");
    }
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.3.2] RULES.md D1319: of a column the
/// scan gives as text, a comparison unpadded with the empty string is
/// judged of the column's lengths lane where its group keeps one, for
/// each operator, no column decoded and no lengths held; of a group with
/// no lane the column is decoded to judge it, as before, and the rows
/// kept are the same. MC/DC of the lane judged: a group with one against
/// one with none.
#[test]
fn a_comparison_with_the_empty_string_is_judged_of_a_lane_where_one_is_kept() {
    let with = Read { compress: true, measured: false, lane: true };
    for op in OPS {
        let wanted = expected(&|s| op.holds(s.len().cmp(&0)));
        assert_eq!(scanned(&TEXT, &told(op, ""), with), (wanted, 0, vec![false; 6]), "{op:?}");
    }
    let (kept, decoded, held) = scanned(&TEXT, &told(CompOp::Ne, ""), Read { lane: false, ..with });
    assert_eq!((kept, held), (expected(&|s| !s.is_empty()), vec![false; 6]));
    assert_eq!(decoded, 6, "with no lane the column is decoded for each judging");
}

/// A group of 5,000 rows of a number and a text, in bands of its parts
/// of 1,024 rows: two of words alone, two of empty strings alone, and a
/// last of words, empty strings and nulls.
fn banded() -> (Arc<BufferPool>, FileId, Group) {
    let (pool, file) = file();
    let pages = Pages {
        pool: &pool,
        file,
        capacity: 8192,
        sealing: &Plain,
        compress: true,
        pax: false,
        imprints: false,
        kernels: &crate::kernels::Portable,
        base: 0,
        sink: None,
    };
    let text = |n: usize| match (n >> 10_u32, n % 3) {
        (0 | 1, _) | (4, 0) => Value::Char(["one", "three"][n % 2].into()),
        (2 | 3, _) | (4, 1) => Value::Char("".into()),
        _ => Value::Null,
    };
    let rows: Vec<Row> = (0..ROWS).map(|n| vec![Value::Exact(Decimal::new(i128::try_from(n).unwrap(), 0)), text(n)]).collect();
    let ids: Vec<u64> = (1..=u64::try_from(ROWS).unwrap()).collect();
    let group = Group::write(&pages, &[INTEGER, TEXT], (&ids, &rows), (&[], &[], &[])).unwrap();
    (pool, file, group)
}

/// Verifies: [ISO/IEC 9075-2:2023 §4.3.2] RULES.md D1319: the bounds of a
/// lengths lane's parts prove a comparison with the empty string of a
/// whole run, and pass over the parts it cannot match. A run of words
/// alone proves `<>` and `>`, a run of empty strings alone `=` and `<=`,
/// either `>=`, neither `<`; a run that crosses the two proves only
/// `>=`, and one with a null proves nothing. A scan for `=` reads the
/// parts with an empty string alone, one for `<>` the parts with a word.
/// Another constant, another conjunct and a group with no lane prove
/// nothing and pass over nothing by lengths. MC/DC of each operator at
/// the bound that decides it.
#[test]
fn a_lanes_bounds_prove_a_run_and_pass_over_parts() {
    use crate::engine::columns::scan::candidates;
    let (_pool, _file, group) = banded();
    let (words, empties, crossing, mixed) = ((0, 2048), (2048, 4096), (1024, 3072), (4096, 5000));
    let proves = |op: CompOp, run: (u32, u32)| proved(&group, &told(op, ""), run);
    let cases = [
        (CompOp::Ne, [true, false, false, false]),
        (CompOp::Gt, [true, false, false, false]),
        (CompOp::Eq, [false, true, false, false]),
        (CompOp::Le, [false, true, false, false]),
        (CompOp::Ge, [true, true, true, false]),
        (CompOp::Lt, [false, false, false, false]),
    ];
    for (op, wanted) in cases {
        assert_eq!([words, empties, crossing, mixed].map(|run| proves(op, run)), wanted, "{op:?}");
    }
    assert!(!proved(&group, &told(CompOp::Ne, "a"), words), "another constant");
    let null = Pred { at: 0, column: 1, conjunct: Conjunct::Null { column: 1, not: true }, of_null: false };
    assert!(!proved(&group, &null, words), "another conjunct");
    let mut bare = group.clone();
    bare.lanes = vec![None; 2];
    assert!(!proved(&bare, &told(CompOp::Ne, ""), words), "no lane");
    let parts = |g: &Group, op: CompOp, text: &str| candidates(g, Some(&Conjunction(vec![told(op, text).conjunct]))).unwrap();
    assert_eq!(parts(&group, CompOp::Eq, ""), [(2048, 5000)], "the parts with an empty string");
    assert_eq!(parts(&group, CompOp::Ne, ""), [(0, 2048), (4096, 5000)], "the parts with a word");
    assert_eq!(parts(&group, CompOp::Lt, ""), [], "no string is before the empty one");
    assert_eq!(parts(&bare, CompOp::Ne, ""), [(0, 5000)], "no lane, no part passed over by lengths");
    assert_eq!(parts(&group, CompOp::Ne, "a"), [(0, 5000)], "another constant");
}
