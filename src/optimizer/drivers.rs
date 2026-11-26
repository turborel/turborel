//! The driver of RULES.md D371 against the one it replaced, which ended a
//! node's turn at its first rewrite and restarted a pass over the whole
//! plan. On queries that fire every built-in rule both reach the same plan,
//! the new one in fewer passes, and rules that undo each other stop once
//! the budget of rewrites is spent.

use super::{Optimizer, Rule, RuleContext};
use crate::db::Database;
use crate::logical::{Cond, LogicalPlan, Truth};

/// The driver replaced: a pass whose node stops at its first rewrite.
fn restarting(on: &[&dyn Rule], plan: LogicalPlan, ctx: &RuleContext<'_>, trace: &mut Vec<&'static str>) -> (LogicalPlan, bool) {
    let mut changed = false;
    let plan = plan.map_inputs(|input| {
        let (p, c) = restarting(on, input, ctx, trace);
        changed |= c;
        p
    });
    for rule in on {
        if let Some(p) = rule.apply(&plan, ctx) {
            trace.push(rule.name());
            return (p, true);
        }
    }
    (plan, changed)
}

/// `plan` rewritten by the replaced driver, to its fixpoint.
fn restarted(optimizer: &Optimizer, mut plan: LogicalPlan, ctx: &RuleContext<'_>) -> (LogicalPlan, usize) {
    let on: Vec<&dyn Rule> = optimizer.rules.iter().filter(|r| !r.late).map(|r| &*r.rule).collect();
    let mut trace = Vec::new();
    for passes in 1..=super::DEFAULT_MAX_PASSES {
        let (p, changed) = restarting(&on, plan, ctx, &mut trace);
        plan = p;
        if !changed {
            return (plan, passes);
        }
    }
    (plan, super::DEFAULT_MAX_PASSES)
}

/// `plan`, a plan's debug text, its subqueries' numbers taken out: each
/// rewrite that makes a guard numbers its subquery anew.
fn unnumbered(plan: &str) -> String {
    let mut out = String::new();
    let mut parts = plan.split("id: ");
    out.push_str(parts.next().unwrap_or_default());
    for part in parts {
        out.push_str(part.trim_start_matches(|c: char| c.is_ascii_digit()));
    }
    out
}

/// Queries that fire every built-in rule: inlining, unnesting, folding,
/// simplifying, merging filters, pushing them through projections,
/// DISTINCT, products and outer joins, factoring ORs, transitivity,
/// guards, HAVING into WHERE and redundant DISTINCT.
const QUERIES: &[&str] = &[
    "SELECT * FROM T, U WHERE T.K = U.K AND U.K = 3 AND 1 = 1",
    "SELECT * FROM (SELECT K, G FROM T WHERE G > 1) AS D, U WHERE D.K = U.K AND D.G < 5",
    "SELECT DISTINCT K FROM (SELECT DISTINCT K, G FROM T) AS D WHERE K > 2",
    "SELECT * FROM T LEFT JOIN U ON T.K = U.K WHERE T.G = 2 AND U.W IS NULL",
    "SELECT * FROM T, U WHERE (T.K = U.K AND T.G = 1) OR (T.K = U.K AND U.W = 2)",
    "SELECT * FROM T, U WHERE T.K = U.K AND 10 / T.G > 1",
    "SELECT G, COUNT(*) FROM T GROUP BY G HAVING G > 3",
    "SELECT * FROM T WHERE K IN (SELECT K FROM U WHERE U.W > T.G)",
    "SELECT * FROM T WHERE EXISTS (SELECT * FROM U WHERE U.K = T.K) AND NOT (G = 1 AND TRUE)",
    "SELECT K, (SELECT MAX(W) FROM U WHERE U.K = T.K) FROM T WHERE G = (SELECT MIN(G) FROM T)",
    "SELECT * FROM T, U, V WHERE T.K = U.K AND U.K = V.K AND V.K < 10",
    "SELECT * FROM T FULL JOIN U ON T.K = U.K AND T.G = 1",
];

/// Verifies: [DESIGN] RULES.md D371: the driver that rewrites a node again
/// after it fired reaches the plan the restarted passes reached, on
/// queries firing every built-in rule, in no more passes.
#[test]
fn the_local_fixpoint_reaches_the_same_plan() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION O
           CREATE TABLE T (K INTEGER NOT NULL, G INTEGER)
           CREATE TABLE U (K INTEGER NOT NULL, W INTEGER)
           CREATE TABLE V (K INTEGER NOT NULL)",
    )
    .unwrap();
    let s = db.session("O");
    for sql in QUERIES {
        let (st, bound) = s.bound_plan(sql).unwrap();
        let ctx = RuleContext { db: &st };
        let (local, trace) = st.optimizer.optimize(bound.clone(), &ctx);
        let (restarted_plan, passes) = restarted(&st.optimizer, bound, &ctx);
        assert_eq!(unnumbered(&format!("{local:?}")), unnumbered(&format!("{restarted_plan:?}")), "{sql}");
        assert!(!trace.is_empty() || passes == 1, "{sql}");
    }
}

