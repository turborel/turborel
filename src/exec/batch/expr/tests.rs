//! Unit tests of expressions over vectors (`expr.rs`): random expressions
//! over a batch of every kind of vector, of a fixed seed, each evaluated
//! a vector at a time and a row at a time, and cases the generator rarely
//! makes.

use super::*;
use crate::error::Result;
use crate::exec::NoSubqueries;
use crate::exec::eval::eval;
use crate::kernels::Portable;
use crate::storage::{Bools, Datetimes, Floats, Ints, Mask, Texts};
use crate::value::datetime::{MICROS_PER_DAY, Moment};
use crate::value::{Decimal, Spelled};

/// Rows of the batch, past two words of a mask.
const ROWS: usize = 150;
/// Columns of the batch.
const COLUMNS: usize = 8;
/// The columns of numbers: integers of scale 0, of scale 2, approximate
/// numbers, values, and integers known as runs.
const NUMBERS: [usize; 5] = [0, 1, 2, 4, 7];
/// The strings the column of texts holds.
const WORDS: [&str; 7] = ["a", "ab", "B", "abc", "", "zz", "a%"];

/// A step of a xorshift generator from `state`, then a number below `n`:
/// the same expressions every run.
fn below(state: &mut u64, n: usize) -> usize {
    *state ^= *state << 13_u32;
    *state ^= *state >> 7_u32;
    *state ^= *state << 17_u32;
    usize::try_from(state.checked_rem(u64::try_from(n).unwrap()).unwrap()).unwrap()
}

/// A number below `n`, signed, centered on 0.
fn signed(state: &mut u64, n: usize) -> i64 {
    i64::try_from(below(state, n)).unwrap().checked_sub(i64::try_from(n.checked_div(2).unwrap()).unwrap()).unwrap()
}

/// The mask of every `k`th row null, from `from`.
fn every(k: usize, from: usize) -> Mask {
    Mask::from_bools(&(0..ROWS).map(|n| n.checked_rem(k) == Some(from)).collect::<Vec<_>>())
}

fn exact(units: i128, scale: u32) -> Value {
    Value::Exact(Decimal::new(units, scale))
}

/// A batch of every kind: integers of scales 0 and 2, approximate
/// numbers, texts, values, truth values, dates, and integers in runs;
/// nulls among all but the runs, a null's slot not always 0.
fn batch(state: &mut u64) -> Batch {
    let mut big = |n: usize| if n % 31 == 3 { 900_000_000_000_000_000 } else { signed(state, 100) };
    let first: Vec<i64> = (0..ROWS).map(&mut big).collect();
    let second: Vec<i64> = (0..ROWS).map(|_| signed(state, 20_000)).collect();
    let floats: Vec<f64> = (0..ROWS).map(|_| f64::from(i32::try_from(signed(state, 400)).unwrap()) / 4.0_f64).collect();
    let words: Vec<&str> = (0..ROWS).map(|_| WORDS[below(state, WORDS.len())]).collect();
    let ends = words
        .iter()
        .scan(0, |end, w| {
            *end = w.len().checked_add(*end).unwrap();
            Some(*end)
        })
        .collect();
    let values = (0..ROWS).map(|n| if n % 3 == 0 { Value::Null } else { exact(i128::from(signed(state, 50)), 1) }).collect();
    let bits = Mask::from_bools(&(0..ROWS).map(|_| below(state, 2) == 1).collect::<Vec<_>>());
    let days = (0..ROWS).map(|_| i64::try_from(below(state, 40)).unwrap().checked_mul(MICROS_PER_DAY).unwrap()).collect();
    let runs: Vec<i64> = (0..ROWS).map(|n| i64::try_from(n.checked_div(20).unwrap()).unwrap()).collect();
    Batch {
        columns: vec![
            Column::Ints(Ints::new(first, 0, Some(every(7, 1)))),
            Column::Ints(Ints::new(second, 2, Some(every(5, 0)))),
            Column::Floats(Floats::new(floats, Some(every(6, 2)))),
            Column::Texts(Texts::new(words.concat(), ends, Some(every(4, 3)))),
            Column::Values(values),
            Column::Bools(Bools::new(bits, Some(every(9, 4)))),
            Column::Datetimes(Datetimes::new(days, None, (Moment::Date, false, 0), Some(every(8, 5)))),
            Column::Ints(Ints::new(runs, 0, None).with_runs((20..ROWS).step_by(20).chain([ROWS]).collect())),
        ],
        rows: ROWS,
    }
}

