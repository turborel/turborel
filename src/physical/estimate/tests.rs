//! Unit tests of a conjunction's estimate: on the row sample, by ranges
//! of one column, by an index's joint distinct keys, and by backoff
//! (RULES.md R-TEST-1, R-TEST-8).

use super::sampled::eligible;
use super::*;
use crate::logical::Slot;
use crate::physical::CostModel;
use crate::stats::{Statistics, StatsBuilder, TableStats};
use crate::value::{DataType, Decimal, Spelled};

fn number(n: u64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

fn eq(column: usize, n: u64) -> Cond {
    Cond::Compare(Expr::Column(column), CompOp::Eq, Expr::Const(number(n)))
}

fn op(column: usize, op: CompOp, n: u64) -> Cond {
    Cond::Compare(Expr::Column(column), op, Expr::Const(number(n)))
}

/// The statistics of `rows` rows: A of 0 to 999 in turn, B = A mod 10, C
/// = the row's number mod 7 and D = A mod 100, so B and D go with A and
/// with each other, C with none.
fn correlated(rows: u64) -> TableStats {
    let mut builder = StatsBuilder::new(4);
    for i in 0..rows {
        builder.add(&vec![number(i % 1_000), number(i % 1_000 % 10), number(i % 7), number(i % 100)], 9);
    }
    builder.finish()
}

/// The share `conds` keep, by `stats`.
fn share(stats: &dyn Statistics, conds: &[Cond]) -> f64 {
    let cost = CostModel::default();
    conjunction(conds, Known { stats: Some(stats), cost: &cost })
}

/// Verifies: [DESIGN] RULES.md D382, the sample: of two correlated
/// columns (B = D mod 10), a conjunction of both is estimated on the
/// sample within two times of the truth, where the product of each
/// column's share is ten times below it; a conjunction the sample keeps
/// no row of is bounded by (0 + 3)/n, the empty sample's bound; one of
/// independent columns stays near the truth.
#[test]
fn a_correlated_conjunction_is_estimated_on_the_sample() {
    let stats = correlated(20_000);
    let truth = 1.0_f64 / 100.0_f64;
    let got = share(&stats, &[eq(3, 5), eq(1, 5)]);
    assert!(got / truth < 2.0_f64 && truth / got < 2.0_f64, "{got} for {truth}");
    let independent = stats.selectivity(3, CompOp::Eq, &number(5)).unwrap() * stats.selectivity(1, CompOp::Eq, &number(5)).unwrap();
    assert!(truth / independent > 5.0_f64, "independence is far off: {independent}");
    let none = share(&stats, &[eq(0, 5), eq(1, 6)]);
    assert!(none <= 3.0_f64 / 1_024.0_f64, "the empty sample's bound: {none}");
    let apart = share(&stats, &[op(0, CompOp::Lt, 500), eq(2, 3)]);
    let truth_apart = 0.5_f64 / 7.0_f64;
    assert!(apart / truth_apart < 1.5_f64 && truth_apart / apart < 1.5_f64, "{apart} for {truth_apart}");
}

/// Verifies: [DESIGN] RULES.md D382, MC/DC of the sample's eligibility,
/// each condition shown to decide alone against the case that samples
/// (two conjuncts of two columns of a table of 20,000 rows with a
/// sample): no sample (not), one conjunct a synopsis answers (not) and a conjunct not
/// evaluable, of a `?` (not); a lone conjunct no synopsis answers, a
/// comparison of two columns, is sampled.
#[test]
fn the_sample_is_asked_only_of_what_it_can_answer() {
    let cost = CostModel::default();
    let large = correlated(20_000);
    let (a, b) = (eq(0, 5), eq(1, 5));
    assert!(eligible(&[&a, &b], known(&large, &cost)).is_some(), "sampled");
    let unsampled = TableStats { sample: None, ..large.clone() };
    assert!(eligible(&[&a, &b], known(&unsampled, &cost)).is_none(), "no sample");
    assert!(eligible(&[&a], known(&large, &cost)).is_none(), "one conjunct a synopsis answers");
    let int = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    let given = Cond::Compare(Expr::Column(1), CompOp::Eq, Expr::Dynamic(Slot { index: 0, data_type: int, at: 0 }));
    assert!(eligible(&[&a, &given], known(&large, &cost)).is_none(), "a ? not yet given");
    let columns = Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Column(1));
    assert!(eligible(&[&columns], known(&large, &cost)).is_some(), "a comparison of columns");
    let truth = 10.0_f64 / 1_000.0_f64;
    let got = share(&large, &[columns]);
    assert!(got / truth < 2.0_f64 && truth / got < 2.0_f64, "A = B: {got} for {truth}");
}

/// What an estimate knows of `stats` by `cost`.
fn known<'a>(stats: &'a TableStats, cost: &'a CostModel) -> Known<'a> {
    Known { stats: Some(stats), cost }
}

