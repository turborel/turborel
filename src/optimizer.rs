//! \[DESIGN\] Rule-based logical optimization.
//!
//! A `Rule` looks at one node of the logical plan, whose inputs have already
//! been rewritten, and either returns an equivalent plan or `None`. The
//! driver applies the enabled rules bottom-up, in registration order, until
//! no rule fires (or the setting `OPTIMIZER.MAX_PASSES` is reached). After
//! a rule fires at a node, the rules are tried at the node again before the
//! walk goes up, within a budget of rewrites (RULES.md D371).
//!
//! Every rule here must preserve the result *and* the run-time errors of the
//! query ([X3.135-1986 §3.3] the implementation "shall achieve the same effect"). That is
//! why constant folding leaves "1/0" alone, and why "FALSE AND x" is not
//! reduced to FALSE: x might raise an error that the query is required to raise.
//! A rule that changes which rows or groups a condition is evaluated on may do so
//! only when the condition cannot raise an error (`Cond::can_raise`; RULES.md D15).
//!
//! Rules are storage-agnostic. Anything that depends on engines, indexes or
//! costs belongs in physical.rs.

mod disjunctions;
mod distinct_aggregates;
mod elimination;
mod folding;
mod hoisting;
mod inlining;
mod limits;
mod outer;
pub(crate) mod plugged;
pub(crate) mod prepared;
mod raising;
mod setops;
mod transitivity;
pub(crate) mod unnesting;
mod views;
mod windows;

use std::collections::HashSet;
use std::sync::Arc;

use crate::db::State;
use crate::logical::{Cond, Expr, LogicalPlan, Truth, and_all, cond_columns, map_cond_columns, split_and};
use crate::settings::{Key, OPTIMIZER_MAX_PASSES};
pub use inlining::{INLINE_FUNCTIONS, InlineFunctions};
pub use unnesting::{UNNEST_SUBQUERIES, UnnestSubqueries};

/// The names of the built-in rules, as EXPLAIN shows them and their
/// settings name them (`switch_name`).
pub const CONSTANT_FOLDING: &str = "constant_folding";
/// See [`CONSTANT_FOLDING`].
pub const SIMPLIFY_CONDITIONS: &str = "simplify_conditions";
/// See [`CONSTANT_FOLDING`].
pub const MERGE_FILTERS: &str = "merge_filters";
/// See [`CONSTANT_FOLDING`].
pub const PUSH_FILTER_INTO_PRODUCT: &str = "push_filter_into_product";
/// See [`CONSTANT_FOLDING`].
pub const PUSH_HAVING_INTO_WHERE: &str = "push_having_into_where";
/// See [`CONSTANT_FOLDING`].
pub const REMOVE_REDUNDANT_DISTINCT: &str = "remove_redundant_distinct";

pub use disjunctions::{FACTOR_DISJUNCTIONS, FactorDisjunctions};
pub use distinct_aggregates::{DISTINCT_AGGREGATES, DistinctAggregates};
pub use elimination::{ELIMINATE_JOINS, EliminateJoins};
pub use folding::ConstantFolding;
pub use hoisting::{HOIST_RAISING_EXPRESSIONS, HoistRaisingExpressions};
pub use limits::{PUSH_LIMIT_THROUGH_UNION, PushLimitThroughUnion};
pub use outer::{PUSH_FILTER_INTO_OUTER_JOIN, PushFilterIntoOuterJoin};
pub(crate) use raising::is_guarded;
pub use raising::{PUSH_RAISING_FILTER_INTO_PRODUCT, PushRaisingFilterIntoProduct};
pub use setops::{PUSH_FILTER_THROUGH_SET_OPERATION, PushFilterThroughSetOperation};
pub use transitivity::{PREDICATE_TRANSITIVITY, PredicateTransitivity};
pub(crate) use views::substitute;
pub use views::{PUSH_FILTER_THROUGH_DISTINCT, PUSH_FILTER_THROUGH_PROJECT, PushFilterThroughDistinct, PushFilterThroughProject};
pub use windows::{PUSH_FILTER_THROUGH_WINDOW, PushFilterThroughWindow};

/// The passes `Optimizer::optimize` makes at most, unless the setting
/// `OPTIMIZER.MAX_PASSES` says otherwise.
pub const DEFAULT_MAX_PASSES: usize = 64;

/// What a rule may consult besides the plan: the catalog, registries, etc.
pub struct RuleContext<'a> {
    /// The database state, read-only.
    pub db: &'a State,
}

