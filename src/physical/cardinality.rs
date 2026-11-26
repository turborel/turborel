//! \[DESIGN\] One row estimator for every plan node (RULES.md D370): the
//! enumeration, the greedy order, a hash join's build side, the index
//! nested-loop choice and the estimates a plan carries all ask it, so no
//! two of them cost one plan at two cardinalities.
//!
//! It answers every node of a logical plan with a `Card` (`cardinality/nodes.rs`),
//! at least one row whenever the node's input has one, so no estimate of
//! zero makes every operator above it free (Leis et al., "How Good Are
//! Query Optimizers, Really?", 2015, §3.4). It looks through every node to
//! the tables under each column (`cardinality/lineage.rs`), so a filter
//! above a join, a grouping or a derived table is estimated by its base
//! table's statistics; joins by classes of equalities
//! (`cardinality/joins.rs`), the same answer for every order of one set
//! (D302); groupings by Cardenas' formula (`cardinality/groups.rs`).
//!
//! One `Estimates` lives for one planning. It reads the cost model's
//! weights once, and asks each table's statistics and row count once, as
//! asking them takes the settings' and the engines' locks. It keeps each
//! node's estimate by the node's address. The nodes of the plan being
//! planned, its subqueries' among them, live the whole planning, so theirs
//! are kept throughout, and the planner asking at each node it builds asks
//! no subtree twice. Any other node is a temporary plan the planner builds
//! and drops (flipped, reordered, extracted), whose address is reused, so
//! its estimate is kept for one asking only, while the plan asked of is
//! borrowed and no node moves. Within an asking it must be kept, as a
//! node's rule asks its inputs' estimates more than once, a column's
//! distinct values among them, and a plan of nested groupings and outer
//! joins would be estimated a number of times exponential in its depth.

mod bounds;
mod composite;
mod groups;
mod having;
mod injected;
mod joins;
mod keys;
mod lineage;
mod nodes;
mod past;
mod semi;
mod tables;
mod width;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use super::{CostModel, PhysicalPlanner};
use crate::catalog::QualifiedName;
use crate::db::State;
use crate::index::Index;
use crate::logical::{Cond, LogicalPlan};
use crate::stats::Statistics;
use crate::storage::cost_of;
use crate::value::Value;

pub(super) use composite::Member;
pub(in crate::physical) use groups::cardenas;
pub use injected::Injected;
pub(super) use joins::{Classes, find};

// ---------------------------------------------------------------------------
// [DESIGN] Defaults of the rows of what has no count to read (D35: every
// estimate has a default, and no plan waits for a statistics pass).
// ---------------------------------------------------------------------------

/// \[DESIGN\] The rows taken of a relation whose rows nothing tells: a
/// table with no storage, a system table, a work table or a transition
/// table. A thousand, the most rows an index's count reads
/// (`index/blink.rs`), so such a relation is neither taken to be free nor
/// to be larger than a table the planner has counted.
pub(super) const UNKNOWN_ROWS: f64 = 1_000.0;
/// \[DESIGN\] The rows taken of a recursive query's member: its fixpoint is
/// not known before it runs, so as many as `UNKNOWN_ROWS`.
pub(super) const RECURSIVE_ROWS: f64 = UNKNOWN_ROWS;
/// \[DESIGN\] The rows taken of a polymorphic table function's result,
/// which its component procedures alone know.
pub(super) const FUNCTION_ROWS: f64 = UNKNOWN_ROWS;
/// \[DESIGN\] The elements taken of each collection an UNNEST spreads: ten,
/// a short array, as a collection's cardinality is in no statistic.
pub(super) const UNNEST_FANOUT: f64 = 10.0;
/// \[DESIGN\] How deep the estimator looks into a plan (RULES.md R-CF-6):
/// below that, a node is taken to have `UNKNOWN_ROWS`. Deeper than any
/// plan of `MAX_RELATIONS` joins and nesting to the parser's limit.
pub(super) const MAX_ESTIMATE_DEPTH: usize = 1_024;

/// What a plan's rows are estimated to be: the estimate, and an upper
/// bound no larger than its inputs' rows allow (infinite if none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Card {
    /// The estimated rows.
    pub(super) rows: f64,
    /// At most this many.
    pub(super) upper: f64,
}

impl Card {
    /// Of a count known exactly.
    pub(super) fn exact(rows: f64) -> Card {
        Card { rows, upper: rows }
    }

    /// Of a default, with no bound.
    pub(super) fn guess(rows: f64) -> Card {
        Card { rows, upper: f64::INFINITY }
    }