/// Verifies: [DESIGN] RULES.md D382, one column's conjuncts: a lower and
/// an upper bound of one column keep the rows between them, not the
/// product of each half's share; a further bound on it keeps no more than
/// it allows; of unknown statistics the halves are taken as independent.
#[test]
fn bounds_of_one_column_are_one_range() {
    let stats = correlated(5_000);
    let between = share(&stats, &[op(0, CompOp::Ge, 100), op(0, CompOp::Lt, 200)]);
    assert!((between - 0.1_f64).abs() < 0.02_f64, "{between}");
    let tighter = share(&stats, &[op(0, CompOp::Ge, 100), op(0, CompOp::Lt, 200), op(0, CompOp::Gt, 150)]);
    assert!(tighter < between && (tighter - 0.05_f64).abs() < 0.02_f64, "{tighter}");
    let cost = CostModel::default();
    let unknown = conjunction(&[op(0, CompOp::Ge, 100), op(0, CompOp::Lt, 200)], Known { stats: None, cost: &cost });
    assert!((unknown - cost.range_selectivity * cost.range_selectivity).abs() < 1e-9_f64);
}

/// Verifies: [DESIGN] RULES.md D382, backoff: of conjuncts on two
/// columns of a table that keeps no sample, the most selective counts whole and the
/// other by its square root.
#[test]
fn conjuncts_no_sample_answers_combine_by_backoff() {
    let stats = TableStats { sample: None, ..correlated(5_000) };
    let (a, c) = (stats.selectivity(0, CompOp::Lt, &number(100)).unwrap(), stats.selectivity(2, CompOp::Eq, &number(3)).unwrap());
    let got = share(&stats, &[op(0, CompOp::Lt, 100), eq(2, 3)]);
    let (low, high) = if a < c { (a, c) } else { (c, a) };
    assert!((got - low * high.sqrt()).abs() < 1e-9_f64, "{got}");
}

/// Verifies: [DESIGN] RULES.md D382, rare equalities: of two correlated
/// columns (D = A mod 100), equalities too rare for the sample to count,
/// one row in 1,000, are estimated by the columns' joint distinct values
/// within two times of the truth, where backoff is ten times below it;
/// a combination the sample never holds, of columns whose every
/// combination it saw more than once, keeps no row; one that may exist
/// unseen keeps at most the empty sample's bound.
#[test]
fn rare_equalities_are_estimated_by_joint_distinct_values() {
    let stats = correlated(20_000);
    let truth = 1.0_f64 / 1_000.0_f64;
    let got = share(&stats, &[eq(0, 15), eq(3, 15)]);
    assert!(got / truth < 2.0_f64 && truth / got < 2.0_f64, "{got} for {truth}");
    assert!(share(&stats, &[eq(1, 2), eq(2, 3), eq(3, 12)]) > 0.0_f64, "a combination seen");
    assert_eq!(share(&stats, &[eq(1, 2), eq(3, 13)]), 0.0_f64, "never together: B is D mod 10");
    let unseen = share(&stats, &[eq(0, 15), eq(3, 16)]);
    assert!(unseen <= 3.0_f64 / 1_024.0_f64, "{unseen}");
}

/// Verifies: [DESIGN] RULES.md D382, a small table: of a table its sample
/// holds whole, a conjunction is its share of the sample however few rows
/// it keeps, none if none; of a sample of half a table, a conjunction it
/// keeps no row of is bounded by 3(1 - 1/2)/n, below the rule of three.
#[test]
fn a_sample_of_the_whole_table_is_exact() {
    let small = correlated(1_000);
    assert_eq!(share(&small, &[eq(0, 5), eq(2, 5)]), 1.0_f64 / 1_000.0_f64, "one row of 1,000: A = 5 is row 5, of C = 5");
    assert_eq!(share(&small, &[eq(0, 5), eq(2, 6)]), 0.0_f64);
    let half = correlated(2_048);
    let none = share(&half, &[op(0, CompOp::Lt, 10), eq(2, 3), op(3, CompOp::Gt, 50)]);
    assert!(none <= 1.5_f64 / 1_024.0_f64 + 1e-12_f64, "{none}");
}

/// Verifies: [DESIGN] RULES.md D382, the sample's memory: a conjunction
/// is remembered under one key whatever the order of its conjuncts, and
/// under another of another constant, column, operator or question, and
/// 5 and 5.0, identical by the standard, are one constant.
#[test]
fn the_sample_remembers_by_the_conjuncts_structure() {
    let key = |question: &str, conds: &[Cond]| fingerprint::key_of(question, &conds.iter().collect::<Vec<&Cond>>(), &|_| {}).key();
    let both = key("kept", &[eq(0, 5), eq(2, 6)]);
    assert_eq!(both, key("kept", &[eq(2, 6), eq(0, 5)]));
    let tenths = Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Const(Value::Exact(Decimal::new(50, 1))));
    assert_eq!(both, key("kept", &[tenths, eq(2, 6)]));
    for other in [
        key("kept", &[eq(0, 5), eq(2, 7)]),
        key("kept", &[eq(1, 5), eq(2, 6)]),
        key("kept", &[op(0, CompOp::Lt, 5), eq(2, 6)]),
        key("like", &[eq(0, 5), eq(2, 6)]),
    ] {
        assert_ne!(both, other);
    }
}

