//! \[DESIGN\] A cursor's specification compiled: bound, optimized and
//! planned, to run or to be shown, as EXPLAIN shows it; and, of one its
//! text makes updatable, the one table it reads [X3.135-1986 §8.3 SR4b].

use super::prepared::Keeping;
use super::{Session, State};
use crate::ast::CursorSpecification;
use crate::binder::OutColumn;
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
        let (optimized, trace, physical) = described(st, logical);
        Ok(Compiled { columns, bound, trace, optimized, physical })
    }

    /// `spec` compiled to run: of a prepared statement, from the plan it
    /// keeps (`keeping`, RULES.md D45). Nothing is shown of it, so no text
    /// is made.
    fn compile_kept(&self, st: &State, spec: &CursorSpecification, params: &Params, keeping: Option<&Keeping<'_>>) -> Result<Compiled> {
        let (columns, logical) = self.cursor_bound(st, spec, params, keeping)?;
        let (optimized, trace, physical) = plan(st, logical);
        Ok(Compiled { columns, bound: String::new(), trace, optimized, physical })
    }

    /// [X3.135-1986 §8.3 SR4b; ISO/IEC 9075:1992 §13.1 SR5, §7.9 SR12] The
    /// compiled specification of a cursor its text makes updatable, and
    /// the one table it reads, unless a subquery of its WHERE clause reads
    /// that table too \[SR12d\]: then it is read-only.
    pub(super) fn updatable_plan(
        &self,
        st: &State,
        spec: &CursorSpecification,
        params: &Params,
        keeping: Option<&Keeping<'_>>,
    ) -> Result<Option<(Compiled, QualifiedName)>> {
        let c = self.compile_kept(st, spec, params, keeping)?;
        let read = c.optimized.subquery_tables();
        let own: Vec<QualifiedName> = c.optimized.tables().into_iter().filter(|t| !read.contains(t)).collect();
        let [table] = own.as_slice() else { return Ok(None) };
        let table = table.clone();
        Ok(Some((c, table)))
    }
}