    /// The share `fraction` of it, at least one row if it has one.
    pub(super) fn kept(self, fraction: f64) -> Card {
        Card { rows: at_least_one(self.rows * fraction.clamp(0.0, 1.0), self.rows), upper: self.upper }
    }

    /// It, at most `upper` (RULES.md D385).
    pub(super) fn bounded(self, upper: f64) -> Card {
        let upper = self.upper.min(upper);
        Card { rows: self.rows.min(upper), upper }
    }
}

/// `rows`, at least one if the input it is of, `of`, has one.
pub(super) fn at_least_one(rows: f64, of: f64) -> f64 {
    rows.max(of.min(1.0_f64)).max(0.0_f64)
}

/// \[DESIGN\] The rows `plan` is estimated to give, as planning estimates
/// them: what a subquery's runner weighs a decorrelated build by
/// (`exec/subquery/probed.rs`, RULES.md D790).
pub(crate) fn estimated_rows(db: &State, plan: &LogicalPlan) -> f64 {
    Estimates::over(db, plan, None).card(plan).rows
}

/// One planning's estimator: the state, the weights read once, and each
/// table's statistics and rows, asked once.
pub(super) struct Estimates<'a> {
    db: &'a State,
    cost: CostModel,
    stats: RefCell<HashMap<QualifiedName, Option<Arc<dyn Statistics>>>>,
    rows: RefCell<HashMap<QualifiedName, Option<f64>>>,
    /// Each index's distinct keys, by its name, as its sketch counts them.
    keys: RefCell<HashMap<String, Option<f64>>>,
    /// Each table's unique keys and referential constraints, by its name.
    table_keys: RefCell<HashMap<QualifiedName, Rc<keys::TableKeys>>>,
    /// Each node's estimate, by its address, for the asking under way.
    memo: RefCell<HashMap<usize, Card>>,
    /// The addresses of the nodes of the plan being planned, and the
    /// estimates of those asked, kept the whole planning.
    planned: HashSet<usize>,
    kept: RefCell<HashMap<usize, Card>>,
    /// How many askings are under way, nested.
    asking: Cell<usize>,
    /// A prepared statement's values, of a plan made with its slots: each
    /// slot estimated as its value is (RULES.md D613).
    values: Option<&'a [Value]>,
}

impl<'a> Estimates<'a> {
    /// An estimator over `db`, its weights read now.
    pub(super) fn new(db: &'a State) -> Estimates<'a> {
        let (stats, rows, keys) = (RefCell::new(HashMap::new()), RefCell::new(HashMap::new()), RefCell::new(HashMap::new()));
        let (memo, asking) = (RefCell::new(HashMap::new()), Cell::new(0));
        let (planned, kept, table_keys) = (HashSet::new(), RefCell::new(HashMap::new()), RefCell::new(HashMap::new()));
        Estimates { db, cost: db.cost(), stats, rows, keys, table_keys, memo, asking, planned, kept, values: None }
    }

    /// An estimator over `db` for the planning of `plan`, whose nodes live
    /// as long as it, its slots estimated by `values`, if given.
    pub(super) fn over(db: &'a State, plan: &LogicalPlan, values: Option<&'a [Value]>) -> Estimates<'a> {
        let mut planned = HashSet::new();
        addresses(plan, &mut planned);
        Estimates { planned, values, ..Estimates::new(db) }
    }

