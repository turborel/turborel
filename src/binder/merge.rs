//! [ISO/IEC 9075-2:2023 §14.12] A `<merge statement>`, bound: its target
//! table, the rows of its source each joined to the target rows it
//! matches, and its WHEN clauses, each bound over those rows.

use std::collections::BTreeSet;

use crate::ast::MergeStatement;
use crate::catalog::{QualifiedName, TableDesc};
use crate::error::Result;
use crate::exec::TransitionRows;
use crate::logical::{Cond, Expr, LogicalPlan};

use super::{Binder, Context, ObjectColumn, ViewCheck};

mod clauses;
mod rows;
mod source;
mod target;
#[cfg(test)]
mod tests;

/// [ISO/IEC 9075-2:2023 §14.12] A MERGE, bound.
///
/// Each row of `rows` is laid out the same, and every condition and
/// expression of it is bound over that layout:
///
/// - the base table's columns, `desc.columns.len()` of them, of a target
///   row a source row matches, or every one null;
/// - the source's columns, `source_width` of them;
/// - for each WHEN NOT MATCHED clause of a search condition, in order,
///   its flag (`BoundWhen::NotMatched`);
/// - the target row's row id (`storage::located`), null of a source row
///   no target row matches.
///
/// The conditions and expressions read only the first two parts.
#[derive(Debug, Clone)]
pub struct BoundMerge {
    /// The table or view the statement names.
    pub named: QualifiedName,
    /// The base table: it, or the base table of the updatable view
    /// [X3.135-1986 §6.5 GR2; ISO/IEC 9075-2:2023 §14.12 SR7].
    pub table: QualifiedName,
    /// Its description.
    pub desc: TableDesc,
    /// The width of the source's rows.
    pub source_width: usize,
    /// \[GR5\] Each source row joined to each target row the ON condition
    /// and the views' conditions match, or alone if none does, laid out as
    /// above: the source LEFT JOIN the target, read before any row is
    /// changed.
    pub rows: LogicalPlan,
    /// The ON condition and the views' conditions, over a row's first two
    /// parts: a target row changed since it was read is checked again by
    /// it (RULES.md D29).
    pub on: Cond,
    /// The `<merge when clause>`s, in order.
    pub clauses: Vec<BoundWhen>,
    /// [ISO/IEC 9075:1992 §11.19 GR11; ISO/IEC 9075-2:2023 §14.12 GR7] The
    /// views WITH CHECK OPTION it spans, which every row it updates or
    /// inserts is checked against.
    pub checks: Vec<ViewCheck>,
    /// The authorization identifier it runs under: CURRENT_USER.
    pub auth: String,
    /// [ISO/IEC 9075-2:2023 §15.14 GR9] Each generated column of the base
    /// table, by its position, its generation expression bound over its
    /// row.
    pub generated: Vec<(usize, Expr)>,
}

/// [ISO/IEC 9075-2:2023 §14.12] A `<merge when clause>`, bound.
#[derive(Debug, Clone)]
pub enum BoundWhen {
    /// WHEN MATCHED: its search condition, if written, and what it does to
    /// its subject rows [GR6a i].
    Matched {
        /// The `<search condition>` after AND, if written.
        condition: Option<Cond>,
        /// Its update or delete.
        action: BoundAction,
    },
    /// WHEN NOT MATCHED [GR6a ii]: the source rows it inserts a row for
    /// and the values of their object columns.
    NotMatched {
        /// Of a clause of a search condition SC, the position in a row of
        /// its flag: true of a source row for which no target row makes
        /// `ON OR NOT SC` true [GR6a ii 1 to 3]. Of a clause of none, none:
        /// it applies to the source rows no target row matches.
        flag: Option<usize>,
        /// The object columns, as columns of the base table, in the order
        /// of the `<insert column list>`, written or implicit \[SR18a\].
        object: Vec<usize>,
        /// The value of each object column, in that order, over a row's
        /// source columns [SR9, GR6a ii 4]: NULL a null constant, DEFAULT
        /// the column's default or its generator's next value, as an
        /// INSERT's VALUES binds them (`Binder::values_row`).
        values: Vec<Expr>,
    },
}

