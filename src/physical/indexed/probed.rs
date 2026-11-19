//! Shared construction of candidate joins, preserving every condition and
//! restoring projected table columns to their logical positions (D35, D85).

use super::{Probing, Read};
use crate::catalog::IndexDesc;
use crate::logical::{Cond, Expr, and_all, judged_by_sets, map_cond_columns, render_cond, render_expr};
use crate::physical::shown::Shown;
use crate::physical::{JoinProbe, PhysicalPlanner, Plan};
use crate::storage::cost_of;

pub(super) struct Choice {
    pub(super) read: Probing,
    pub(super) probe: JoinProbe,
    pub(super) total: f64,
    pub(super) spatial: bool,
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
        let model = self.db.cost();
        let rows = cost_of(storage.row_count());
        let each = rows * share;
        let cost = model.index_descent * (rows + 1.0_f64).log2() + each * storage.fetch_cost();
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
        };
        Some(Choice { read: probing, probe, total: outer * cost, spatial: false })
    }
}

pub(super) fn planned(choice: Choice, outer: Plan, (cond, width): (Cond, usize), shown: Shown<'_>) -> Plan {
    let Choice { mut read, probe, total, spatial } = choice;
    let through = read.through.take();
    let mapped = |c: usize| match &through {
        Some((columns, _)) => c.checked_sub(width).and_then(|i| columns.get(i)).map_or(c, |i| i.saturating_add(width)),
        None => c,
    };
    let cond = map_cond_columns(cond, &mapped);
    let filter = read.filter.map(|c| map_cond_columns(c, &|i| i.saturating_add(width)));
    let condition = and_all(std::iter::once(cond).chain(filter).collect()).map(judged_by_sets);
    let joined_names = through
        .as_ref()
        .and_then(|(_, table)| shown.names().map(|names| names.iter().take(width).chain(table).cloned().collect::<Vec<_>>()));
    let table_shown = joined_names.as_ref().map_or(shown, |names| Shown::of(Some(names)));
    let detail = table_shown.text(|names| condition.as_ref().map(|c| render_cond(c, names)).unwrap_or_default());
    let joined = Plan::IndexJoin {
        outer: false,
        spatial,
        left: Box::new(outer),
        table: read.table,
        index: read.index,
        method: read.method,
        probe,
        cond: condition,
        width: read.width,
        columns: read.needed,
        detail,
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
