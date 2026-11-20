//! The driver of RULES.md D371 against the one it replaced, which ended a
//! node's turn at its first rewrite and restarted a pass over the whole
//! plan: on queries that fire every built-in rule, both reach the same
//! plan, the one each rule's equivalence and their common fixpoint give.

use super::{Optimizer, Rule, RuleContext};
use crate::db::Database;
use crate::logical::LogicalPlan;

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
    let on: Vec<&dyn Rule> = optimizer.rules.iter().map(|r| &*r.rule).collect();
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
    let mut rest = plan;
    while let Some(at) = rest.find("id: ") {
        out.push_str(&rest[..at]);
        rest = rest[at + 4..].trim_start_matches(|c: char| c.is_ascii_digit());
    }
    out.push_str(rest);
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

/// Verifies: [DESIGN] RULES.md D371: one pass of the driver reaches the
/// fixpoint, so a second pass finds no rule to fire, on queries whose
/// rewrites cascade through the nodes they make.
#[test]
fn one_pass_reaches_the_fixpoint() {
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
        let on: Vec<&dyn Rule> = st.optimizer.rules.iter().map(|r| &*r.rule).collect();
        let mut trace = Vec::new();
        let (once, _) = super::pass(&on, bound, (&ctx, super::DEFAULT_MAX_PASSES), &mut trace);
        let (_, again) = super::pass(&on, once, (&ctx, super::DEFAULT_MAX_PASSES), &mut trace);
        assert!(!again, "{sql}: {trace:?}");
    }
}
