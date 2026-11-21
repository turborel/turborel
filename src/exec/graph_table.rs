//! \[EXTENSION\] [GS §8] A GRAPH_TABLE, run: of each row of its input, the
//! table references before it, that row and each row of the GRAPH_TABLE
//! of its operands' values there. Of the second operand of a LEFT join,
//! an input row none of whose rows its condition makes true is kept, its
//! own columns null, as a JSON table's is (`exec/json_table.rs`).
//!
//! Each path pattern is searched by path search over the graph of its
//! element tables (`graph_table/store.rs`), from every vertex of the
//! tables a match of it may start at, within the GRAPH_TABLE's limits,
//! else the settings' [GS §20] (`graph_table/limits.rs`). Its matches are
//! the path bindings joined on the variables they share and kept by the
//! match mode [GS §7.3] (`graph::matching`); each gives its rows if its
//! postfilter is true (`graph_table/values.rs`). A search ended by a
//! limit ON LIMIT TRUNCATE warns, 01W61. Rigid, cyclic path patterns the
//! binder chose it for are matched by a worst-case optimal join instead
//! (`graph::wcoj`, RULES.md D148).

mod correlated;
mod labels;
mod limits;
mod reach;
mod shared;
mod start;
mod store;
mod values;

use std::collections::VecDeque;
use std::sync::Arc;

use super::{RowIter, Runner, Subqueries, broken, eval, eval_cond, execute, failed};
use crate::error::Result;
use crate::exec::Db;
use crate::graph::binding::PathBinding;
use crate::graph::matching::{PathMatches, matches};
use crate::graph::program::{PathPattern, VarId};
use crate::graph::view::ElementId;
use crate::graph::wcoj::Rigid;
use crate::logical::{ElementRead, GraphMatch, Subquery, Truth, Unnest};
use crate::physical::Plan;
use crate::storage::Row;
use crate::txn::{TxId, checked};
use crate::value::Value;
use correlated::Searched;
use shared::Shared;
use store::Store;

/// The rows of `plan`, a GRAPH_TABLE over its input.
#[inline(never)]
pub(super) fn graph_table(plan: Plan, db: &Db, tx: TxId) -> RowIter {
    let Plan::Unnest(u) = plan else { return failed(broken("a GRAPH_TABLE is not one")) };
    let (u, input) = u.split();
    let Some(graph) = u.graph.clone() else { return failed(broken("a GRAPH_TABLE has no graph")) };
    let rows = Rows {
        input: execute(input, db, tx),
        runner: Runner::new(db, tx),
        db: db.clone(),
        tx,
        u,
        graph,
        opened: None,
        made: VecDeque::new(),
        current: None,
    };
    Box::new(checked(rows, db.tx.stop_of(tx)))
}

/// The graph searched, and where each path pattern's matches start:
/// each pattern as searched, and whether reversed (`graph_table/start.rs`).
struct Opened {
    store: Arc<Store>,
    starts: Vec<Vec<ElementId>>,
    /// Of each path pattern, the earlier one it is written as.
    earlier: Vec<Option<usize>>,
    /// Of each path pattern, it as searched, and whether from its end.
    oriented: Vec<(crate::logical::PathShape, bool)>,
}

/// The rows of a GRAPH_TABLE over its input.
struct Rows {
    input: RowIter,
    runner: Runner,
    db: Db,
    tx: TxId,
    u: Unnest<()>,
    graph: Arc<GraphMatch>,
    /// The graph, opened at the first input row.
    opened: Option<Opened>,
    /// The rows of the current input row not yet given.
    made: VecDeque<Row>,
    /// The current input row, and whether a row of it was kept.
    current: Option<(Row, bool)>,
}

impl Iterator for Rows {
    type Item = Result<Row>;

