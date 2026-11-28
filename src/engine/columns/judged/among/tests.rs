//! Unit tests of a join's keys asked by a scan (`among.rs`, RULES.md
//! D1294).

use std::borrow::Cow;
use std::sync::Arc;

use super::super::super::group::{Group, Pages, Seal};
use super::super::super::prepared::Prepared;
use super::super::{Judgment, KEPT_SHARE};
use super::*;
use crate::disk::buffer::{BufferPool, FileId};
use crate::engine::{MEMORY_POOL_BYTES, memory_file};
use crate::error::Result;
use crate::index::Probe;
use crate::memory::MemoryBudget;
use crate::storage::{Condition, Conjunct, Conjunction, Held, Mask, Row};
use crate::value::{CompOp, DataType, Decimal, Spelled, Value};

const INTEGER: DataType = DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer };
/// K, a number of its row; Q, the same for four rows running; T, a text.
const TYPES: [DataType; 3] = [INTEGER, INTEGER, DataType::varying(20)];
const ROWS: u32 = 10_000;
const RUN: u32 = 2048;

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

fn number(n: u32) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// A group of `ROWS` rows of `TYPES`.
fn group(pages: &Pages<'_, Plain>) -> Group {
    let ids: Vec<u64> = (1..=u64::from(ROWS)).collect();
    let rows: Vec<Row> =
        (0..ROWS).map(|n| vec![number(n), number(n.checked_div(4).unwrap()), Value::Char(format!("text {n:05}").into())]).collect();
    Group::write(pages, &TYPES, (&ids, &rows), (&[], &[], &[])).unwrap()
}

/// The keys `keys`, a set, a scan's first `tried` runs counted.
fn members(keys: impl Iterator<Item = i64>, tried: usize) -> Arc<Members> {
    Arc::new(Members::new(Held::Keys(keys.collect()), 0, tried))
}

/// The keys `members` told of `column`.
fn among(column: usize, members: &Arc<Members>) -> Conjunct {
    Conjunct::Among { column, members: Arc::clone(members) }
}

/// What each run of `group` gave by `judgment`: the rows kept, as their
/// places in the group, and how many columns were decoded to judge them.
fn judged(judgment: &Judgment, group: &Group, pages: &Pages<'_, Plain>) -> Vec<(Vec<u32>, usize)> {
    let mut prepared = Prepared::default();
    let mut out = Vec::new();
    for from in (0..ROWS).step_by(usize::try_from(RUN).unwrap()) {
        let to = from.saturating_add(RUN).min(ROWS);
        let run = judgment.judge(group, (pages, &mut prepared), (&TYPES[..], (from, to)), None).unwrap();
        out.push((run.kept.iter().map(|r| from.saturating_add(*r)).collect(), run.decoded.len()));
    }
    out
}

/// The rows of each run of `ROWS` that `keeps` is true of.
fn wanted(keeps: impl Fn(u32) -> bool) -> Vec<Vec<u32>> {
    (0..ROWS)
        .step_by(usize::try_from(RUN).unwrap())
        .map(|from| (from..from.saturating_add(RUN).min(ROWS)).filter(|&n| keeps(n)).collect())
        .collect()
}

/// Verifies: [DESIGN] RULES.md D1294: a join's keys keep of a vector of
/// integers the rows whose key they hold, of every row and of rows
/// given, a null never, whatever unit lies at its place; of integers
/// kept as runs the same rows, asked once a run.
#[test]
fn a_joins_keys_keep_the_rows_whose_key_they_hold() {
    let keys = members([3_i64, 7, 20].into_iter(), 16);
    let units: Vec<i64> = (0_i32..40_i32).map(|n| i64::from(n % 10_i32) * 3 + i64::from(n % 3_i32)).collect();
    let nulls = Mask::from_bools(&(0_i32..40_i32).map(|n| n == 1_i32 || n == 21_i32).collect::<Vec<_>>());
    let ints = Ints::new(units.clone(), 0, Some(nulls));
    let holds = |r: &u32| ![1, 21].contains(r) && [3, 7, 20].contains(&units[index(*r)]);
    let all: Vec<u32> = (0..40).collect();
    let third: Vec<u32> = (0..40).step_by(3).collect();
    assert_eq!(kept(&keys, &ints, None), all.iter().copied().filter(holds).collect::<Vec<u32>>());
    assert_eq!(kept(&keys, &ints, Some(&third)), third.iter().copied().filter(holds).collect::<Vec<u32>>());
    assert_ne!(kept(&keys, &ints, None), Vec::<u32>::new(), "some row is kept");
    let sorted: Vec<i64> = (0..40_i64).map(|n| n.checked_div(4).unwrap()).collect();
    let runs = Ints::new(sorted.clone(), 0, None).with_runs((1..=10).map(|n: usize| n.saturating_mul(4)).collect());
    assert!(runs.runs().is_some());
    let in_runs = |r: &u32| [3, 7, 20].contains(&sorted[index(*r)]);
    assert_eq!(kept(&keys, &runs, None), all.iter().copied().filter(in_runs).collect::<Vec<u32>>());
    assert_eq!(kept(&keys, &runs, Some(&third)), third.iter().copied().filter(in_runs).collect::<Vec<u32>>());
}

