//! Unit tests of conjuncts judged by a sketch's codes (`sketched.rs`).

use std::borrow::Cow;
use std::sync::Arc;

use super::super::super::group::{Group, Pages, Seal};
use super::super::super::prepared::Prepared;
use super::super::super::sketch::Bin;
use super::super::Judgment;
use super::*;
use crate::disk::buffer::{BufferPool, FileId};
use crate::engine::{MEMORY_POOL_BYTES, memory_file};
use crate::error::Result;
use crate::index::Probe;
use crate::memory::MemoryBudget;
use crate::storage::{Condition, Conjunct, Conjunction, Row};
use crate::value::{CompOp, DataType, Decimal, Spelled, Value};

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

/// A web address, every nineteenth null, every fourth one address, some
/// ending in spaces.
fn url(n: u32) -> Value {
    if n.is_multiple_of(19) {
        return Value::Null;
    }
    if n.is_multiple_of(4) {
        return Value::Char("http://often.example/".to_owned());
    }
    let pad = if n.is_multiple_of(11) { " " } else { "" };
    Value::Char(format!("http://site{}.example/page{}{pad}", n.wrapping_mul(7919) % 300, n.wrapping_mul(104_729) % 50_000))
}

fn compare(op: CompOp, c: &str) -> Conjunct {
    Conjunct::Condition(Condition { columns: vec![1], probe: Probe::Compare { op, value: Value::Char(c.to_owned()) } })
}

/// The conjuncts judged, of `constants`: every comparison of each, a LIKE
/// prefix as its bounds, IN with a null, NOT IN, an OR with IS NULL, IS
/// NULL and IS NOT NULL.
fn conjuncts(constants: &[&str]) -> Vec<Conjunct> {
    let mut out = Vec::new();
    for c in constants {
        for op in [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Gt, CompOp::Le, CompOp::Ge] {
            out.push(compare(op, c));
        }
    }
    let strings: Vec<Value> = constants.iter().map(|c| Value::Char((*c).to_owned())).collect();
    out.push(Conjunct::NotIn { column: 1, values: strings.clone() });
    out.push(Conjunct::In { column: 1, values: strings.into_iter().chain([Value::Null]).collect() });
    out.push(Conjunct::Any(vec![compare(CompOp::Lt, "http://site1"), Conjunct::Null { column: 1, not: false }]));
    out.push(Conjunct::Null { column: 1, not: false });
    out.push(Conjunct::Null { column: 1, not: true });
    out
}

/// The constants tried of `edges`: an edge a point and one not, a value
/// between edges, the same padded, a prefix, one below every value and
/// one above.
fn constants(edges: &Edges) -> Vec<String> {
    let point = edges.each().find(|(_, p)| *p).map(|(t, _)| t.to_owned()).unwrap();
    let plain = edges.each().find(|(_, p)| !*p).map(|(t, _)| t.to_owned()).unwrap();
    let between = "http://site150.example/page7".to_owned();
    vec![point, format!("{plain}  "), plain, between.clone(), format!("{between} "), "http://site2".into(), String::new(), "zz".into()]
}

fn pred(conjunct: &Conjunct) -> Pred {
    Pred { at: 0, column: 1, conjunct: conjunct.clone(), of_null: conjunct.matches_key(&[&Value::Null]) }
}

/// Verifies: [DESIGN] RULES.md D525, what codes say: of each conjunct of
/// every kind, of an edge a point, one not and padded, a value between
/// edges, a prefix and constants past every value, a code said to keep
/// holds only values the conjunct matches, one said to leave only values
/// it does not, a null's code as the conjunct is of a null; a comparison
/// leaves at most one code to compare, an equality of a point none.
#[test]
fn codes_say_what_their_values_say() {
    let values: Vec<Value> = (0..ROWS).map(url).collect();
    let refs: Vec<&Value> = values.iter().collect();
    let edges = Edges::of(&refs);
    let texts = constants(&edges);
    let strs: Vec<&str> = texts.iter().map(String::as_str).collect();
    for conjunct in conjuncts(&strs) {
        let p = pred(&conjunct);
        let verdicts = verdicts(&edges, &p).unwrap();
        assert_eq!(verdicts.of(NULL_CODE), if p.of_null { Verdict::Keep } else { Verdict::Drop }, "{conjunct:?} of a null");
        for v in values.iter().filter(|v| !v.is_null()) {
            let Value::Char(s) = v else { continue };
            let matched = conjunct.matches_key(&[v]);
            match verdicts.of(edges.code(s.as_bytes())) {
                Verdict::Keep => assert!(matched, "{s:?} kept by {conjunct:?}"),
                Verdict::Drop => assert!(!matched, "{s:?} left by {conjunct:?}"),
                Verdict::Check => {}
            }
        }
        let checked = edges.bins().filter(|&(code, _)| verdicts.of(code) == Verdict::Check).count();
        if let Conjunct::Condition(_) = conjunct {
            assert!(checked <= 1, "{conjunct:?}: {checked} codes to compare");
        }
    }
    let point = compare(CompOp::Eq, strs[0]);
    let all = verdicts(&edges, &pred(&point)).unwrap();
    assert!(edges.bins().all(|(code, _)| all.of(code) != Verdict::Check), "a point compares none");
    let (_, bin) = edges.bins().find(|(_, b)| matches!(b, Bin::Point(_))).unwrap();
    assert!(matches!(bin, Bin::Point(_)));
    let of_number = Conjunct::Condition(Condition {
        columns: vec![1],
        probe: Probe::Compare { op: CompOp::Eq, value: Value::Exact(Decimal::new(1, 0)) },
    });
    assert!(verdicts(&edges, &pred(&of_number)).is_none(), "a constant not a string");
}

