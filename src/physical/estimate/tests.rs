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
/// sample): a table of 5,000 rows, under `SAMPLED_FROM` (not sampled), no sample
/// (not), one conjunct a synopsis answers (not) and a conjunct not
/// evaluable, of a `?` (not); a lone conjunct no synopsis answers, a
/// comparison of two columns, is sampled.
#[test]
fn the_sample_is_asked_only_of_what_it_can_answer() {
    let cost = CostModel::default();
    let large = correlated(20_000);
    let (a, b) = (eq(0, 5), eq(1, 5));
    assert!(eligible(&[&a, &b], known(&large, &cost)).is_some(), "sampled");
    assert!(eligible(&[&a, &b], known(&correlated(5_000), &cost)).is_none(), "too small");
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
/// columns no sample answers, the most selective counts whole and the
/// other by its square root.
#[test]
fn conjuncts_no_sample_answers_combine_by_backoff() {
    let stats = correlated(5_000);
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
