//! [ISO/IEC 9075-2:2023 §14.12 SR11, SR12, GR5] The source of a MERGE:
//! its `<table reference>`, no joined table, exposing a name other than
//! the target's, and the plan of its rows, the table Q.

use std::collections::BTreeSet;

use super::super::from::{Bounds, From, FromItem, bind_from, from_plan};
use super::super::{Binder, Context, rule};
use super::target::CLAUSE;
use crate::ast::TableReference;
use crate::error::Result;
use crate::logical::LogicalPlan;

/// [ISO/IEC 9075-2:2023 §7.6 AR1b ii 1 B] SELECT on at least one column of
/// each table the source names.
const SOURCE_PRIVILEGE: &str = "ISO/IEC 9075-2:2023 §7.6 AR1b ii 1 B";

/// The source, bound.
pub(super) struct Source {
    /// Its FROM items, each placed after the target's columns, as the rows
    /// the statement's conditions are bound over have them.
    pub(super) items: Vec<FromItem>,
    /// The columns of its result, each an item of `items` and a column of
    /// it (`From::visible_of`).
    pub(super) visible: Vec<(usize, usize)>,
    /// The width of its rows.
    pub(super) width: usize,
    /// The display names of its columns.
    pub(super) names: Vec<String>,
    /// [GR5] The plan of its rows, Q, every column of them read.
    pub(super) plan: LogicalPlan,
}

/// [SR11, SR12, GR5] The source `tr` of a MERGE whose target is `target`,
/// `target_width` columns wide: no `<joined table>` [SR11], exposing no
/// name equivalent to the target's [SR12], its tables read as SELECT on
/// them allows.
pub(super) fn bind_source(cx: Context<'_>, tr: &TableReference, (target, target_width): (&FromItem, usize)) -> Result<Source> {
    if let TableReference::Joined(j) = tr {
        return Err(rule(format!("[{CLAUSE} SR11] the <table reference> of a MERGE is no <joined table>")).or_at(j.at));
    }
    let From { items, trees, .. } = bind_from(Bounds { cx, clause: SOURCE_PRIVILEGE, outer: None }, std::slice::from_ref(tr))?;
    if let Some(same) = items.iter().find(|it| same_exposed(target, it)) {
        let msg = format!("[{CLAUSE} SR12] the source exposes {}, the name the target is exposed by", same.designator());
        return Err(rule(msg).or_at(same.at));
    }
    let width = items.iter().map(FromItem::width).sum();
    let visible = From::visible_of(&trees, &items);
    let names: Vec<String> = items.iter().flat_map(FromItem::row_names).collect();
    // Every column of a source row is in the rows bound over, and the
    // insert values may read any of them.
    let reader = Binder::new(cx, None, (items.clone(), names.clone(), visible.clone()), (0..width).collect::<BTreeSet<usize>>());
    let plan = from_plan(&reader, &trees)?;
    let items = items.into_iter().map(|it| FromItem { offset: it.offset.saturating_add(target_width), ..it }).collect();
    Ok(Source { items, visible, width, names, plan })
}

/// [SR12] Whether `source` exposes a name equivalent to the one `target`
/// is exposed by: the same table name, the same correlation name, or a
/// correlation name equal to the other's table identifier, as two table
/// references of one FROM clause would [X3.135-1986 §5.20 SR2, SR3].
fn same_exposed(target: &FromItem, source: &FromItem) -> bool {
    match (&target.correlation, &source.correlation) {
        (None, None) => target.table == source.table,
        (Some(x), Some(y)) => x == y,
        (Some(x), None) => *x == source.table.ident,
        (None, Some(y)) => *y == target.table.ident,
    }
}
