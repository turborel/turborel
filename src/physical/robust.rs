//! \[DESIGN\] A join's method chosen by its regret (RULES.md D386), not by
//! its cost at the left operand's estimate alone.
//!
//! An index nested-loop join probes once for each left row, so its cost
//! grows with the left's rows far faster than a hash join's, which reads
//! the right whole once. An estimate low by ten times can so make the
//! index join a hundred times dearer than the hash join it replaced, the
//! more so where its probes miss the pool (Leis et al., "How Good Are
//! Query Optimizers, Really?", 2015, §4.1). Each method's cost is linear
//! in the left's rows, and the rows are taken to lie between the estimate
//! `e` and a pessimistic `h`: the estimate times PLANNER.ESTIMATE_MARGIN,
//! at most the upper bound the statistics prove (`cardinality/bounds.rs`).
//! A method's regret at some rows is its cost there over the cheapest
//! method's. The method taken is the one whose larger regret at `e` and
//! at `h` is least, which for costs linear in the rows is its largest
//! regret over the whole interval: minimax regret, as Chu, Halpern and
//! Seshadri, "Least Expected Cost Query Optimization", PODS 1999, and
//! Babcock and Chaudhuri, "Towards a Robust Query Optimizer", SIGMOD 2005,
//! weigh plans over a range of estimates. A margin of 1 makes `h` the
//! estimate, and the cheaper method at it is taken, as before.

/// A cost linear in the left operand's rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Linear {
    /// What it costs of no rows.
    pub(super) fixed: f64,
    /// What each row adds.
    pub(super) per_row: f64,
}

impl Linear {
    /// The cost of `rows` rows.
    pub(super) fn at(self, rows: f64) -> f64 {
        self.fixed + self.per_row * rows
    }
}

/// The rows an operand estimated to give `rows`, of at most `upper`, is
/// taken to give at worst: `margin` times the estimate, at most the bound,
/// at least the estimate.
pub(super) fn pessimistic(rows: f64, upper: f64, margin: f64) -> f64 {
    (rows * margin.max(1.0_f64)).min(upper).max(rows)
}

/// The regret of a cost `cost` where the cheapest is `least`: how many
/// times the cheapest it is, 1 if it is the cheapest.
fn regret(cost: f64, least: f64) -> f64 {
    if cost <= least {
        return 1.0_f64;
    }
    cost / least.max(f64::MIN_POSITIVE)
}

/// Of `costs`, each a method's, the method whose larger regret at the
/// rows `e` and `h` is least, the first of those of equal regret.
pub(super) fn choose<M: Copy>(costs: &[(M, Linear)], (e, h): (f64, f64)) -> Option<M> {
    least_regret(costs.iter().map(|&(method, cost)| (method, (cost.at(e), cost.at(h)))))
}

/// Of `costs`, each a method's costs at the two ends of what an estimate
/// is taken to be, the method whose larger regret of the two is least,
/// the first of those of equal regret: the rule of `choose` for costs
/// that are not linear in what is estimated, which holds of any that
/// only rise with it or only fall (RULES.md D1278).
pub(super) fn least_regret<M: Copy>(costs: impl Iterator<Item = (M, (f64, f64))> + Clone) -> Option<M> {
    let (at_e, at_h) = costs.clone().fold((f64::INFINITY, f64::INFINITY), |(e, h), (_, (x, y))| (e.min(x), h.min(y)));
    let mut chosen: Option<(M, f64)> = None;
    for (method, (x, y)) in costs {
        let worst = regret(x, at_e).max(regret(y, at_h));
        if chosen.is_none_or(|(_, best)| worst < best) {
            chosen = Some((method, worst));
        }
    }
    chosen.map(|(method, _)| method)
}