/// Verifies: [DESIGN] RULES.md D1294: a scan told a join's keys of a
/// column of integers keeps of each run the rows whose key they hold,
/// its column decoded once for the run; beside a comparison it is asked
/// of the rows the comparison kept; of a column kept as runs the same;
/// and keys told of a column of texts are not asked at all.
#[test]
fn a_scan_asks_a_joins_keys_of_each_run() {
    let (pool, file) = file();
    let pages = pages(&pool, file);
    let group = group(&pages);
    let hundredth = || members((0..i64::from(ROWS)).step_by(100), 16);
    let alone = Judgment::of(Some(&Conjunction(vec![among(0, &hundredth())])), &TYPES);
    let gave = judged(&alone, &group, &pages);
    assert_eq!(gave.iter().map(|g| g.0.clone()).collect::<Vec<_>>(), wanted(|n| n % 100 == 0));
    assert!(gave.iter().all(|g| g.1 == 1), "the key's column decoded once a run");
    let below = Conjunct::Condition(Condition { columns: vec![0], probe: Probe::Compare { op: CompOp::Lt, value: number(5_000) } });
    let both = Judgment::of(Some(&Conjunction(vec![among(0, &hundredth()), below])), &TYPES);
    let kept_rows: Vec<Vec<u32>> = judged(&both, &group, &pages).into_iter().map(|g| g.0).collect();
    assert_eq!(kept_rows, wanted(|n| n % 100 == 0 && n < 5_000));
    let quarters = Judgment::of(Some(&Conjunction(vec![among(1, &members([5_i64, 2_000].into_iter(), 16))])), &TYPES);
    let of_runs: Vec<Vec<u32>> = judged(&quarters, &group, &pages).into_iter().map(|g| g.0).collect();
    assert_eq!(of_runs, wanted(|n| [5, 2_000].contains(&n.checked_div(4).unwrap())));
    assert!(Judgment::of(Some(&Conjunction(vec![among(2, &hundredth())])), &TYPES).is_empty(), "of texts, not asked");
}

/// Verifies: [DESIGN] RULES.md D1294 (MC/DC): keys that keep one row in
/// `KEPT_SHARE` or more of a scan's first runs are asked of those runs
/// and of none after: every row of a later run is given, its column not
/// decoded for them; keys that keep fewer go on being asked of every
/// run.
#[test]
fn a_scan_stops_asking_keys_that_keep_most_rows() {
    let (pool, file) = file();
    let pages = pages(&pool, file);
    let group = group(&pages);
    let share = u32::try_from(KEPT_SHARE).unwrap();
    let dense = members((0..i64::from(ROWS)).step_by(usize::try_from(share).unwrap()), 2);
    let gave = judged(&Judgment::of(Some(&Conjunction(vec![among(0, &dense)])), &TYPES), &group, &pages);
    let asked = wanted(|n| n % share == 0);
    let all = wanted(|_| true);
    assert_eq!((&gave[0].0, &gave[1].0), (&asked[0], &asked[1]), "asked of its first two runs");
    assert!(gave.iter().take(2).all(|g| g.1 == 1));
    assert_eq!((&gave[2].0, &gave[4].0), (&all[2], &all[4]), "every row of the runs after");
    assert!(gave.iter().skip(2).all(|g| g.1 == 0), "and no column decoded for them");
    let sparse = members((0..i64::from(ROWS)).step_by(usize::try_from(share).unwrap().saturating_add(1)), 2);
    let went_on = judged(&Judgment::of(Some(&Conjunction(vec![among(0, &sparse)])), &TYPES), &group, &pages);
    let fewer = wanted(|n| n % share.saturating_add(1) == 0);
    assert_eq!(went_on.iter().map(|g| g.0.clone()).collect::<Vec<_>>(), fewer, "asked of every run");
    assert!(went_on.iter().all(|g| g.1 == 1));
}