/// [ISO/IEC 9075-2:2023 §14.12] A `<merge update or delete specification>`,
/// bound.
#[derive(Debug, Clone)]
pub enum BoundAction {
    /// UPDATE: each object column's position in the base table's row and
    /// its new value (`None` is NULL), in the order written [GR6a i 1 B
    /// III].
    Update(Vec<(usize, Option<Expr>)>),
    /// DELETE [GR6a i 7].
    Delete,
}

/// [ISO/IEC 9075-2:2023 §14.12] `stmt`, bound; `inserts` the object
/// columns of each of its WHEN NOT MATCHED clauses, in order, as the
/// INSERT each is binds them (`ObjectColumn`), each a column of the base
/// table.
///
/// The target's exposed name is in scope in the ON condition, each WHEN
/// condition and each SET clause \[SR9\], the source's there and in each
/// merge insert value list too [§7.6 SR20b]: those are bound over the
/// source's columns alone.
///
/// Of a view whose INSTEAD OF triggers take every WHEN clause, `held` its
/// rows, each followed by its ordinal (`storage::located`): T is the view
/// itself, its rows those, each ordinal its row id [ISO/IEC 9075-2:2023
/// §15.10 GR3a, §15.13 GR3a, §15.16 GR3a]; `inserts` are then columns of
/// the view.
///
/// # Errors
///
/// A rule of §14.12, or of what it reads, is broken (SQLCODE -103 and
/// others), or a privilege it needs is not held (SQLCODE -104).
pub fn bind_merge(
    cx: Context<'_>,
    stmt: &MergeStatement,
    inserts: &[Vec<ObjectColumn>],
    held: Option<&TransitionRows>,
) -> Result<BoundMerge> {
    let target::Target { name, write, item } = target::bind_target(cx, stmt, held.is_some())?;
    let tw = write.base.columns.len();
    let src = source::bind_source(cx, &stmt.source, (&item, tw))?;
    let names: Vec<String> = item.row_names().into_iter().chain(src.names.iter().cloned()).collect();
    // The target is item 0, so each source item is one further on.
    let visible =
        (0..write.named.columns.len()).map(|k| (0, k)).chain(src.visible.iter().map(|&(i, k)| (i.saturating_add(1), k))).collect();
    let items = std::iter::once(item).chain(src.items.iter().cloned()).collect();
    let both = Binder::new(cx, None, (items, names.clone(), visible), BTreeSet::new());
    let alone = Binder::new(cx, None, (src.items.clone(), names.clone(), src.visible.clone()), BTreeSet::new());
    // [SR19b] SC1, T being no system-versioned table.
    let sc1 = clauses::condition(&both, &stmt.on)?;
    let scopes = clauses::Scopes { both: &both, source: &alone, base: &write.base, flags: tw.saturating_add(src.width) };
    let (bound, flagged) = clauses::bind_clauses(&scopes, &stmt.clauses, inserts)?;
    let sides =
        rows::Sides { base: &write.base, names: &names, view: write.cond.as_ref(), target_width: tw, source_width: src.width, held };
    let flags = flagged.into_iter().map(|scj| rows::flag(&sides, &sc1, scj)).collect();
    let rows = rows::joined(&sides, src.plan, flags, &sc1);
    // A target row changed since it was read is matched again by the ON
    // condition and the views' conditions (RULES.md D29).
    let on = match write.cond {
        Some(view) => Cond::And(Box::new(sc1), Box::new(view)),
        None => sc1,
    };
    let generated = super::generations(cx.db, &write.base)?;
    Ok(BoundMerge {
        named: name,
        table: write.base.name.clone(),
        desc: write.base,
        source_width: src.width,
        rows,
        on,
        clauses: bound,
        checks: write.checks,
        auth: cx.auth.to_owned(),
        generated,
    })
}
