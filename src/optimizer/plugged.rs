//! \[EXTENSION\] The optimizer rules of extensions, run as a statement is
//! planned (RULES.md D275, D276, D277).
//!
//! Each rule's PHASE says when: BOUND rules rewrite the plan as bound,
//! before the built-in rules, and REWRITTEN rules the plan they made,
//! before the physical planner orders joins. A phase's rules run in the
//! order of their extensions' names, then of their manifests, each a pass
//! at a time up to its PASSES, and no more than OPTIMIZER.MAX_PASSES,
//! until it answers no change. A pass sends the plan's form (`plan.rs`)
//! and takes back the form rewritten, which is checked whole
//! (`plan/back.rs`).
//!
//! A rule is an optimization, so its failure fails nothing. A rule whose
//! call fails, runs past PLUGINS.CALL_TIMEOUT_MS or gives a plan not
//! valid is skipped for the statement, every pass of it undone, EXPLAIN
//! names it skipped, and the statement completes with the warning
//! SQLSTATE 01T02 where its warnings are heard. A statement planned makes
//! one call of each rule a pass, so a rule costs a round trip a pass of
//! every statement planned; a prepared statement's plan, kept, makes none.
//! The subqueries a statement runs again as it runs are not given to
//! rules, which see their plans opaque.

use std::sync::Arc;

use super::RuleContext;
use crate::db::State;
use crate::error::{Result, SqlState, Warnings};
use crate::logical::LogicalPlan;
use crate::physical::Seen;
use crate::plugin::Bridged;
use crate::plugin::bridge::leaked;
use crate::plugin::bridge::rule::{Phase, PluginRule};
use crate::plugin::plan::{form_of, plan_of};
use crate::settings::OPTIMIZER_MAX_PASSES;

/// \[DESIGN\] SQLSTATE 01T02, warning: an optimizer rule of an extension
/// was skipped, as it failed or gave a plan not valid (RULES.md D277).
pub const RULE_SKIPPED: SqlState = SqlState::of_bytes(*b"01T02");

/// What EXPLAIN shows after the name of a rule skipped.
const SKIPPED: &str = "skipped";

/// The rules of `st`'s extensions of `phase`, in the order they run.
fn rules(st: &State, phase: Phase) -> Vec<Arc<PluginRule>> {
    let all = st.extensions.values().flat_map(|e| e.objects.iter());
    all.filter_map(|(_, b)| if let Bridged::Rule(r) = b { Some(Arc::clone(r)) } else { None }).filter(|r| r.phase() == phase).collect()
}

/// `plan` rewritten by one pass of `rule`, if it rewrote it.
///
/// # Errors
///
/// The rule's call's, or SQLSTATE 38T03 of a plan not valid.
fn pass(st: &State, rule: &PluginRule, plan: &LogicalPlan) -> Result<Option<LogicalPlan>> {
    let (form, kept) = form_of(plan, st)?;
    let Some(rewritten) = rule.rewrite(&form)? else { return Ok(None) };
    plan_of(&rewritten, &kept, st).map(Some)
}

/// `plan` rewritten by the rules of `phase`, the names of those that
/// fired added to `trace`: and whether one was skipped.
fn run(st: &State, phase: Phase, mut plan: LogicalPlan, trace: &mut Vec<&'static str>) -> (LogicalPlan, bool) {
    let most = u32::try_from(st.settings.integer(OPTIMIZER_MAX_PASSES)).unwrap_or(u32::MAX);
    let mut skipped = false;
    for rule in rules(st, phase) {
        let (before, fired) = (plan.clone(), trace.len());
        for _ in 0..rule.passes().min(most) {
            match pass(st, &rule, &plan) {
                Ok(Some(p)) => {
                    plan = p;
                    trace.push(rule.name());
                }
                Ok(None) => break,
                Err(_) => {
                    plan = before;
                    trace.truncate(fired);
                    trace.push(leaked(&format!("{} {SKIPPED}", rule.name())));
                    skipped = true;
                    break;
                }
            }
        }
    }
    (plan, skipped)
}

/// `logical` optimized: the extensions' BOUND rules, the built-in rules
/// `st`'s settings leave on, and the extensions' REWRITTEN rules; with
/// the names of the rules that fired, in order. A rule skipped raises
/// RULE_SKIPPED to `heard`, if given. The built-in rules named in `except`
/// are left off. Its rules see the statistics as `seen`, the statement's
/// (RULES.md D1089).
pub(crate) fn optimize(
    (st, seen): (&State, &Seen),
    logical: LogicalPlan,
    (heard, except): (Option<&Warnings>, &[&str]),
) -> (LogicalPlan, Vec<&'static str>) {
    let none = st.extensions.values().all(|e| e.objects.iter().all(|(_, b)| !matches!(b, Bridged::Rule(_))));
    if none {
        return st.optimizer.optimize_except(logical, &RuleContext { db: st, seen }, except);
    }
    let mut trace = Vec::new();
    let (bound, early) = run(st, Phase::Bound, logical, &mut trace);
    let (optimized, built_in) = st.optimizer.optimize_except(bound, &RuleContext { db: st, seen }, except);
    trace.extend(built_in);
    let (rewritten, late) = run(st, Phase::Rewritten, optimized, &mut trace);
    if let Some(w) = heard.filter(|_| early || late) {
        w.raise(RULE_SKIPPED);
    }
    (rewritten, trace)
}