/// \[DESIGN\] A rewrite of the logical plan.
///
/// Contract (RULES.md R-SAFE-3):
/// * `apply` returns `Some` only with an equivalent plan: the same rows (in the
///   same order under a Sort), the same columns, and the same run-time errors.
///   A condition that can raise an error (`Cond::can_raise`) is evaluated on
///   the same rows or groups as before (RULES.md D15).
/// * `apply` returns `None` when it does not apply, so rewriting reaches a
///   fixed point; the driver stops after `OPTIMIZER.MAX_PASSES` otherwise.
/// * The plan keeps the shapes the executor runs: every leaf a `Scan` of a
///   catalog table, a `System` table's rows, or `Values`.
/// * `name` is unique and constant: the setting `OPTIMIZER.NAME` turns the
///   rule on or off (`switch_name`).
/// * The `RuleContext` is read-only: a rule must not change the database.
/// * Methods may be called from several threads at once (`Send + Sync`): an
///   implementation guards its own state.
/// * No method may call back into the `Database` or its sessions (RULES.md
///   R-ERR-4); the `Database` docs say what happens if one does.
pub trait Rule: Send + Sync {
    /// The rule's unique name.
    fn name(&self) -> &'static str;
    /// An equivalent plan for `plan`, whose inputs are already rewritten, or
    /// `None` if the rule does not apply.
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan>;
}

/// \[DESIGN\] The group of the settings turning each rule on or off
/// (RULES.md D36): `OPTIMIZER.CONSTANT_FOLDING`, ON or OFF.
pub const SWITCH_GROUP: &str = "OPTIMIZER";

/// The name of the setting that turns the rule `rule` on or off: its name
/// upper case, in `SWITCH_GROUP`.
pub fn switch_name(rule: &str) -> String {
    format!("{SWITCH_GROUP}.{}", rule.to_ascii_uppercase())
}

/// A rule, and the setting that turns it on or off, once declared.
#[derive(Clone)]
struct Switched {
    rule: Arc<dyn Rule>,
    switch: Option<Key>,
}

/// The rules, in the order they are tried, each turned on or off by a
/// setting of its own (`switch_name`), so a session may turn one off for
/// itself and the database's choice is kept with its definitions
/// (RULES.md D36).
#[derive(Clone)]
pub struct Optimizer {
    rules: Vec<Switched>,
}

impl Optimizer {
    /// An optimizer with no rules: plans are left as bound.
    pub fn empty() -> Optimizer {
        Optimizer { rules: Vec::new() }
    }

    /// An optimizer with the built-in rules.
    pub fn with_builtin_rules() -> Optimizer {
        let mut o = Optimizer::empty();
        // Before the others, which then move the conditions they make.
        o.add(Box::new(InlineFunctions));
        o.add(Box::new(UnnestSubqueries));
        o.add(Box::new(ConstantFolding));
        o.add(Box::new(SimplifyConditions));
        o.add(Box::new(MergeFilters));
        o.add(Box::new(PushFilterThroughProject));
        o.add(Box::new(PushFilterThroughDistinct));
        // Before the conjuncts are pushed apart.
        o.add(Box::new(FactorDisjunctions));
        o.add(Box::new(PredicateTransitivity));
        // Before the raising conjuncts move, so those it leaves of one
        // operand alone move too (RULES.md D373).
        o.add(Box::new(HoistRaisingExpressions));
        o.add(Box::new(PushRaisingFilterIntoProduct));
        o.add(Box::new(PushFilterIntoProduct));
        o.add(Box::new(PushFilterIntoOuterJoin));
        o.add(Box::new(PushFilterThroughSetOperation));
        o.add(Box::new(PushFilterThroughWindow));
        o.add(Box::new(PushLimitThroughUnion));
        o.add(Box::new(EliminateJoins));
        o.add(Box::new(PushHavingIntoWhere));
        o.add(Box::new(RemoveRedundantDistinct));
        o.add(Box::new(DistinctAggregates));
        o
    }

    /// Rules run in the order they were added. Its setting is declared as
    /// the change that adds it is published (`Database::with_optimizer`).
    pub fn add(&mut self, rule: Box<dyn Rule>) {
        self.rules.push(Switched { rule: Arc::from(rule), switch: None });
    }

