//! Shared construction of candidate joins, preserving every condition and
//! restoring projected table columns to their logical positions (D35, D85).

use super::{Probing, Read};
use crate::catalog::IndexDesc;
use crate::logical::{Cond, Expr, and_all, judged_by_sets, map_cond_columns, render_cond, render_expr};
use crate::physical::shown::Shown;
use crate::physical::{JoinProbe, Paired, PhysicalPlanner, Plan, Preserved, Spatial, Unmatched};
use crate::storage::cost_of;

pub(super) struct Choice {
    pub(super) read: Probing,
    pub(super) probe: JoinProbe,
    pub(super) total: f64,
    pub(super) spatial: bool,
    pub(super) preserve: Preserve,
    /// The outer table's R-tree, of a traversal of both trees (D138).
    pub(super) paired: Option<Box<Paired>>,
}

/// Which unmatched rows a planned join keeps (`Preserved`): none, the
/// probing operand's, or both operands'.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Preserve {
    Inner,
    Outer,
    Both,
}

impl PhysicalPlanner<'_> {
    pub(in crate::physical::indexed) fn join_cost(
        &self,
        read: &Read<'_>,
        ix: &IndexDesc,
        probe: JoinProbe,
        (outer, share): (f64, f64),
    ) -> Option<Choice> {
        let storage = self.db.storage_of(read.table).ok()?;
        let desc = self.db.catalog.table(read.table)?;
        let model = self.weights();
        let rows = cost_of(storage.row_count());
        let each = rows * share;
        let cost = model.index_descent * (rows + 1.0_f64).log2() + each * storage.fetch_cost(read.needed.len());
        let probing = Probing {
            table: read.table.clone(),
            filter: read.filter.cloned(),
            width: desc.columns.len().saturating_add(usize::from(read.located)),
            index: ix.name.clone(),
            method: ix.method.clone(),
            used: Vec::new(),
            each,
            cost,
            through: read.through.clone(),
            needed: read.needed.to_vec(),
            located: read.located,
            positions: None,
            bounds: Vec::new(),
            bounded: Vec::new(),
        };
        Some(Choice { read: probing, probe, total: outer * cost, spatial: false, preserve: Preserve::Inner, paired: None })
    }
}

pub(super) fn planned(choice: Choice, outer: Plan, (cond, width): (Cond, usize), shown: Shown<'_>) -> Plan {
    let Choice { mut read, probe, total, spatial, preserve, paired } = choice;
    let spatial = match (paired, spatial) {
        (Some(paired), _) => Some(Box::new(Spatial::Paired(paired))),
        (None, true) => Some(Box::new(Spatial::Adaptive)),
        (None, false) => None,
    };
    let through = read.through.take();
    let mapped = |c: usize| match &through {
        Some((columns, _)) => c.checked_sub(width).and_then(|i| columns.get(i)).map_or(c, |i| i.saturating_add(width)),
        None => c,
    };
    let cond = map_cond_columns(cond, &mapped);
    let filter = read.filter.map(|c| map_cond_columns(c, &|i| i.saturating_add(width)));
    let kept = preserved(preserve, filter.as_ref(), width);
    let condition = and_all(std::iter::once(cond).chain(filter).collect()).map(judged_by_sets);
    let joined_names = through
        .as_ref()
        .and_then(|(_, table)| shown.names().map(|names| names.iter().take(width).chain(table).cloned().collect::<Vec<_>>()));
    let table_shown = joined_names.as_ref().map_or(shown, |names| Shown::of(Some(names)));
    let detail = table_shown.text(|names| condition.as_ref().map(|c| render_cond(c, names)).unwrap_or_default());
    let joined = Plan::IndexJoin {
        outer: kept,
        spatial,
        left: Box::new(outer),
        table: read.table,
        index: read.index.into(),
        method: read.method.into(),
        probe,
        cond: condition,
        width: read.width,
        columns: read.needed.into_boxed_slice(),
        detail: detail.into(),
        est: read.each,
        cost: total,
        located: read.located,
    };
    match through {
        Some((columns, _)) => {
            let exprs: Vec<_> = (0..width).chain(columns.iter().map(|&i| i.saturating_add(width))).map(Expr::Column).collect();
            let projection = table_shown.text(|names| exprs.iter().map(|e| render_expr(e, names)).collect::<Vec<_>>().join(", "));
            Plan::Project { exprs, detail: projection, input: Box::new(joined) }
        }
        None => joined,
    }
}

/// The rows matching none a join keeps, of the table's `filter` over the
/// pair's columns, the probing operand `width` columns wide.
fn preserved(preserve: Preserve, filter: Option<&Cond>, width: usize) -> Preserved {
    match preserve {
        Preserve::Inner => Preserved::Inner,
        Preserve::Outer => Preserved::Outer,
        Preserve::Both => Preserved::Both(Box::new(Unmatched { filter: filter.cloned().map(judged_by_sets), width })),
    }
}