    fn next(&mut self) -> Option<Result<Row>> {
        loop {
            if let Some(row) = self.made.pop_front() {
                return Some(Ok(row));
            }
            // [ISO/IEC 9075-2:2023 §7.10 GR1d] of a LEFT join, the input
            // row null extended if none of its rows was kept.
            if let Some((row, false)) = self.current.take()
                && self.u.outer.is_some()
            {
                let mut padded = row;
                padded.resize(padded.len().saturating_add(self.graph.columns.len()), Value::Null);
                return Some(Ok(padded));
            }
            let row = match self.input.next()? {
                Ok(row) => row,
                Err(e) => return Some(Err(e)),
            };
            match self.of(&row) {
                Ok(kept) => self.current = Some((row, kept)),
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

impl Rows {
    /// Opens the graph the first time: each element table's rows held if
    /// they are not stored, and each path pattern's starts.
    fn open(&mut self) -> Result<()> {
        if self.opened.is_some() {
            return Ok(());
        }
        let mut held = Vec::new();
        for t in &self.graph.tables {
            held.push(match &t.read {
                ElementRead::Stored => None,
                ElementRead::Held(plan) => {
                    let rows = self.runner.rows(&Subquery::new(plan.clone(), Vec::new()), &[], None)?;
                    Some(rows.rows()?.collect::<Result<Vec<_>>>()?)
                }
            });
        }
        let store = Arc::new(Store::new(&self.db, self.tx, Arc::clone(&self.graph), held)?);
        let mut starts = Vec::new();
        let oriented =
            start::oriented(&self.db, &self.graph, &limits::options(&self.db, self.tx, (&self.graph.options, self.graph.hints.path_algo)));
        for (path, _) in &oriented {
            let mut of = Vec::new();
            for &t in &path.starts {
                of.extend(labels::starts(&store, path, t)?);
            }
            starts.push(of);
        }
        self.opened = Some(Opened { store, starts, earlier: self.graph.earlier(), oriented });
        Ok(())
    }

    /// The path bindings of each path pattern of `shared`, the patterns
    /// searched, and each binding's choice of the bindings it read.
    fn searched(&self, shared: &Arc<Shared>, (starts, earlier): (&[Vec<ElementId>], &[Option<usize>])) -> Result<Vec<Searched>> {
        if self.graph.wcoj {
            return Ok(self.joined(shared, starts)?.into_iter().map(|(p, b)| (p, b, Vec::new())).collect());
        }
        let options = limits::options(&self.db, self.tx, (&self.graph.options, self.graph.hints.path_algo));
        let oriented = self.opened.as_ref().map(|o| o.oriented.as_slice()).unwrap_or_default();
        let mut out: Vec<Option<Searched>> = vec![None; oriented.len()];
        for n in correlated::order(&self.graph) {
            let Some((path, reversed)) = oriented.get(n) else { continue };
            let from = starts.get(n).map_or(&[][..], Vec::as_slice);
            let found = self.path_searched(shared, (n, path, *reversed), (from, &options, earlier.get(n).copied().flatten()), &out)?;
            if let Some(slot) = out.get_mut(n) {
                *slot = Some(found);
            }
        }
        out.into_iter().map(|o| o.ok_or_else(|| broken("a path pattern was not searched"))).collect()
    }

    /// \[GS §16 R16.2\] The path bindings of each rigid path pattern of
    /// `shared`, of the worst-case optimal join (`graph::wcoj`).
    fn joined(&self, shared: &Arc<Shared>, starts: &[Vec<ElementId>]) -> Result<Vec<(PathPattern, Vec<PathBinding>)>> {
        let patterns: Vec<PathPattern> = self.graph.paths.iter().map(|p| p.instantiate(&mut |k| shared::filter(shared, k))).collect();
        let vars = &self.graph.vars;
        let global = |n: usize, var: VarId| vars.iter().position(|v| v.bound.contains(&(n, var)));
        let rigid =
            Rigid::new(&patterns, (&global, vars.len())).ok_or_else(|| broken("a worst-case optimal join's pattern is not rigid"))?;
        let found = rigid.bindings(&*shared.store, starts)?;
        drop(rigid);
        Ok(patterns.into_iter().zip(found).collect())
    }

    /// Makes the rows of input row `row`; whether one was kept.
    fn of(&mut self, row: &Row) -> Result<bool> {
        let operands = self.u.collections.iter().map(|e| eval(e, row, &self.runner)).collect::<Result<Vec<_>>>()?;
        self.open()?;
        let opened = self.opened.as_ref().ok_or_else(|| broken("a GRAPH_TABLE's graph is not open"))?;
        let (store, starts, earlier) = (Arc::clone(&opened.store), opened.starts.clone(), opened.earlier.clone());
        let fixed = vec![None; self.graph.paths.len()];
        let shared = Arc::new(Shared { db: self.db.clone(), tx: self.tx, graph: Arc::clone(&self.graph), store, operands, fixed });
        let (mut patterns, mut found, mut tags) = (Vec::new(), Vec::new(), Vec::new());
        for (pattern, bindings, read) in self.searched(&shared, (&starts, &earlier))? {
            patterns.push(pattern);
            found.push(bindings);
            tags.push(read);
        }
        let paths: Vec<PathMatches<'_>> = patterns.iter().zip(found).map(|(pattern, bindings)| PathMatches { pattern, bindings }).collect();
        let mut out = Vec::new();
        for choice in matches(&paths, self.graph.mode) {
            if !correlated::consistent(&tags, &choice) {
                continue;
            }
            let chosen: values::Chosen<'_> = paths.iter().zip(&choice).filter_map(|(p, &c)| p.bindings.get(c)).collect();
            let matched = shared.match_row(&chosen, &self.runner)?;
            if let Some(c) = &self.graph.postfilter
                && eval_cond(c, &matched, &self.runner)? != Truth::True
            {
                continue;
            }
            if !shared.segments_hold(&chosen, &matched, &self.runner)? {
                continue;
            }
            shared.columns_of(&chosen, &matched, &self.runner, &mut out)?;
        }
        let _ = self.runner.warnings();
        self.kept(row, out)
    }

    /// Queues the rows of input row `row` of COLUMNS' values `made`, of a
    /// LEFT join those its condition is true of; whether one was.
    fn kept(&mut self, row: &Row, made: Vec<Vec<Value>>) -> Result<bool> {
        let mut kept = false;
        for values in made {
            let mut r = row.clone();
            r.extend(values);
            if let Some(cond) = &self.u.outer
                && eval_cond(cond, &r, &self.runner)? != Truth::True
            {
                continue;
            }
            kept = true;
            self.made.push_back(r);
        }
        Ok(kept)
    }
}