    /// Each rule's name, in the order they run.
    pub fn rules(&self) -> Vec<&'static str> {
        self.rules.iter().map(|r| r.rule.name()).collect()
    }

    /// Reused physical plans retain scalar preparation only when its
    /// controlling rule is still installed and enabled for this session.
    pub(crate) fn enabled(&self, name: &str, settings: &crate::settings::Settings) -> bool {
        self.rules.iter().find(|r| r.rule.name() == name).is_some_and(|r| r.switch.is_none_or(|key| settings.is_on(key)))
    }

    /// The names of the rules whose settings are not yet declared.
    pub(crate) fn unswitched(&self) -> Vec<&'static str> {
        self.rules.iter().filter(|r| r.switch.is_none()).map(|r| r.rule.name()).collect()
    }

    /// Makes `key` the setting that turns the rule `name` on or off.
    pub(crate) fn switched(&mut self, name: &str, key: Key) {
        for r in self.rules.iter_mut().filter(|r| r.rule.name() == name) {
            r.switch = Some(key);
        }
    }

    /// Rewrite to a fixpoint, by the rules `ctx`'s settings leave on.
    /// Returns the plan and the names of the rules that fired, in order.
    pub fn optimize(&self, plan: LogicalPlan, ctx: &RuleContext<'_>) -> (LogicalPlan, Vec<&'static str>) {
        self.optimize_except(plan, ctx, &[])
    }

    /// As `optimize`, the rules named in `except` left off too.
    pub fn optimize_except(&self, mut plan: LogicalPlan, ctx: &RuleContext<'_>, except: &[&str]) -> (LogicalPlan, Vec<&'static str>) {
        let mut trace = Vec::new();
        let settings = &ctx.db.settings;
        let on: Vec<&dyn Rule> = self
            .rules
            .iter()
            .filter(|r| r.switch.is_none_or(|k| settings.is_on(k)) && !except.contains(&r.rule.name()))
            .map(|r| &*r.rule)
            .collect();
        // A safety net against rules that keep undoing each other.
        let passes = usize::try_from(settings.integer(OPTIMIZER_MAX_PASSES)).unwrap_or(DEFAULT_MAX_PASSES);
        // As many rewrites as the passes of the driver before D371 made at
        // most, one at each node in each pass.
        let mut budget = nodes(&plan).saturating_mul(passes);
        for _ in 0..passes {
            let (p, changed) = pass(&on, plan, (ctx, &mut budget), &mut trace);
            plan = p;
            if !changed {
                break;
            }
        }
        (plan, trace)
    }
}

/// The nodes of `plan` and of its subqueries' plans, which unnesting
/// makes nodes of it.
fn nodes(plan: &LogicalPlan) -> usize {
    let own = plan.subqueries().iter().fold(1_usize, |n, q| n.saturating_add(nodes(&q.plan)));
    plan.inputs().fold(own, |n, input| n.saturating_add(nodes(input)))
}

/// One pass of the rules `on` over `plan`, its inputs first: the plan, and
/// whether a rule fired. A node a rule rewrote has the rules tried at it
/// again, where a restarted pass over the whole plan would find them only
/// later (RULES.md D371); the inputs a rewrite made wait for the next
/// pass, as walking them again at once costs a walk of the subtree for
/// each rewrite. Every rewrite spends one of `budget`, the optimization's,
/// so rules that undo each other stop once it is spent. The walk recurs
/// only into the plan's depth; each node's rounds are a loop.
fn pass(
    on: &[&dyn Rule],
    plan: LogicalPlan,
    (ctx, budget): (&RuleContext<'_>, &mut usize),
    trace: &mut Vec<&'static str>,
) -> (LogicalPlan, bool) {
    let mut changed = false;
    let mut plan = plan.map_inputs(|input| {
        let (p, c) = pass(on, input, (ctx, &mut *budget), trace);
        changed |= c;
        p
    });
    while *budget > 0 {
        let Some((name, p)) = on.iter().find_map(|rule| rule.apply(&plan, ctx).map(|p| (rule.name(), p))) else { break };
        trace.push(name);
        changed = true;
        *budget = budget.saturating_sub(1);
        plan = p;
    }
    (plan, changed)
}

// ===========================================================================
// Built-in rules
// ===========================================================================

/// Remove the known-TRUE operands of AND and known-FALSE operands of OR,
/// and a filter that is TRUE.
/// Only identities that drop no evaluation of other parts are used:
/// "TRUE AND x" = x, "FALSE OR x" = x, NOT on a constant.
pub struct SimplifyConditions;

/// The simplified form of `c`, or `None` if nothing in it simplifies.
fn simplify(c: &Cond) -> Option<Cond> {
    use Truth::{False, True, Unknown};
    let or_same = |s: &Option<Cond>, orig: &Cond| s.clone().unwrap_or_else(|| orig.clone());
    match c {
        Cond::And(a, b) | Cond::Or(a, b) => {
            let is_and = matches!(c, Cond::And(..));
            // TRUE is the identity of AND, FALSE of OR.
            let identity = if is_and { True } else { False };
            let (sa, sb) = (simplify(a), simplify(b));
            let (left, right) = (or_same(&sa, a), or_same(&sb, b));
            let is_identity = |side: &Cond| if let Cond::Const(t) = side { *t == identity } else { false };
            if is_identity(&left) {
                Some(right)
            } else if is_identity(&right) {
                Some(left)
            } else if sa.is_some() || sb.is_some() {
                Some(if is_and { Cond::And(Box::new(left), Box::new(right)) } else { Cond::Or(Box::new(left), Box::new(right)) })
            } else {
                None
            }
        }
        Cond::Not(a) => {
            let sa = simplify(a);
            // [X3.135-1986 §5.18 GR2] NOT(true) is false, NOT(false) is true, NOT(unknown) is unknown.
            if let Cond::Const(t) = or_same(&sa, a) {
                return Some(Cond::Const(match t {
                    True => False,
                    False => True,
                    Unknown => Unknown,
                }));
            }
            sa.map(|inner| Cond::Not(Box::new(inner)))
        }
        Cond::Const(_)
        | Cond::Is(..)
        | Cond::Compare(..)
        | Cond::Distinct(..)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(..)
        | Cond::Quantified(..)
        | Cond::Match { .. }
        | Cond::In(..)
        | Cond::Value(_) => None,
    }
}

impl Rule for SimplifyConditions {
    fn name(&self) -> &'static str {
        SIMPLIFY_CONDITIONS
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond, input } = plan else { return None };
        if matches!(cond, Cond::Const(Truth::True)) {
            return Some((**input).clone());
        }
        simplify(cond).map(|cond| LogicalPlan::Filter { cond, input: input.clone() })
    }
}