/// The rows of each run of a vector of `group` each of `conjuncts` keeps,
/// judged by the scan, of every row and of every third.
fn kept(group: &Group, pages: &Pages<'_, Plain>, conjunct: &Conjunct) -> Vec<Vec<u32>> {
    let judgment = Judgment::of(Some(&Conjunction(vec![conjunct.clone()])), &TYPES);
    let mut prepared = Prepared::default();
    let mut out = Vec::new();
    for from in (0..ROWS).step_by(2048) {
        let to = from.saturating_add(2048).min(ROWS);
        for given in [None, Some((0..to.saturating_sub(from)).step_by(3).collect::<Vec<u32>>())] {
            out.push(judgment.judge(group, (pages, &mut prepared), (&TYPES, (from, to)), given).unwrap().kept);
        }
    }
    out
}

/// Verifies: [DESIGN] RULES.md D525, a scan judging by sketches: of a
/// group of texts coded by symbols, nulls, a frequent value and texts
/// ending in spaces among them, sketched, each conjunct of every kind
/// keeps of each run, of every row and of every third, the rows whose
/// value it matches, as a group not sketched keeps them.
#[test]
fn scans_by_sketches_keep_what_values_keep() {
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
    };
    let ids: Vec<u64> = (1..=u64::from(ROWS)).collect();
    let rows: Vec<Row> = (0..ROWS).map(|n| vec![Value::Exact(Decimal::new(i128::from(n), 0)), url(n)]).collect();
    let sketched = Group::write(&pages, &TYPES, (&ids, &rows), (&[], &[], &[1])).unwrap();
    let plain = Group::write(&pages, &TYPES, (&ids, &rows), (&[], &[], &[])).unwrap();
    assert!(!sketched.sketches[1].is_empty(), "a sketch");
    let texts = constants(&sketched.sketches[1]);
    let strs: Vec<&str> = texts.iter().map(String::as_str).collect();
    for conjunct in conjuncts(&strs) {
        let by_sketch = kept(&sketched, &pages, &conjunct);
        let mut expected = Vec::new();
        for from in (0..ROWS).step_by(2048) {
            let to = from.saturating_add(2048).min(ROWS);
            let matches = |r: &u32| conjunct.matches_key(&[&rows[usize::try_from(from.saturating_add(*r)).unwrap()][1]]);
            expected.push((0..to.saturating_sub(from)).filter(matches).collect::<Vec<u32>>());
            expected.push((0..to.saturating_sub(from)).step_by(3).filter(matches).collect::<Vec<u32>>());
        }
        assert_eq!(by_sketch, expected, "{conjunct:?}");
        assert_eq!(kept(&plain, &pages, &conjunct), expected, "{conjunct:?}, not sketched");
    }
}

/// Verifies: [DESIGN] RULES.md D525, few rows asked for: of a run of
/// which one row of each sixteen is asked for, a scan compares their
/// texts, reading no code of the sketch and judging no code, and keeps
/// the rows whose value the conjunct matches; of one of which every
/// third is asked for, it reads the codes.
#[test]
fn few_rows_asked_for_are_compared_without_the_sketch() {
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
    };
    let ids: Vec<u64> = (1..=u64::from(ROWS)).collect();
    let rows: Vec<Row> = (0..ROWS).map(|n| vec![Value::Exact(Decimal::new(i128::from(n), 0)), url(n)]).collect();
    let group = Group::write(&pages, &TYPES, (&ids, &rows), (&[], &[], &[1])).unwrap();
    let conjunct = compare(CompOp::Ge, "http://site150");
    let judgment = Judgment::of(Some(&Conjunction(vec![conjunct.clone()])), &TYPES);
    for (step, read) in [(16, false), (3, true)] {
        let mut prepared = Prepared::default();
        let given: Vec<u32> = (0..2048).step_by(step).collect();
        let kept = judgment.judge(&group, (&pages, &mut prepared), (&TYPES, (0, 2048)), Some(given.clone())).unwrap().kept;
        let expected: Vec<u32> = given.into_iter().filter(|&r| conjunct.matches_key(&[&rows[usize::try_from(r).unwrap()][1]])).collect();
        assert_eq!(kept, expected, "every {step}th row");
        assert_eq!(!prepared.sketches.is_empty(), read, "every {step}th row, the sketch read");
    }
}