/// A constant number: small, of a scale, approximate, null, or large.
fn number(state: &mut u64) -> Expr {
    Expr::Const(match below(state, 6) {
        0 => exact(i128::from(signed(state, 20)), 0),
        1 => exact(i128::from(signed(state, 2_000)), 3),
        2 => Value::Approx(f64::from(i32::try_from(signed(state, 40)).unwrap()) / 8.0_f64),
        3 => Value::Null,
        4 => exact(0, 0),
        _ => exact(999_999_999_999_999, 0),
    })
}

/// A random expression of numbers, `depth` levels at most.
fn value(state: &mut u64, depth: usize) -> Expr {
    let leaf = depth == 0 || below(state, 4) == 0;
    if leaf {
        return if below(state, 3) == 0 { number(state) } else { Expr::Column(NUMBERS[below(state, NUMBERS.len())]) };
    }
    let deeper = depth.saturating_sub(1);
    match below(state, 6) {
        0 => Expr::Neg(Box::new(value(state, deeper))),
        1 | 2 => {
            let op = [ArithOp::Add, ArithOp::Sub, ArithOp::Mul, ArithOp::Div][below(state, 4)];
            Expr::Arith(Box::new(value(state, deeper)), op, Box::new(value(state, deeper)))
        }
        3 => Expr::Case(vec![(cond(state, deeper), value(state, deeper))], Box::new(value(state, deeper))),
        4 => {
            let to = match below(state, 3) {
                0 => DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric },
                1 => DataType::Exact { precision: 18, scale: 3, spelled: Spelled::Numeric },
                _ => DataType::Approximate { precision: 53, spelled: Spelled::Numeric },
            };
            Expr::Cast(Box::new(value(state, deeper)), to)
        }
        _ => Expr::Case(
            vec![(cond(state, deeper), value(state, deeper)), (cond(state, deeper), value(state, deeper))],
            Box::new(Expr::Const(Value::Null)),
        ),
    }
}

/// A random condition, `depth` levels at most.
fn cond(state: &mut u64, depth: usize) -> Cond {
    let deeper = depth.saturating_sub(1);
    let ops = [CompOp::Eq, CompOp::Ne, CompOp::Lt, CompOp::Le, CompOp::Gt, CompOp::Ge];
    let op = ops[below(state, ops.len())];
    let nested = depth > 0;
    match below(state, if nested { 12 } else { 8 }) {
        0 | 1 => Cond::Compare(value(state, deeper), op, value(state, deeper)),
        2 => Cond::Compare(Expr::Column(3), op, Expr::Const(Value::Char(WORDS[below(state, WORDS.len())].to_owned()))),
        3 => Cond::Compare(Expr::Column(6), op, Expr::Const(date(state))),
        4 => Cond::IsNull(value(state, deeper), below(state, 2) == 1),
        5 => Cond::Like(Expr::Column(3), Expr::Const(Value::Char(["a%", "_b%", "%!%", "B"][below(state, 4)].to_owned())), escape(state)),
        6 => Cond::In(value(state, deeper), Arc::new(InSet::of(vec![exact(1, 0), exact(-3, 0), exact(25, 1), Value::Null]))),
        7 => Cond::Value(Expr::Column(5)),
        8 => Cond::And(Box::new(cond(state, deeper)), Box::new(cond(state, deeper))),
        9 => Cond::Or(Box::new(cond(state, deeper)), Box::new(cond(state, deeper))),
        10 => Cond::Not(Box::new(cond(state, deeper))),
        _ => match below(state, 2) {
            0 => {
                Cond::Is(Box::new(cond(state, deeper)), [Truth::True, Truth::False, Truth::Unknown][below(state, 3)], below(state, 2) == 1)
            }
            _ => Cond::Distinct(value(state, deeper), value(state, deeper)),
        },
    }
}

/// A date constant within the column's.
fn date(state: &mut u64) -> Value {
    let day = i64::try_from(below(state, 40)).unwrap();
    Value::Datetime(crate::value::datetime::Datetime::of_local(Moment::Date, day.checked_mul(MICROS_PER_DAY).unwrap(), 0, false, 0))
}

/// An escape character, or none.
fn escape(state: &mut u64) -> Option<Expr> {
    (below(state, 2) == 1).then(|| Expr::Const(Value::Char("!".to_owned())))
}