    /// `cond`, each slot its value, if the estimator is given values and
    /// it fills; else itself.
    pub(super) fn valued<'c>(&self, cond: &'c Cond) -> std::borrow::Cow<'c, Cond> {
        match self.values.and_then(|values| super::estimate::estimated(cond, values)) {
            Some(filled) => std::borrow::Cow::Owned(filled),
            None => std::borrow::Cow::Borrowed(cond),
        }
    }

    /// The state estimated of.
    pub(super) fn db(&self) -> &'a State {
        self.db
    }

    /// The cost model's weights.
    pub(super) fn cost(&self) -> &CostModel {
        &self.cost
    }

    /// `table`'s statistics, asked of its engine once. The cache's borrow
    /// is released before the engine is asked (RULES.md R-ERR-4).
    pub(super) fn statistics(&self, table: &QualifiedName) -> Option<Arc<dyn Statistics>> {
        let known = self.stats.borrow().get(table).cloned();
        if let Some(stats) = known {
            return stats;
        }
        let asked = self.db.statistics(table);
        self.stats.borrow_mut().insert(table.clone(), asked.clone());
        asked
    }

    /// `table`'s rows, counted by its engine once.
    pub(super) fn table_rows(&self, table: &QualifiedName) -> Option<f64> {
        let known = self.rows.borrow().get(table).copied();
        if let Some(rows) = known {
            return rows;
        }
        let counted = self.db.row_count(table).map(cost_of);
        self.rows.borrow_mut().insert(table.clone(), counted);
        counted
    }

    /// The distinct keys the index `name`, `index`, counts, asked of it
    /// once: a sketch's count is an estimate made anew at each asking.
    pub(super) fn index_keys(&self, name: &str, index: &dyn Index) -> Option<f64> {
        let known = self.keys.borrow().get(name).copied();
        if let Some(keys) = known {
            return keys;
        }
        let counted = index.distinct_keys();
        self.keys.borrow_mut().insert(name.to_owned(), counted);
        counted
    }

    /// The card of `plan`.
    pub(super) fn card(&self, plan: &LogicalPlan) -> Card {
        self.asked(|| self.card_at(plan, 0))
    }

    /// `f`, an asking of the estimator: the outermost forgets the
    /// estimates the last one kept, whose nodes may be gone.
    pub(super) fn asked<R>(&self, f: impl FnOnce() -> R) -> R {
        let outer = self.asking.get();
        if outer == 0 {
            // Dropped, not cleared: clearing a table that once held a large
            // plan's nodes writes its whole capacity at every asking.
            drop(self.memo.take());
        }
        self.asking.set(outer.saturating_add(1));
        let answer = f();
        self.asking.set(outer);
        answer
    }

    /// The estimate of the node at `at` kept, if any.
    fn kept_card(&self, at: usize) -> Option<Card> {
        let kept = if self.planned.contains(&at) { &self.kept } else { &self.memo };
        kept.borrow().get(&at).copied()
    }

    /// Keeps `card`, the estimate of the node at `at`.
    fn keep_card(&self, at: usize, card: Card) {
        let kept = if self.planned.contains(&at) { &self.kept } else { &self.memo };
        kept.borrow_mut().insert(at, card);
    }

    /// The estimated rows of `plan`.
    pub(super) fn rows(&self, plan: &LogicalPlan) -> f64 {
        self.card(plan).rows
    }
}

impl PhysicalPlanner<'_> {
    /// `f` of this planning's estimator, or of one made for the call if
    /// the planner was not made by `plan` (a unit test's).
    pub(super) fn estimating<R>(&self, f: impl FnOnce(&Estimates<'_>) -> R) -> R {
        match self.est {
            Some(est) => f(est),
            None => f(&Estimates::new(self.db)),
        }
    }

    /// The card of `plan`.
    pub(super) fn card(&self, plan: &LogicalPlan) -> Card {
        self.estimating(|est| est.card(plan))
    }

    /// The cost model's weights, as read when planning began.
    pub(super) fn weights(&self) -> CostModel {
        self.est.map_or_else(|| self.db.cost(), |est| est.cost().clone())
    }

    /// `table`'s statistics, as asked once this planning.
    pub(super) fn statistics(&self, table: &QualifiedName) -> Option<Arc<dyn Statistics>> {
        self.est.map_or_else(|| self.db.statistics(table), |est| est.statistics(table))
    }

    /// The distinct keys the index `name`, `index`, counts, as asked once
    /// this planning.
    pub(super) fn index_keys(&self, name: &str, index: &dyn Index) -> Option<f64> {
        self.est.map_or_else(|| index.distinct_keys(), |est| est.index_keys(name, index))
    }

    /// `table`'s rows, as counted once this planning.
    pub(super) fn table_rows(&self, table: &QualifiedName) -> Option<f64> {
        self.est.map_or_else(|| self.db.row_count(table).map(cost_of), |est| est.table_rows(table))
    }
}

/// Adds the addresses of `plan`'s nodes, and its subqueries' plans', to
/// `found`. It recurs only into the plan's depth, as planning does.
fn addresses(plan: &LogicalPlan, found: &mut HashSet<usize>) {
    found.insert(std::ptr::from_ref(plan).addr());
    for q in plan.subqueries() {
        addresses(&q.plan, found);
    }
    for input in plan.inputs() {
        addresses(input, found);
    }
}

// Unit tests: each node's rule, lineage, Cardenas' formula and classes on
// synthetic catalogs (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod keyed_tests;
#[cfg(test)]
mod tests;
