//! Unit tests of column imprints (`imprints.rs`): their bins, their
//! words, and that a scan narrowed by them never leaves out a row its
//! condition matches, of scattered, sorted and skewed values and nulls.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::sync::Arc;

use super::*;
use crate::disk::buffer::{BufferPool, FileId};
use crate::engine::columns::group::read;
use crate::engine::columns::scan::{candidates, read_candidates, read_ordered};
use crate::engine::{MEMORY_POOL_BYTES, memory_file};
use crate::index::Probe;
use crate::memory::MemoryBudget;
use crate::storage::{Condition, Row, RowId};
use crate::value::{CompOp, Decimal, Spelled};

const INTEGER: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
const TEXT: DataType = DataType::varying(20);
const TYPES: [DataType; 3] = [INTEGER, TEXT, INTEGER];
/// Rows of each group tested.
const ROWS: i128 = 6000;

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
    memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 4_i32), "a test").unwrap()
}

/// Row `n` of a group whose first column is as `shape` makes it of `n`:
/// a text of the value, and the value again, every eleventh null and
/// those of 400 rows in a row.
fn row(n: i128, shape: fn(i128) -> i128) -> Row {
    let v = shape(n);
    let text = Value::Char(format!("v{:05}", v.rem_euclid(100_000)).into());
    let last = if n % 11 == 0 || (3000..3400).contains(&n) { Value::Null } else { int(v) };
    vec![int(v), text, last]
}

/// Scattered values, sorted ones, and skewed ones: nine in ten one value.
const SHAPES: [fn(i128) -> i128; 3] = [|n| n * 7919 % ROWS, |n| n, |n| if n % 10 == 0 { n } else { 42 }];

fn compare(column: usize, op: CompOp, value: Value) -> Conjunct {
    Conjunct::Condition(Condition { columns: vec![column], probe: Probe::Compare { op, value } })
}

/// The conditions tried: comparisons of each kind, a range, IN, NOT IN,
/// an OR, a prefix of texts, IS NULL and a condition of two columns.
fn conditions() -> Vec<Conjunct> {
    let range = Probe::Range { equal: Vec::new(), low: Box::new((CompOp::Ge, int(100))), high: Box::new((CompOp::Lt, int(160))) };
    let prefix = Probe::Range {
        equal: Vec::new(),
        low: Box::new((CompOp::Ge, Value::Char("v001".into()))),
        high: Box::new((CompOp::Lt, Value::Char("v002".into()))),
    };
    vec![
        compare(0, CompOp::Lt, int(30)),
        compare(0, CompOp::Gt, int(5990)),
        compare(0, CompOp::Eq, int(42)),
        compare(0, CompOp::Eq, int(4321)),
        compare(2, CompOp::Le, int(7)),
        Conjunct::Condition(Condition { columns: vec![0], probe: range }),
        Conjunct::Condition(Condition { columns: vec![1], probe: prefix }),
        Conjunct::In { column: 0, values: vec![int(1), int(2000), int(5999)] },
        Conjunct::NotIn { column: 2, values: vec![int(42)] },
        Conjunct::Any(vec![compare(0, CompOp::Lt, int(3)), compare(0, CompOp::Gt, int(5996))]),
        Conjunct::Null { column: 2, not: false },
        Conjunct::Condition(Condition {
            columns: vec![0, 2],
            probe: Probe::Prefix { equal: vec![int(42)], then: Some((CompOp::Gt, int(10))) },
        }),
    ]
}

/// Verifies: [DESIGN] RULES.md D511, imprints never leave out a matching
/// row: of a group of scattered, sorted and skewed values, a text of
/// each and a column with nulls, every row each condition matches, a
/// comparison, a range, a prefix, IN, NOT IN, an OR, IS NULL or one of
/// two columns, is among
/// the runs a scan reads; imprints read back from the header; and a
/// condition of a value rare among skewed ones reads a block or two of
/// the part the zone maps alone keep.
#[test]
fn imprints_never_leave_out_a_matching_row() {
    let (pool, file) = file();
    let pages = Pages {
        pool: &pool,
        file,
        capacity: 4096,
        sealing: &Plain,
        compress: true,
        pax: false,
        imprints: true,
        kernels: &crate::kernels::Portable,
    };
    for (s, shape) in SHAPES.into_iter().enumerate() {
        let all: Vec<Row> = (0..ROWS).map(|n| row(n, shape)).collect();
        let ids: Vec<RowId> = (1..=u64::try_from(ROWS).unwrap()).collect();
        let group = Group::write(&pages, &TYPES, (&ids, &all), (&[], &[], &[])).unwrap();
        let back = read(&pages, group.header[0], &TYPES).unwrap();
        assert_eq!(format!("{:?}", back.edges), format!("{:?}", group.edges), "edges read back");
        // Scattered values, the first shape's, get none.
        assert!(group.edges.iter().all(|e| e.is_empty() == (s == 0) && e.len() < BINS), "shape {s}: bins of all or none");
        for conjunct in conditions() {
            let condition = Conjunction(vec![conjunct.clone()]);
            let runs = read_candidates(&back, &pages, Some(&condition)).unwrap();
            for (n, r) in all.iter().enumerate() {
                let at = u32::try_from(n).unwrap();
                if condition.matches(r) {
                    assert!(runs.iter().any(|&(from, to)| from <= at && at < to), "row {n} of {conjunct:?} read");
                }
            }
        }
    }
    let skewed: Vec<Row> = (0..ROWS).map(|n| row(n, SHAPES[2])).collect();
    let ids: Vec<RowId> = (1..=u64::try_from(ROWS).unwrap()).collect();
    let group = Group::write(&pages, &TYPES, (&ids, &skewed), (&[], &[], &[])).unwrap();
    let one = Conjunction(vec![compare(0, CompOp::Eq, int(4320))]);
    let rows = |runs: &[(u32, u32)]| runs.iter().map(|(f, t)| t - f).sum::<u32>();
    let narrowed = rows(&read_candidates(&group, &pages, Some(&one)).unwrap());
    assert!(narrowed < rows(&candidates(&group, Some(&one))), "imprints narrow");
    assert!(narrowed <= BLOCK * 2, "a block or two of 64: {narrowed}");
}

