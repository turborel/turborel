//! \[DESIGN\] The query a SELECT ... INTO of a kept program reads its
//! delta table by, kept as a prepared query's is (RULES.md D793). Its
//! statement keeps the plan of the change it runs first (D792); the query
//! keeps its own beside it, bound once over a transition table of its
//! own, which each run refills with the rows the change gives
//! (`db/delta.rs`), its physical plan kept or learned as a prepared
//! query's; so is a kept INSERT's query (`db/write/inserted.rs`).

use std::sync::OnceLock;

use super::{Keeping, KeptPlan, Plans};
use crate::binder::{Bound, Context, OutColumn};
use crate::error::Result;
use crate::exec::{HeldRows, TransitionRows};
use crate::logical::{LogicalPlan, filled};
use crate::params::Params;
use crate::storage::Row;

/// The query of a SELECT ... INTO of a kept program over a delta table:
/// its plans, and the rows it reads, made at its first run.
#[derive(Debug, Default)]
pub(in crate::db) struct DeltaQuery {
    plans: Plans,
    rows: OnceLock<TransitionRows>,
}

impl KeptPlan {
    /// Its query's columns and plan: a query's, or an INSERT's, whose
    /// query is kept as a query's is (RULES.md D793), of no columns.
    pub(super) fn query(&self) -> Option<(Vec<OutColumn>, &LogicalPlan)> {
        match self {
            KeptPlan::Query(columns, logical) => Some((columns.clone(), logical)),
            KeptPlan::Insert(_, Some(logical)) => Some((Vec::new(), logical)),
            KeptPlan::Write(_) | KeptPlan::Insert(_, None) | KeptPlan::Merge(..) | KeptPlan::Call(_) => None,
        }
    }
}

impl<'a> Keeping<'a> {
    /// It, its statement's query over its delta table `query`.
    pub(in crate::db) fn querying(self, query: Option<&'a DeltaQuery>) -> Keeping<'a> {
        Keeping { query, ..self }
    }

    /// The plans of its statement's query over its delta table, as this
    /// execution sees them, if it keeps them.
    pub(in crate::db) fn query(&self) -> Option<Keeping<'a>> {
        self.query.map(|q| Keeping { plans: &q.plans, st: self.st, session: self.session, query: None })
    }

    /// Of a statement whose query over its delta table is kept, the rows
    /// that query reads, refilled with `held`, the change's; else `held`
    /// as rows of their own.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn delta_rows(keeping: Option<&Keeping<'_>>, held: HeldRows<Row>) -> Result<TransitionRows> {
        let Some((query, st)) = keeping.and_then(|k| k.query.map(|q| (q, k.st))) else { return Ok(TransitionRows::with(held)) };
        let rows = query.rows.get_or_init(|| TransitionRows::new(st.list_space()));
        rows.give(held)?;
        Ok(rows.clone())
    }

    /// The query's columns and plan, its values of `params` filled in: as
    /// it keeps them, else as `bind` binds it with `params`' slots, kept
    /// unless the binding holds a value.
    ///
    /// # Errors
    ///
    /// Those of binding it; as `filled`; SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn query_bound(
        &self,
        params: &Params,
        bind: impl FnOnce(Context<'_>) -> Result<Bound>,
    ) -> Result<(Vec<OutColumn>, LogicalPlan)> {
        let values = params.dynamic_values();
        if let Some(KeptPlan::Query(columns, plan)) = self.plan()? {
            return Ok((columns, filled(plan, values)?));
        }
        let binding = params.keeping();
        let bound = self.session.bound(self.st, &binding, bind)?;
        self.made((!binding.held()).then(|| KeptPlan::Query(bound.columns.clone(), bound.plan.clone())))?;
        Ok((bound.columns, filled(bound.plan, values)?))
    }
}