/// Each row's value of `e`, given `rows` of `batch`, evaluated a row at a
/// time.
fn by_rows(e: &Expr, batch: &Batch, rows: Option<&[u32]>) -> Vec<Result<Value>> {
    let count = rows.map_or(batch.rows, <[u32]>::len);
    (0..count)
        .map(|n| {
            let r = crate::storage::sel::nth(rows, n);
            let row: Vec<Value> = (0..COLUMNS).map(|c| batch.value(c, r).cloned().unwrap_or(Value::Null)).collect();
            eval(e, &row, &NoSubqueries)
        })
        .collect()
}

/// How `e` evaluated over vectors compares with a row's evaluation:
/// whether it was evaluated so, after asserting it gives a row's values,
/// and gives none if a row's raises.
fn checked(e: &Expr, batch: &Batch, rows: Option<&[u32]>) -> Option<bool> {
    let expected = by_rows(e, batch, rows);
    let program = Program::of(e.clone(), (0..COLUMNS).collect());
    let made = program.column(batch, rows, &NoSubqueries);
    let raised = expected.iter().any(Result::is_err);
    let Some(made) = made else { return (!raised).then_some(false) };
    assert!(!raised, "{e:?} of {rows:?}: a row raises, the vectors do not");
    let found: Vec<String> = made.into_values().iter().map(|v| format!("{v:?}")).collect();
    let wanted: Vec<String> = expected.into_iter().map(|v| format!("{:?}", v.unwrap())).collect();
    assert_eq!(found, wanted, "{e:?} of {rows:?}");
    Some(true)
}

/// Verifies: [DESIGN] RULES.md D362, D363 [X3.135-1986 §5.9, §5.11,
/// §5.13-§5.15, §5.18; ISO/IEC 9075:1992 §6.9, §6.10, §8.12]: random
/// expressions of a fixed seed, over every kind of vector, nulls among
/// them, of every row and of a selection, give over vectors what a row's
/// evaluation gives each row, or none if any row's raises an error;
/// most are evaluated over vectors.
#[test]
fn random_expressions_give_what_rows_give() {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let batch = batch(&mut state);
    let some: Vec<u32> = (0..u32::try_from(ROWS).unwrap()).filter(|n| n % 3 != 1).collect();
    let (mut vectored, mut replayed) = (0_usize, 0_usize);
    for case in 0..1_500_u32 {
        let e = if case % 2 == 0 { value(&mut state, 4) } else { Expr::Truth(Box::new(cond(&mut state, 3))) };
        for rows in [None, Some(some.as_slice())] {
            match checked(&e, &batch, rows) {
                Some(true) => vectored += 1,
                Some(false) => replayed += 1,
                None => {}
            }
        }
    }
    assert!(vectored > replayed * 4, "{vectored} over vectors, {replayed} replayed without an error");
}

/// Verifies: [DESIGN] RULES.md D362 [X3.135-1986 §5.18]: a filter's rows
/// kept over vectors are those whose condition a row's evaluation finds
/// true, a null or false not, of every row and of a selection.
#[test]
fn rows_kept_are_those_rows_find_true() {
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let batch = batch(&mut state);
    let some: Vec<u32> = (0..u32::try_from(ROWS).unwrap()).filter(|n| n % 4 != 2).collect();
    for _ in 0..400_u32 {
        let e = Expr::Truth(Box::new(cond(&mut state, 3)));
        let program = Program::of(e.clone(), (0..COLUMNS).collect());
        for rows in [None, Some(some.as_slice())] {
            let expected = by_rows(&e, &batch, rows);
            let Some(kept) = program.kept(&batch, rows, (&NoSubqueries, &Portable)) else { continue };
            let wanted: Vec<u32> = expected
                .iter()
                .enumerate()
                .filter(|(_, v)| matches!(v, Ok(Value::Bool(true))))
                .map(|(n, _)| u32::try_from(crate::storage::sel::nth(rows, n)).unwrap())
                .collect();
            assert_eq!(kept, wanted, "{e:?} of {rows:?}");
        }
    }
}

/// A batch of one column of integers of scale 0, `units` its values,
/// `nulls` its nulls.
fn integers(units: Vec<i64>, nulls: Option<Mask>) -> Batch {
    let rows = units.len();
    Batch { columns: vec![Column::Ints(Ints::new(units, 0, nulls))], rows }
}

fn int(n: i128) -> Expr {
    Expr::Const(exact(n, 0))
}

fn arith(a: Expr, op: ArithOp, b: Expr) -> Expr {
    Expr::Arith(Box::new(a), op, Box::new(b))
}

/// The vector of `e` of every row of `batch`, its columns the batch's,
/// each value as it shows in a debug.
fn of(e: Expr, batch: &Batch) -> Option<Vec<String>> {
    let inputs = (0..batch.columns.len()).collect();
    Program::of(e, inputs).column(batch, None, &NoSubqueries).map(|c| shown(&c.into_values()))
}

