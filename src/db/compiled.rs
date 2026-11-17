//! \[DESIGN\] A cursor's specification compiled: bound, optimized and
//! planned, to run or to be shown, as EXPLAIN shows it; and, of one its
//! text makes updatable, the one table it reads [X3.135-1986 §8.3 SR4b].

use super::prepared::Keeping;
use super::{Session, State};
use crate::ast::CursorSpecification;
use crate::binder::{DescribedKey, OutColumn, described_key, family_in, member_plan};
use crate::catalog::QualifiedName;
use crate::error::Result;
use crate::logical::LogicalPlan;
use crate::optimizer::RuleContext;
use crate::params::Params;
use crate::physical::{PhysicalPlanner, Plan};
use crate::value::Value;

/// A cursor's specification bound and planned: its columns, its bound plan
/// shown, if compiled to be shown, the optimizer's trace, and its plans.
pub(super) struct Compiled {
    pub(super) columns: Vec<OutColumn>,
    /// [ISO/IEC 9075-2:2023 §4.27] Of a statement compiled to be shown or
    /// described, the key of its rows; none of one compiled to run.
    pub(super) key: Option<DescribedKey>,
    pub(super) bound: String,
    pub(super) trace: Vec<&'static str>,
    pub(super) optimized: LogicalPlan,
    pub(super) physical: Plan,
}

/// Optimize a logical plan and choose its physical plan, to run.
pub(super) fn plan(st: &State, logical: LogicalPlan) -> (LogicalPlan, Vec<&'static str>, Plan) {
    planned(st, logical, PhysicalPlanner::new(st))
}

/// As `plan`, of a prepared statement's plan with its slots: its paths
/// chosen by `values`, one execution's, its probes holding the slots, so
/// that another execution may fill them (`physical/kept.rs`).
pub(super) fn plan_slotted(st: &State, logical: LogicalPlan, values: &[Value]) -> Plan {
    planned(st, logical, PhysicalPlanner::slotted(st, values)).2
}

/// As `plan`, to show: each operator of the physical plan described, as
/// EXPLAIN shows it (`physical/shown.rs`).
fn described(st: &State, logical: LogicalPlan) -> (LogicalPlan, Vec<&'static str>, Plan) {
    planned(st, logical, PhysicalPlanner::describing(st))
}

/// `logical` optimized, the rules that fired, and the physical plan
/// `planner` chooses for it.
fn planned(st: &State, logical: LogicalPlan, planner: PhysicalPlanner<'_>) -> (LogicalPlan, Vec<&'static str>, Plan) {
    let (optimized, trace) = st.optimizer.optimize(logical, &RuleContext { db: st });
    let physical = planner.plan(&optimized);
    (optimized, trace, physical)
}

impl Session {
    /// `spec` compiled to be shown, as EXPLAIN shows it: its plan as bound
    /// as text, and each operator of its physical plan described.
    pub(super) fn compile(&self, st: &State, spec: &CursorSpecification, params: &Params) -> Result<Compiled> {
        let (columns, logical) = self.cursor_bound(st, spec, params, None)?;
        let bound = logical.explain();
        let key = Some(described_key(st, &logical));
        let (optimized, trace, physical) = described(st, logical);
        Ok(Compiled { columns, key, bound, trace, optimized, physical })
    }

    /// [X3.135-1986 §8.3 SR4b; ISO/IEC 9075:1992 §13.1 SR5, §7.9 SR12] The
    /// compiled specification of a cursor its text makes updatable, and
    /// the one table it reads, unless a subquery of its WHERE clause reads
    /// that table too \[SR12d\], or a column of it has no counterpart in
    /// that table [ISO/IEC 9075-2:2023 §7.16 SR25c]: then it is read-only.
    pub(super) fn updatable_plan(
        &self,
        st: &State,
        spec: &CursorSpecification,
        params: &Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<Option<Updating>> {
        let (columns, logical) = self.cursor_bound(st, spec, params, keeping)?;
        // [ISO/IEC 9075-2:2023 §4.17.9] a typed table read with its proper
        // subtables: each member's rows read by a plan of its own, the
        // table's own first, as the cursor's.
        let members = family_in(st, &logical).unwrap_or_default();
        let others: Vec<LogicalPlan> = (1..members.len()).map(|k| member_plan(logical.clone(), &members, k)).collect();
        let logical = if members.is_empty() { logical } else { member_plan(logical, &members, 0) };
        let (optimized, trace, physical) = plan(st, logical);
        let c = Compiled { columns, key: None, bound: String::new(), trace, optimized, physical };
        let Some(table) = only_table(&c.optimized) else { return Ok(None) };
        // [ISO/IEC 9075-2:2023 §7.16 SR25c] every result column has a
        // counterpart in the table: not an outer reference, nor an SQL
        // parameter's or variable's column [SR17c].
        if c.columns.iter().any(|col| col.origin.is_none()) {
            return Ok(None);
        }
        let mut subtables = Vec::new();
        for (member, other) in members.iter().skip(1).zip(others) {
            let (best, _, run) = plan(st, other);
            if only_table(&best).as_ref() != Some(member) {
                return Ok(None);
            }
            subtables.push((member.clone(), run));
        }
        Ok(Some(Updating { compiled: c, table, subtables }))
    }
}

/// The one table `optimized` reads but in a subquery, if it reads one.
fn only_table(optimized: &LogicalPlan) -> Option<QualifiedName> {
    let read = optimized.subquery_tables();
    let own: Vec<QualifiedName> = optimized.tables().into_iter().filter(|t| !read.contains(t)).collect();
    let [table] = own.as_slice() else { return None };
    Some(table.clone())
}

/// The compiled specification of an updatable cursor, and what it reads:
/// its one table, by `compiled`'s plan; and of a typed table read with its
/// proper subtables, each subtable, by a plan of its own.
pub(super) struct Updating {
    pub(super) compiled: Compiled,
    pub(super) table: QualifiedName,
    pub(super) subtables: Vec<(QualifiedName, Plan)>,
}
