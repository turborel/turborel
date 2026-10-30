//! \[DESIGN\] Rule-based logical optimization.
//!
//! A `Rule` looks at one node of the logical plan, whose inputs have already
//! been rewritten, and either returns an equivalent plan or `None`. The
//! driver applies the enabled rules bottom-up, in registration order, until
//! no rule fires (or the setting `OPTIMIZER.MAX_PASSES` is reached).
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

mod outer;
mod transitivity;
mod views;

use std::collections::HashSet;
use std::sync::Arc;

use crate::db::State;
use crate::exec::{NoSubqueries, eval, eval_cond};
use crate::logical::{Cond, Expr, LogicalPlan, Truth, and_all, cond_columns, map_cond_columns, split_and};
use crate::settings::OPTIMIZER_MAX_PASSES;

/// The names of the built-in rules, as EXPLAIN shows them and `set_enabled`
/// takes them.
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

pub use outer::{PUSH_FILTER_INTO_OUTER_JOIN, PushFilterIntoOuterJoin};
pub use transitivity::{PREDICATE_TRANSITIVITY, PredicateTransitivity};
pub use views::{PUSH_FILTER_THROUGH_DISTINCT, PUSH_FILTER_THROUGH_PROJECT, PushFilterThroughDistinct, PushFilterThroughProject};

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
/// * `name` is unique and constant; `set_enabled` finds the rule by it.
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

/// The rules, in the order they are tried, and which are disabled.
#[derive(Clone)]
pub struct Optimizer {
    rules: Vec<Arc<dyn Rule>>,
    disabled: HashSet<String>,
}

impl Optimizer {
    /// An optimizer with no rules: plans are left as bound.
    pub fn empty() -> Optimizer {
        Optimizer { rules: Vec::new(), disabled: HashSet::new() }
    }

    /// An optimizer with the built-in rules, all enabled.
    pub fn with_builtin_rules() -> Optimizer {
        let mut o = Optimizer::empty();
        o.add(Box::new(ConstantFolding));
        o.add(Box::new(SimplifyConditions));
        o.add(Box::new(MergeFilters));
        o.add(Box::new(PushFilterThroughProject));
        o.add(Box::new(PushFilterThroughDistinct));
        // Before the conjuncts are pushed apart.
        o.add(Box::new(PredicateTransitivity));
        o.add(Box::new(PushFilterIntoProduct));
        o.add(Box::new(PushFilterIntoOuterJoin));
        o.add(Box::new(PushHavingIntoWhere));
        o.add(Box::new(RemoveRedundantDistinct));
        o
    }

    /// Rules run in the order they were added.
    pub fn add(&mut self, rule: Box<dyn Rule>) {
        self.rules.push(Arc::from(rule));
    }

    /// Enable or disable a rule by name. Returns false if there is no such rule.
    pub fn set_enabled(&mut self, name: &str, on: bool) -> bool {
        if !self.rules.iter().any(|r| r.name() == name) {
            return false;
        }
        if on {
            self.disabled.remove(name);
        } else {
            self.disabled.insert(name.to_string());
        }
        true
    }

    /// Each rule's name and whether it is enabled, in the order they run.
    pub fn rules(&self) -> Vec<(&'static str, bool)> {
        self.rules.iter().map(|r| (r.name(), !self.disabled.contains(r.name()))).collect()
    }

    /// Rewrite to a fixpoint. Returns the plan and the names of the rules
    /// that fired, in order.
    pub fn optimize(&self, mut plan: LogicalPlan, ctx: &RuleContext<'_>) -> (LogicalPlan, Vec<&'static str>) {
        let mut trace = Vec::new();
        // A safety net against rules that keep undoing each other.
        let passes = usize::try_from(ctx.db.settings.integer(OPTIMIZER_MAX_PASSES)).unwrap_or(DEFAULT_MAX_PASSES);
        for _ in 0..passes {
            let (p, changed) = self.pass(plan, ctx, &mut trace);
            plan = p;
            if !changed {
                break;
            }
        }
        (plan, trace)
    }

    fn pass(&self, plan: LogicalPlan, ctx: &RuleContext<'_>, trace: &mut Vec<&'static str>) -> (LogicalPlan, bool) {
        let mut changed = false;
        let plan = plan.map_inputs(|input| {
            let (p, c) = self.pass(input, ctx, trace);
            changed |= c;
            p
        });
        for rule in self.rules.iter().filter(|r| !self.disabled.contains(r.name())) {
            if let Some(p) = rule.apply(&plan, ctx) {
                trace.push(rule.name());
                return (p, true);
            }
        }
        (plan, changed)
    }
}

// ===========================================================================
// Built-in rules
// ===========================================================================

/// Evaluate column-free subexpressions once, at planning time. A subexpression
/// whose evaluation fails ("1/0") is left for run time, so the error is still
/// raised exactly when the standard requires. Calls are folded only for
/// functions that declare themselves deterministic.
pub struct ConstantFolding;

/// The folded form of `e`, or `None` if nothing in it folds: its parts
/// folded, and it computed if they are all constants.
fn fold_expr(e: &Expr) -> Option<Expr> {
    let mut changed = false;
    let folded = e.clone().map_children(&mut |child| match fold_expr(&child) {
        Some(done) => {
            changed = true;
            done
        }
        None => child,
    });
    if computable(&folded) {
        // A failing subexpression is left for run time, which raises the error
        // when the standard requires it [X3.135-1986 §3.3].
        if let Ok(v) = eval(&folded, &Vec::new(), &NoSubqueries) {
            return Some(Expr::Const(v));
        }
    }
    changed.then_some(folded)
}

