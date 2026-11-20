//! \[EXTENSION\] Cardinalities given to the planner in place of its
//! estimates (RULES.md D377), to test it, as Leis et al., "How Good Are
//! Query Optimizers, Really?" (2015), §4, inject the true rows of every
//! intermediate result. What is left of a plan's cost is then the cost
//! model's and the search's. A program gives them a session
//! (`Session::with_cardinalities`), no SQL. Each names a set of a query's
//! relations as EXPLAIN names them, by their correlation names, and the
//! rows of their join with their filters. The estimator takes it for any
//! node that joins and filters exactly that set, a table's read and filter
//! among them, and the join enumeration for any join of it. Sets not named
//! keep their estimates.

use std::collections::BTreeMap;

use super::{Card, Estimates};
use crate::logical::LogicalPlan;
use crate::physical::PhysicalPlanner;

/// \[DESIGN\] How deep a chain is looked into for its relations (RULES.md
/// R-CF-6): no deeper than the estimator looks.
const MAX_CHAIN_DEPTH: usize = super::MAX_ESTIMATE_DEPTH;

/// \[EXTENSION\] Rows given in place of the planner's estimates, each of a
/// set of a query's relations named by their correlation names.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Injected {
    sets: BTreeMap<Vec<String>, f64>,
}

impl Injected {
    /// None given.
    pub fn new() -> Injected {
        Injected::default()
    }

    /// Itself, with `rows` given as the rows of the join of `relations`,
    /// named as EXPLAIN names them, with their filters. The rows are at
    /// least one, as no estimate is less (`physical/cardinality.rs`).
    #[must_use]
    pub fn with(mut self, relations: &[&str], rows: f64) -> Injected {
        let mut names: Vec<String> = relations.iter().map(|&r| r.to_owned()).collect();
        names.sort();
        names.dedup();
        self.sets.insert(names, rows.max(1.0_f64));
        self
    }

    /// Whether none is given.
    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }

    /// The rows given of the set `names`, sorted, if any are.
    fn rows_of(&self, names: &[String]) -> Option<f64> {
        self.sets.get(names).copied()
    }

    /// Each set given, its names and rows.
    fn sets(&self) -> impl Iterator<Item = (&[String], f64)> {
        self.sets.iter().map(|(names, &rows)| (names.as_slice(), rows))
    }
}

/// The name a relation is shown by: the qualifier of its first column's
/// name, or that name, as the join enumeration's note shows it.
pub(in crate::physical) fn label(plan: &LogicalPlan) -> String {
    let first = plan.names().into_iter().next().unwrap_or_default();
    first.rsplit_once('.').map_or(first.clone(), |(q, _)| q.to_owned())
}

/// The names of the relations `plan` joins and filters, sorted, if it is a
/// table's read, a filter of a chain, or a product: of a product, both
/// operands'; of any other node, itself one relation.
fn relations(plan: &LogicalPlan, depth: usize, out: &mut Vec<String>) {
    let next = depth.saturating_add(1);
    if next < MAX_CHAIN_DEPTH {
        if let LogicalPlan::Filter { input, .. } = plan {
            return relations(input, next, out);
        }
        if let LogicalPlan::Product { left, right } = plan {
            relations(left, next, out);
            return relations(right, next, out);
        }
    }
    out.push(label(plan));
}

impl Estimates<'_> {
    /// The card given of `plan`, if it joins and filters a set given:
    /// a table's read, a filter or a product.
    pub(super) fn injected(&self, plan: &LogicalPlan) -> Option<Card> {
        let given = self.db().optimizer.injected().filter(|i| !i.is_empty())?;
        if !matches!(plan, LogicalPlan::Scan { .. } | LogicalPlan::Filter { .. } | LogicalPlan::Product { .. }) {
            return None;
        }
        let mut names = Vec::new();
        relations(plan, 0, &mut names);
        names.sort();
        given.rows_of(&names).map(Card::exact)
    }
}

impl PhysicalPlanner<'_> {
    /// Of the relations `leaves`, a chain's, each set given and its rows:
    /// the places of its relations among them. A set naming a relation
    /// the chain has not, or two of one name, is not the chain's.
    pub(in crate::physical) fn injected_sets<'p>(&self, leaves: impl Iterator<Item = &'p LogicalPlan>) -> Vec<(Vec<usize>, f64)> {
        let Some(given) = self.db.optimizer.injected().filter(|i| !i.is_empty()) else { return Vec::new() };
        let labels: Vec<String> = leaves.map(label).collect();
        let place = |name: &String| {
            let mut at = labels.iter().enumerate().filter(|(_, l)| *l == name).map(|(at, _)| at);
            at.next().filter(|_| at.next().is_none())
        };
        given.sets().filter_map(|(names, rows)| Some((names.iter().map(place).collect::<Option<Vec<usize>>>()?, rows))).collect()
    }
}

// Unit tests: how sets are named and their rows kept (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] RULES.md D377: a set is its names in any order,
    /// each once; its rows at least one; one given again takes the later
    /// rows; and none given is empty.
    #[test]
    fn a_set_is_its_names_in_any_order() {
        assert!(Injected::new().is_empty());
        let given = Injected::new().with(&["U", "T", "U"], 40.0_f64).with(&["S"], 0.25_f64).with(&["T", "U"], 50.0_f64);
        assert_eq!(given.rows_of(&["T".to_owned(), "U".to_owned()]), Some(50.0_f64));
        assert_eq!(given.rows_of(&["S".to_owned()]), Some(1.0_f64));
        assert_eq!(given.rows_of(&["T".to_owned()]), None);
        assert_eq!(given.sets().count(), 2);
    }
}
