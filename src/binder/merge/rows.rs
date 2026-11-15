//! [ISO/IEC 9075-2:2023 §14.12 GR5, GR6a ii 1 to 3] The rows a MERGE
//! reads before it changes any: each source row, with the flag of each
//! WHEN NOT MATCHED clause of a search condition, LEFT JOIN the target
//! rows the ON condition matches, each with its row id, laid out as
//! `BoundMerge` says.

use std::sync::Arc;

use crate::catalog::TableDesc;
use crate::exec::TransitionRows;
use crate::logical::{Cond, Expr, LogicalPlan, OuterJoin, Subquery, map_cond_columns};
use crate::storage::ROW_ID;

/// The two sides of the rows bound over: the target's base table columns,
/// then the source's.
pub(super) struct Sides<'b> {
    /// The base table changed.
    pub(super) base: &'b TableDesc,
    /// The display names of the columns of both sides, in order.
    pub(super) names: &'b [String],
    /// The views' conditions, over the base table's rows: of the rows of T
    /// read as the view is [SR7].
    pub(super) view: Option<&'b Cond>,
    /// The width of the target's side: the base table's columns.
    pub(super) target_width: usize,
    /// The width of the source's side.
    pub(super) source_width: usize,
    /// Of a view whose INSTEAD OF triggers take every WHEN clause, its
    /// rows, each followed by its ordinal as its row id: T's rows, in
    /// place of a scan of a base table.
    pub(super) held: Option<&'b TransitionRows>,
}

impl Sides<'_> {
    /// The rows of T, as the view is read, every column of them; with each
    /// row's id after its columns, if `located` (`storage::located`). Of
    /// rows held, those rows.
    fn target(&self, located: bool) -> LogicalPlan {
        let mut names: Vec<String> = self.names.get(..self.target_width).unwrap_or_default().to_vec();
        if let Some(rows) = self.held {
            names.push(ROW_ID.to_owned());
            let all = LogicalPlan::Transition { rows: rows.clone(), names };
            if located {
                return all;
            }
            let exprs = (0..self.target_width).map(Expr::Column).collect();
            return LogicalPlan::Project { exprs, input: Box::new(all) };
        }
        if located {
            names.push(ROW_ID.to_owned());
        }
        let needed = (0..self.target_width).collect();
        let scan = LogicalPlan::Scan { table: self.base.name.clone(), names, needed, located };
        match self.view {
            Some(cond) => LogicalPlan::Filter { cond: cond.clone(), input: Box::new(scan) },
            None => scan,
        }
    }

    /// `e`, over both sides, as an expression of a subquery over the
    /// target's rows applied to the source's: each column of the source a
    /// slot, the outer expression of the same number its column of a
    /// source row. Its recursion is as deep as `e`, which the parser's
    /// nesting bounds made (RULES.md R-CF-6).
    fn correlated(&self, e: Expr) -> Expr {
        if let Expr::Column(p) = e {
            return match p.checked_sub(self.target_width) {
                Some(k) => Expr::Outer(k, self.names.get(p).cloned().unwrap_or_default()),
                None => Expr::Column(p),
            };
        }
        e.map_children(&mut |child| self.correlated(child))
    }
}

/// [GR6a ii 1 to 3] The flag of a WHEN NOT MATCHED clause of the search
/// condition `scj`, ON condition `sc1`, both over both sides: true of a
/// source row R2 for which no row R1 of T makes `SC1 OR NOT SCj` true,
/// that is, a row of Q not in S2, of S3. It is the General Rules as
/// printed, `NOT EXISTS (SELECT * FROM TR1 MCN WHERE (SC1) OR NOT (SCj))`
/// for each source row, T read as the view is, before any row changes;
/// over a projection of the source's rows.
pub(super) fn flag(sides: &Sides<'_>, sc1: &Cond, scj: Cond) -> Expr {
    let either = Cond::Or(Box::new(sc1.clone()), Box::new(Cond::Not(Box::new(scj))));
    let cond = either.map_exprs(&mut |e| sides.correlated(e));
    let plan = LogicalPlan::Filter { cond, input: Box::new(sides.target(false)) };
    let outer = (0..sides.source_width).map(Expr::Column).collect();
    Expr::Truth(Box::new(Cond::Not(Box::new(Cond::Exists(Arc::new(Subquery { plan, outer }))))))
}

/// [GR5] The rows of a MERGE: each row of the source's plan `source`,
/// followed by `flags`, LEFT JOIN the rows of T, each followed by its row
/// id, that the ON condition `sc1`, over both sides, matches; so a source
/// row matching none is kept, and the target's index can be probed for
/// each. Projected to `BoundMerge`'s layout: the target's columns, the
/// source's, the flags, the row id.
pub(super) fn joined(sides: &Sides<'_>, source: LogicalPlan, flags: Vec<Expr>, sc1: &Cond) -> LogicalPlan {
    let (tw, sw, nf) = (sides.target_width, sides.source_width, flags.len());
    let ahead = sw.saturating_add(nf);
    let projected = (0..sw).map(Expr::Column).chain(flags).collect();
    let left = Box::new(LogicalPlan::Project { exprs: projected, input: Box::new(source) });
    // From [target, source] to the join's [source, flags, target, row id].
    let cond = map_cond_columns(sc1.clone(), &|p| match p.checked_sub(tw) {
        Some(k) => k,
        None => p.saturating_add(ahead),
    });
    let join = LogicalPlan::OuterJoin { kind: OuterJoin::Left, cond, left, right: Box::new(sides.target(true)) };
    let target = (0..tw).map(|k| Expr::Column(k.saturating_add(ahead)));
    let flagged = (sw..ahead).map(Expr::Column);
    let row_id = Expr::Column(ahead.saturating_add(tw));
    let exprs = target.chain((0..sw).map(Expr::Column)).chain(flagged).chain(std::iter::once(row_id)).collect();
    LogicalPlan::Project { exprs, input: Box::new(join) }
}