/// Filter(a, Filter(b, x)) = Filter(b AND a, x), when `a` cannot raise an
/// error: the merged filter evaluates `a` on rows `b` removed.
pub struct MergeFilters;

impl Rule for MergeFilters {
    fn name(&self) -> &'static str {
        MERGE_FILTERS
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond: outer, input } = plan else { return None };
        let LogicalPlan::Filter { cond: inner, input: x } = &**input else { return None };
        // [X3.135-1986 §5.18 GR2] defines AND only by its truth table, and nothing
        // permits leaving an operand unevaluated, so the engine evaluates both. The
        // merged filter therefore evaluates `outer` on rows that `inner` removed. That is only the same effect if `outer`
        // cannot raise an error there (RULES.md D15).
        if outer.can_raise() {
            return None;
        }
        Some(LogicalPlan::Filter { cond: Cond::And(Box::new(inner.clone()), Box::new(outer.clone())), input: x.clone() })
    }
}

/// Move each conjunct of a filter over a product to the side whose columns it
/// uses: the product of filtered inputs has the same rows ([X3.135-1986 §5.20 GR2b, §5.21 GR2]).
pub struct PushFilterIntoProduct;

fn wrap(input: LogicalPlan, conds: Vec<Cond>) -> LogicalPlan {
    match and_all(conds) {
        Some(cond) => LogicalPlan::Filter { cond, input: Box::new(input) },
        None => input,
    }
}

impl Rule for PushFilterIntoProduct {
    fn name(&self) -> &'static str {
        PUSH_FILTER_INTO_PRODUCT
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond, input } = plan else { return None };
        let LogicalPlan::Product { left, right } = &**input else { return None };
        // RULES.md D15: after the push each conjunct sees different rows (one
        // side's rows even when the other side is empty; fewer product rows for
        // the conjuncts kept above), so a condition that can raise stays whole.
        if cond.can_raise() {
            return None;
        }
        let lw = left.width();
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let (mut l, mut r, mut keep) = (Vec::new(), Vec::new(), Vec::new());
        for c in conjuncts {
            let mut cols = Vec::new();
            cond_columns(&c, &mut cols);
            if cols.is_empty() {
                keep.push(c);
            } else if cols.iter().all(|&i| i < lw) {
                l.push(c);
            } else if cols.iter().all(|&i| i >= lw) {
                r.push(map_cond_columns(c, &|i| i.saturating_sub(lw)));
            } else {
                keep.push(c);
            }
        }
        if l.is_empty() && r.is_empty() {
            return None;
        }
        let product = LogicalPlan::Product { left: Box::new(wrap((**left).clone(), l)), right: Box::new(wrap((**right).clone(), r)) };
        Some(wrap(product, keep))
    }
}