/// Verifies: [DESIGN] RULES.md D371: the driver trying the rules again at
/// a node it rewrote reaches the fixpoint in fewer passes, over the
/// queries, than the driver restarting a pass after each rewrite.
#[test]
fn rewriting_a_node_again_takes_fewer_passes() {
    let db = Database::new();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION O
           CREATE TABLE T (K INTEGER NOT NULL, G INTEGER)
           CREATE TABLE U (K INTEGER NOT NULL, W INTEGER)
           CREATE TABLE V (K INTEGER NOT NULL)",
    )
    .unwrap();
    let s = db.session("O");
    let (mut local, mut restarting_passes) = (0_usize, 0_usize);
    for sql in QUERIES {
        let (st, bound) = s.bound_plan(sql).unwrap();
        let ctx = RuleContext { db: &st };
        let on: Vec<&dyn Rule> = st.optimizer.rules.iter().filter(|r| !r.late).map(|r| &*r.rule).collect();
        let (mut plan, mut budget, mut trace) = (bound.clone(), usize::MAX, Vec::new());
        for _ in 0..super::DEFAULT_MAX_PASSES {
            let (p, changed) = super::pass(&on, plan, (&ctx, &mut budget), &mut trace);
            plan = p;
            local = local.saturating_add(1);
            if !changed {
                break;
            }
        }
        restarting_passes = restarting_passes.saturating_add(restarted(&st.optimizer, bound, &ctx).1);
    }
    assert!(local < restarting_passes, "{local} against {restarting_passes}");
}

/// A rule turning `TRUE` into `NOT FALSE`, and one turning it back: two
/// rules that undo each other, as a faulty extension's may.
struct Flip(bool);

impl Rule for Flip {
    fn name(&self) -> &'static str {
        if self.0 { "flip" } else { "flop" }
    }

    fn apply(&self, plan: &LogicalPlan, _: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond, input } = plan else { return None };
        let shown = format!("{cond:?}");
        let (yes, no) = (Cond::Const(Truth::True), Cond::Not(Box::new(Cond::Const(Truth::False))));
        let flipped = if self.0 && shown == format!("{yes:?}") {
            no
        } else if !self.0 && shown == format!("{no:?}") {
            yes
        } else {
            return None;
        };
        Some(LogicalPlan::Filter { cond: flipped, input: input.clone() })
    }
}

/// `depth` filters of TRUE over a row.
fn plan_of(depth: usize) -> LogicalPlan {
    let mut plan = LogicalPlan::Values { rows: vec![vec![]], types: vec![] };
    for _ in 0..depth {
        plan = LogicalPlan::Filter { cond: Cond::Const(Truth::True), input: Box::new(plan) };
    }
    plan
}

/// Verifies: [DESIGN] RULES.md D371, regression test (R-TEST-7): rules that
/// undo each other at every node of a deep plan stop once the budget of
/// rewrites is spent, here as many as the plan has nodes, and the whole
/// optimization at the passes' limit times that; the driver rewriting the
/// inputs a rewrite made again with no bound ran for hours on such plans.
#[test]
fn rules_undoing_each_other_end_within_the_budget() {
    let db = Database::new();
    let st = db.snapshot().unwrap();
    let ctx = RuleContext { db: &st };
    let plan = plan_of(40);
    let (forth, back) = (Flip(true), Flip(false));
    let on: [&dyn Rule; 2] = [&forth, &back];
    let mut budget = super::nodes(&plan);
    assert_eq!(budget, 41);
    let mut trace = Vec::new();
    let (_, changed) = super::pass(&on, plan, (&ctx, &mut budget), &mut trace);
    assert!(changed && budget == 0 && trace.len() == 41, "{}", trace.len());
    let mut optimizer = Optimizer::empty();
    optimizer.add(Box::new(Flip(true)));
    optimizer.add(Box::new(Flip(false)));
    let (_, all) = optimizer.optimize(plan_of(40), &ctx);
    assert_eq!(all.len(), 41 * super::DEFAULT_MAX_PASSES);
}

/// Verifies: [DESIGN] RULES.md D1201: a late rule is applied once the
/// others have reached their fixpoint, and they are not applied again.
/// Of two rules that undo each other, the late one rewrites each node
/// once and the other, which found nothing to rewrite before it, never.
#[test]
fn a_late_rule_is_applied_after_the_others() {
    let db = Database::new();
    let st = db.snapshot().unwrap();
    let ctx = RuleContext { db: &st };
    let mut optimizer = Optimizer::empty();
    optimizer.add_late(Box::new(Flip(true)));
    optimizer.add(Box::new(Flip(false)));
    let (plan, trace) = optimizer.optimize(plan_of(5), &ctx);
    assert_eq!(trace, ["flip"; 5]);
    assert!(!format!("{plan:?}").contains("Const(True)"), "{plan:?}");
}