/// Verifies: [DESIGN] RULES.md D511, bins: of no more distinct values
/// than bins, each is an edge; of many, the edges are fewer than the
/// bins, ascending and apart; nulls take no bin, and a column of nulls
/// alone has none; a value is in the bin of the first edge not below it.
#[test]
fn bins_are_drawn_apart_and_ascending() {
    let few: Vec<Value> = (0..1000).map(|n| int(n % 10)).collect();
    assert_eq!(format!("{:?}", edges(few.iter())), format!("{:?}", (0..10).map(int).collect::<Vec<_>>()));
    let many: Vec<Value> = (0..100_000).map(int).collect();
    let cut = edges(many.iter());
    assert_eq!(cut.len(), BINS - 1);
    assert!(cut.windows(2).all(|w| compare_values(&w[0], &w[1]).is_lt()));
    let nulls = vec![Value::Null; 100];
    assert!(edges(nulls.iter()).is_empty());
    let words = words(&[int(10), int(20)], [int(5), int(10), int(15), Value::Null, int(99)].iter());
    assert_eq!(words, vec![0b111], "bins 0, 0, 1, none and 2");
}

/// Verifies: [DESIGN] RULES.md D511, a mask: of a condition true of a
/// null, or a column with no bins, none; else the bins it may match.
#[test]
fn masks_are_of_the_bins_a_condition_may_match() {
    let cut = vec![int(10), int(20), int(30)];
    let whole = (int(0), int(40));
    assert_eq!(mask(&cut, Some(&whole), (0, &compare(0, CompOp::Lt, int(15)))), Some(0b11));
    assert_eq!(mask(&cut, Some(&whole), (0, &compare(0, CompOp::Gt, int(30)))), Some(0b1000));
    assert_eq!(mask(&cut, Some(&whole), (0, &Conjunct::Null { column: 0, not: false })), None, "true of a null");
    assert_eq!(mask(&[], Some(&whole), (0, &compare(0, CompOp::Lt, int(15)))), None, "no bins");
    assert_eq!(
        bridged(&[(0, 64), (128, 192), (512, 576), (1000, 1064)]),
        vec![(0, 192), (512, 576), (1000, 1064)],
        "gaps of 4 blocks kept"
    );
}

/// Verifies: [DESIGN] RULES.md D511, narrowest first: of two conjuncts,
/// the one that keeps fewer rows of a group by its imprints and zone maps
/// is judged first, whatever their order in the condition.
#[test]
fn conjuncts_are_ordered_narrowest_first() {
    let (pool, file) = file();
    let pages = Pages {
        pool: &pool,
        file,
        capacity: 4096,
        sealing: &Plain,
        compress: true,
        pax: false,
        imprints: true,
        kernels: &crate::kernels::Portable,
    };
    let all: Vec<Row> = (0..ROWS).map(|n| row(n, SHAPES[1])).collect();
    let ids: Vec<RowId> = (1..=u64::try_from(ROWS).unwrap()).collect();
    let group = Group::write(&pages, &TYPES, (&ids, &all), (&[], &[], &[])).unwrap();
    let wide = compare(2, CompOp::Gt, int(10));
    let narrow = compare(0, CompOp::Lt, int(100));
    let (_, order) = read_ordered(&group, &pages, Some(&Conjunction(vec![wide, narrow]))).unwrap();
    assert_eq!(order, vec![1, 0]);
}

/// `a` against `b`.
fn compare_values(a: &Value, b: &Value) -> Ordering {
    crate::value::compare(a, b).unwrap()
}