/// Whether `e`, not a constant, can be computed now: an operation on
/// constants alone, calling only a deterministic function.
fn computable(e: &Expr) -> bool {
    let mut constant = !matches!(e, Expr::Column(_) | Expr::Const(_) | Expr::Outer(..) | Expr::Subquery(..));
    if let Expr::Call(func, _) = e {
        constant = constant && func.0.deterministic();
    }
    e.for_each_child(&mut |child| constant = constant && matches!(child, Expr::Const(_)));
    constant
}

/// The folded form of `c`, or `None` if nothing in it folds.
fn fold_cond(c: &Cond) -> Option<Cond> {
    match c {
        Cond::Const(_)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Quantified(..)
        | Cond::Is(..)
        | Cond::Unique(_)
        | Cond::Match { .. } => None,
        Cond::Or(a, b) | Cond::And(a, b) => match (fold_cond(a), fold_cond(b)) {
            (None, None) => None,
            (left, right) => {
                let left = Box::new(left.unwrap_or_else(|| (**a).clone()));
                let right = Box::new(right.unwrap_or_else(|| (**b).clone()));
                Some(if matches!(c, Cond::Or(..)) { Cond::Or(left, right) } else { Cond::And(left, right) })
            }
        },
        Cond::Not(a) => fold_cond(a).map(|a| Cond::Not(Box::new(a))),
        Cond::Compare(a, op, b) => {
            let (fa, fb) = (fold_expr(a), fold_expr(b));
            let (a2, b2) = (fa.clone().unwrap_or_else(|| a.clone()), fb.clone().unwrap_or_else(|| b.clone()));
            if let (Expr::Const(_), Expr::Const(_)) = (&a2, &b2) {
                let changed = fa.is_some() || fb.is_some();
                let cmp = Cond::Compare(a2, *op, b2);
                return match eval_cond(&cmp, &Vec::new(), &NoSubqueries) {
                    Ok(t) => Some(Cond::Const(t)),
                    // Left for run time, as in `fold_expr`.
                    Err(_) => changed.then_some(cmp),
                };
            }
            if fa.is_none() && fb.is_none() { None } else { Some(Cond::Compare(a2, *op, b2)) }
        }
    }
}

impl Rule for ConstantFolding {
    fn name(&self) -> &'static str {
        CONSTANT_FOLDING
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        match plan {
            LogicalPlan::Filter { cond, input } => fold_cond(cond).map(|cond| LogicalPlan::Filter { cond, input: input.clone() }),
            LogicalPlan::OuterJoin { kind, cond, left, right } => {
                fold_cond(cond).map(|cond| LogicalPlan::OuterJoin { kind: *kind, cond, left: left.clone(), right: right.clone() })
            }
            LogicalPlan::Project { exprs, input } => {
                let folded: Vec<Option<Expr>> = exprs.iter().map(fold_expr).collect();
                if folded.iter().all(Option::is_none) {
                    return None;
                }
                let exprs = folded.into_iter().zip(exprs).map(|(f, e)| f.unwrap_or_else(|| e.clone())).collect();
                Some(LogicalPlan::Project { exprs, input: input.clone() })
            }
            LogicalPlan::Scan { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::DefaultRow { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Sort { .. } => None,
        }
    }
}

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
            match or_same(&sa, a) {
                // [X3.135-1986 §5.18 GR2] NOT(true) is false, NOT(false) is true, NOT(unknown) is unknown.
                Cond::Const(t) => Some(Cond::Const(match t {
                    True => False,
                    False => True,
                    Unknown => Unknown,
                })),
                inner @ (Cond::Or(..)
                | Cond::And(..)
                | Cond::Not(_)
                | Cond::Is(..)
                | Cond::Compare(..)
                | Cond::IsNull(..)
                | Cond::Like(..)
                | Cond::Exists(_)
                | Cond::Unique(_)
                | Cond::Quantified(..)
                | Cond::Match { .. }) => sa.map(|_| Cond::Not(Box::new(inner))),
            }
        }
        Cond::Const(_)
        | Cond::Is(..)
        | Cond::Compare(..)
        | Cond::IsNull(..)
        | Cond::Like(..)
        | Cond::Exists(_)
        | Cond::Unique(_)
        | Cond::Quantified(..)
        | Cond::Match { .. } => None,
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
/// a constant HAVING condition must stay where it is.
pub struct PushHavingIntoWhere;

impl Rule for PushHavingIntoWhere {
    fn name(&self) -> &'static str {
        PUSH_HAVING_INTO_WHERE
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond, input } = plan else { return None };
        let LogicalPlan::Aggregate { groups, aggs, input: rows } = &**input else { return None };
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
        let agg = LogicalPlan::Aggregate { groups: groups.clone(), aggs: aggs.clone(), input: Box::new(wrap((**rows).clone(), down)) };
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
        | LogicalPlan::DefaultRow { .. }
        | LogicalPlan::Project { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Sort { .. } => None,
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
                | Expr::Case(..)
                | Expr::Cast(..)
                | Expr::Domain(..)
                | Expr::Subquery(..) => None,
            })
            .collect();
        // A UNIQUE column may be null [ISO/IEC 9075:1992 §8.9], and rows
        // with nulls there may be duplicates: its columns must be NOT NULL.
        let keyed = |c: &usize| projected.contains(c) && desc.columns.get(*c).is_some_and(|col| col.not_null);
        desc.unique().any(|u| u.iter().all(keyed)).then(|| (**input).clone())
    }
}