/// A HAVING conjunct that uses only grouping columns keeps exactly the groups
/// whose rows it would keep before grouping, so it can be applied first, as if
/// it were in WHERE, and fewer rows get grouped. It must use at least one
/// grouping column: without GROUP BY there is always one group [X3.135-1986 §5.23 GR1], so
/// a constant HAVING condition must stay where it is. Nor is one pushed below
/// a grouping that makes groups before any row, of each empty grouping set
/// [ISO/IEC 9075-2:2023 §7.13 GR2a]: such a group is there whatever rows
/// are let through.
pub struct PushHavingIntoWhere;

impl Rule for PushHavingIntoWhere {
    fn name(&self) -> &'static str {
        PUSH_HAVING_INTO_WHERE
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond, input } = plan else { return None };
        let LogicalPlan::Aggregate { groups, collations, aggs, seeds, input: rows } = &**input else { return None };
        // A row of a group is not the group's value under a collation, and
        // a group made before any row is no group of the rows let through.
        if !collations.is_empty() || !seeds.is_empty() {
            return None;
        }
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let (mut down, mut keep) = (Vec::new(), Vec::new());
        for c in conjuncts {
            let mut cols = Vec::new();
            cond_columns(&c, &mut cols);
            if !cols.is_empty() && cols.iter().all(|&i| i < groups.len()) {
                // Every column is below `groups.len()`, so each maps.
                down.push(map_cond_columns(c, &|i| groups.get(i).copied().unwrap_or(i)));
            } else {
                keep.push(c);
            }
        }
        // RULES.md D15: the conjuncts kept above would then see only the groups
        // the pushed ones let through, so none of them may raise an error.
        if down.is_empty() || keep.iter().any(Cond::can_raise) {
            return None;
        }
        let agg = LogicalPlan::Aggregate {
            groups: groups.clone(),
            collations: Vec::new(),
            aggs: aggs.clone(),
            seeds: seeds.clone(),
            input: Box::new(wrap((**rows).clone(), down)),
        };
        Some(wrap(agg, keep))
    }
}

/// DISTINCT over a single table is redundant when the select list includes
/// every column of a UNIQUE constraint, each NOT NULL: no two rows share
/// their values [X3.135-1986 §6.4 GR2], so no two output rows can be
/// duplicates. (SQL-86 made a UNIQUE column NOT NULL [§6.4 SR3]; SQL-92 does
/// not [ISO/IEC 9075:1992 §11.7], and two rows null there may be.) This
/// rule consults the catalog through the context.
pub struct RemoveRedundantDistinct;

fn base_table(plan: &LogicalPlan) -> Option<&LogicalPlan> {
    match plan {
        LogicalPlan::Scan { .. } => Some(plan),
        LogicalPlan::Filter { input, .. } => base_table(input),
        LogicalPlan::Product { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Project { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Sort { .. }
        | LogicalPlan::Limit { .. }
        | LogicalPlan::Recursive { .. }
        | LogicalPlan::Unnest(..)
        | LogicalPlan::Window(..)
        | LogicalPlan::Recognize(..)
        | LogicalPlan::Polymorphic(..)
        | LogicalPlan::WorkTable { .. }
        | LogicalPlan::Transition { .. } => None,
    }
}

impl Rule for RemoveRedundantDistinct {
    fn name(&self) -> &'static str {
        REMOVE_REDUNDANT_DISTINCT
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Distinct { input } = plan else { return None };
        let LogicalPlan::Project { exprs, input: rows } = &**input else { return None };
        let Some(LogicalPlan::Scan { table, .. }) = base_table(rows) else { return None };
        let desc = ctx.db.catalog.table(table)?;
        let projected: HashSet<usize> = exprs
            .iter()
            .filter_map(|e| match e {
                Expr::Column(i) => Some(*i),
                Expr::Const(_)
                | Expr::Neg(_)
                | Expr::Arith(..)
                | Expr::Call(..)
                | Expr::Outer(..)
                | Expr::Dynamic(_)
                | Expr::Case(..)
                | Expr::Cast(..)
                | Expr::SortKey(..)
                | Expr::Domain(..)
                | Expr::Warned(..)
                | Expr::Subquery(..)
                | Expr::Truth(_)
                | Expr::NextValue(_)
                | Expr::Invoked(_) => None,
            })
            .collect();
        // A UNIQUE column may be null [ISO/IEC 9075:1992 §8.9], and rows
        // with nulls there may be duplicates: its columns must be NOT NULL.
        let keyed = |c: &usize| projected.contains(c) && desc.columns.get(*c).is_some_and(|col| col.not_null && col.collation.is_none());
        desc.unique().any(|u| u.iter().all(keyed)).then(|| (**input).clone())
    }
}

// Unit tests: the driver's local fixpoint against a pass restarted after
// every rewrite (RULES.md R-TEST-1).
#[cfg(test)]
mod drivers;