// Unit tests: the four cases of the rule, the margin of 1 and the bound
// (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;

    /// A hash join reading a right of 1,000 rows, its rows hashed at half a
    /// row, and an index join probing at `probe` a row.
    fn methods(probe: f64) -> [(&'static str, Linear); 2] {
        [("hash", Linear { fixed: 1_500.0_f64, per_row: 0.5_f64 }), ("index", Linear { fixed: 0.0_f64, per_row: probe })]
    }

    /// Verifies: [DESIGN] RULES.md D386: the index join is taken when it is
    /// cheaper at both the estimate and the pessimistic rows, the hash join
    /// when it is cheaper at both; when each is cheaper at one point, the
    /// one whose regret at the other is less, either way. MC/DC of the
    /// condition that keeps a method, `worst < best`: a first method kept
    /// alone, a later one of less regret replacing it, and of equal regret
    /// not.
    #[test]
    fn the_method_of_least_worst_regret_is_taken() {
        // Index cheaper at both: 10 rows and 80 at 10 a probe.
        assert_eq!(choose(&methods(10.0_f64), (10.0_f64, 80.0_f64)), Some("index"));
        // Hash cheaper at both: 1,000 rows and 8,000.
        assert_eq!(choose(&methods(10.0_f64), (1_000.0_f64, 8_000.0_f64)), Some("hash"));
        // Index cheaper at the estimate, 1,000 against 1,550, dearer at the
        // bound, 8,000 against 1,900: its regret 4.2 is greater than the hash
        // join's 1.55, so the hash join.
        assert_eq!(choose(&methods(10.0_f64), (100.0_f64, 800.0_f64)), Some("hash"));
        // Index cheaper at the estimate, 250 against 1,550, a little dearer at
        // the bound, 2,000 against 1,900: its regret 1.05 is less than the
        // hash join's 6.2, so the index join.
        assert_eq!(choose(&methods(2.5_f64), (100.0_f64, 800.0_f64)), Some("index"));
        // Index cheaper at the bound only, its probes cheaper than hashing
        // but begun dearer: its regret at 1 row, 1,600.3 / 1,500.5, is less
        // than the hash join's at 3,000, 3,000 / 2,500, so the index join.
        let cheap_probes =
            [("hash", Linear { fixed: 1_500.0_f64, per_row: 0.5_f64 }), ("index", Linear { fixed: 1_600.0_f64, per_row: 0.3_f64 })];
        assert_eq!(choose(&cheap_probes, (1.0_f64, 3_000.0_f64)), Some("index"));
        // Of equal regret, the first.
        let same = [("a", Linear { fixed: 1.0_f64, per_row: 1.0_f64 }), ("b", Linear { fixed: 1.0_f64, per_row: 1.0_f64 })];
        assert_eq!(choose(&same, (1.0_f64, 2.0_f64)), Some("a"));
        assert_eq!(choose::<&str>(&[], (1.0_f64, 2.0_f64)), None);
    }

    /// Verifies: [DESIGN] RULES.md D386: with a margin of 1 the pessimistic
    /// rows are the estimate, and the method cheaper there is taken; the
    /// bound caps the pessimistic rows but never below the estimate.
    #[test]
    fn a_margin_of_one_trusts_the_estimate() {
        assert!((pessimistic(100.0_f64, f64::INFINITY, 1.0_f64) - 100.0_f64).abs() < f64::EPSILON);
        assert_eq!(choose(&methods(10.0_f64), (100.0_f64, pessimistic(100.0_f64, f64::INFINITY, 1.0_f64))), Some("index"));
        assert!((pessimistic(100.0_f64, 300.0_f64, 8.0_f64) - 300.0_f64).abs() < f64::EPSILON);
        assert!((pessimistic(100.0_f64, 50.0_f64, 8.0_f64) - 100.0_f64).abs() < f64::EPSILON);
        assert!((pessimistic(100.0_f64, f64::INFINITY, 8.0_f64) - 800.0_f64).abs() < f64::EPSILON);
        assert!((regret(0.0_f64, 0.0_f64) - 1.0_f64).abs() < f64::EPSILON);
    }
}