/// Verifies: [DESIGN] RULES.md D382, an IN list written as a chain of
/// ORs: of one column, a synopsis answers it, so alone it is estimated as
/// its disjuncts are, near the share of its 64 values, and never
/// sampled; an OR of two columns'
/// equalities anywhere along the chain is not answered.
#[test]
fn a_chain_of_ors_of_one_column_is_answered_by_its_synopsis() {
    let cost = CostModel::default();
    let large = correlated(20_000);
    let one = crate::logical::or_all((0..64).map(|n| eq(0, n * 7)).collect()).unwrap();
    assert!(sampled::answered(&one));
    assert!(eligible(&[&one], known(&large, &cost)).is_none());
    let got = conjunction(std::slice::from_ref(&one), known(&large, &cost));
    assert_eq!(got, selectivity(&one, known(&large, &cost)));
    assert!(got > 0.03_f64 && got < 0.07_f64, "64 of 1,000 values: {got}");
    let mut equalities: Vec<Cond> = (0..64).map(|n| eq(0, n * 7)).collect();
    equalities.insert(40, eq(1, 3));
    let two = crate::logical::or_all(equalities).unwrap();
    assert!(!sampled::answered(&two));
}

/// A function of one argument named HELD, that holds `.0`.
struct Held(u64);

impl crate::functions::ScalarFunction for Held {
    fn name(&self) -> &'static str {
        "HELD"
    }
    fn return_type(&self, _: &[DataType]) -> crate::error::Result<DataType> {
        Ok(DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric })
    }
    fn call(&self, _: &[Value]) -> crate::error::Result<Value> {
        Ok(number(self.0))
    }
}

/// "`f`(column 0) = 5".
fn held_is_five(f: &crate::functions::ScalarRef) -> Cond {
    Cond::Compare(Expr::Call(f.clone(), vec![Expr::Column(0)]), CompOp::Eq, Expr::Const(number(5)))
}

/// Verifies: [DESIGN] RULES.md D1299, D382: the sample remembers a
/// conjunct of a function by the function's handle, not by its name
/// alone. Two functions of one name that hold what differs are two
/// questions, each with its own answer, and one function asked twice is
/// one question, answered once.
///
/// Regression test: a function once wrote its name alone, so the second
/// of two that held what differed was given the answer of the first.
#[test]
fn two_functions_of_one_name_are_two_questions() {
    let handle = |held: u64| crate::functions::ScalarRef(std::sync::Arc::new(Held(held)));
    let (five, six) = (handle(5), handle(6));
    let key = |f: &crate::functions::ScalarRef| fingerprint::key_of("kept", &[&held_is_five(f)], &|_| {}).key();
    assert_ne!(key(&five), key(&six), "two handles of one name");
    assert_eq!(key(&five), key(&five), "one handle");
    let stats = correlated(20_000);
    assert_eq!(share(&stats, &[held_is_five(&five)]), 1.0_f64, "HELD of 5 is 5 of every row");
    let none = share(&stats, &[held_is_five(&six)]);
    assert!(none <= 3.0_f64 / 1_024.0_f64, "HELD of 6 is 5 of none, the empty sample's bound: {none}");
    assert_eq!(share(&stats, &[held_is_five(&five)]), 1.0_f64, "and the first keeps its own");
}

/// Verifies: [DESIGN] RULES.md D1299: an answer the sample remembers
/// keeps the handles its key was written of, so none is freed and its
/// address taken by another function while the answer may be given, and
/// lets them go when the statistics are dropped.
#[test]
fn a_remembered_answer_keeps_its_functions_until_the_statistics_go() {
    let of = std::sync::Arc::new(Held(5));
    let five = crate::functions::ScalarRef(of.clone());
    let stats = correlated(20_000);
    let before = std::sync::Arc::strong_count(&of);
    assert_eq!(share(&stats, &[held_is_five(&five)]), 1.0_f64);
    let kept = std::sync::Arc::strong_count(&of);
    assert!(kept > before, "kept beside the answer: {kept} for {before}");
    assert_eq!(share(&stats, &[held_is_five(&five)]), 1.0_f64);
    assert_eq!(std::sync::Arc::strong_count(&of), kept, "once, however often it is asked");
    drop(stats);
    assert_eq!(std::sync::Arc::strong_count(&of), before, "let go with the statistics");
}