/// Each of `values` as it shows in a debug.
fn shown(values: &[Value]) -> Vec<String> {
    values.iter().map(|v| format!("{v:?}")).collect()
}

/// Verifies: [X3.135-1986 §5.9 GR6a; ISO/IEC 9075:1992 §6.9 GR2]: a
/// result past 64 bits, or of 19 digits, gives none, so the batch is
/// evaluated a row at a time and raises the row's error; one of a null
/// row does not, its garbage slot never read; a CASE whose untaken branch
/// divides by 0 raises nothing, each branch evaluated of its own rows.
#[test]
fn overflows_give_none_and_untaken_branches_none() {
    let column = || Expr::Column(0);
    let batch = integers(vec![3, -7, 0, 99], None);
    assert!(of(arith(column(), ArithOp::Mul, int(200_000_000_000_000_000)), &batch).is_none(), "19 digits");
    assert!(of(arith(column(), ArithOp::Mul, int(i128::from(i64::MAX))), &batch).is_none(), "past 64 bits");
    let nulled = integers(vec![i64::MAX, 2], Some(Mask::from_bools(&[true, false])));
    assert_eq!(of(arith(column(), ArithOp::Add, int(1)), &nulled), Some(shown(&[Value::Null, exact(3, 0)])), "of a null, no flag");
    let ne = Cond::Compare(column(), CompOp::Ne, int(0));
    let case = Expr::Case(vec![(ne, arith(int(100), ArithOp::Div, column()))], Box::new(int(-1)));
    let made = of(case, &batch).expect("no row divides by 0");
    let quotient = by_rows(&arith(int(100), ArithOp::Div, int(99)), &batch, None).remove(0).unwrap();
    assert_eq!(made.get(2..), Some(&shown(&[exact(-1, 0), quotient])[..]), "the ELSE of the row of 0");
}

/// Verifies: [X3.135-1986 §5.11 GR2, §5.18 GR2]: a comparison with NaN
/// is unknown over vectors as of a row; OR of unknown and true is true,
/// AND of unknown and false false, NOT unknown unknown.
#[test]
fn nan_compares_unknown_and_logic_is_three_valued() {
    let batch = Batch { columns: vec![Column::Floats(Floats::new(vec![f64::NAN, 1.0_f64], None))], rows: 2 };
    let unknown = || Cond::Compare(Expr::Column(0), CompOp::Gt, Expr::Const(Value::Approx(0.0_f64)));
    let truth = |c: Cond| of(Expr::Truth(Box::new(c)), &batch);
    assert_eq!(truth(unknown()), Some(shown(&[Value::Null, Value::Bool(true)])));
    let or = Cond::Or(Box::new(unknown()), Box::new(Cond::Const(Truth::True)));
    assert_eq!(truth(or), Some(shown(&[Value::Bool(true), Value::Bool(true)])));
    let and = Cond::And(Box::new(unknown()), Box::new(Cond::Const(Truth::False)));
    assert_eq!(truth(and), Some(shown(&[Value::Bool(false), Value::Bool(false)])));
    assert_eq!(truth(Cond::Not(Box::new(unknown()))), Some(shown(&[Value::Null, Value::Bool(false)])));
}

/// Verifies: [ISO/IEC 9075:1992 §8.12 GR2; ISO/IEC 9075-2:2023 §4.6.1]: a
/// null truth value of a vector whose bit is set reads unknown, not true:
/// IS TRUE false of it, IS UNKNOWN true, and a filter keeps it not.
#[test]
fn null_truth_values_read_unknown_whatever_their_bit() {
    let bits = Mask::from_bools(&[true, true, false]);
    let batch = Batch { columns: vec![Column::Bools(Bools::new(bits, Some(Mask::from_bools(&[true, false, false]))))], rows: 3 };
    let value = || Cond::Value(Expr::Column(0));
    let is = |t: Truth| of(Expr::Truth(Box::new(Cond::Is(Box::new(value()), t, false))), &batch);
    assert_eq!(is(Truth::True), Some(shown(&[Value::Bool(false), Value::Bool(true), Value::Bool(false)])));
    assert_eq!(is(Truth::Unknown), Some(shown(&[Value::Bool(true), Value::Bool(false), Value::Bool(false)])));
    let program = Program::of(Expr::Truth(Box::new(value())), vec![0]);
    assert_eq!(program.kept(&batch, None, (&NoSubqueries, &Portable)), Some(vec![1]));
}
