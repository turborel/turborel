//! [ISO/IEC 9075:1992 §11.19 GR6] What a view's columns are drawn from:
//! the leaf underlying tables of its query, and the columns of them each of
//! its columns is drawn from, which its owner's REFERENCES on it are found
//! from (`catalog::applicable`). Split from binder/views.rs (RULES.md
//! R-SIZE-2).
//!
//! It is known of a `<query specification>` whose `<from clause>` names
//! tables and views only, joined without common columns: each item of its
//! select list is drawn from the columns it references, in a subquery
//! too, so none is missed. Of a `<table value constructor>`, whose columns
//! are drawn from none [§7.10 SR16a]; and of a set operation by position
//! of operands it is known of, each column drawn from those of both
//! operands' [SR16d]. Of any other query, or one that reads a view of
//! which it is not known, it is not known, and the owner holds no
//! REFERENCES on the view.

use super::query::Nesting;
use super::set_operations::bind_query_expression;
use super::{Context, Selected, Uses, qualify};
use crate::ast::{Correspondence, JoinSpecification, QueryExpression, TableName, TableReference};
use crate::catalog::{QualifiedName, Underlying};
use crate::error::Result;

/// What the columns of the view whose query is `query`, bound with `cx`,
/// are drawn from, if it is known.
///
/// # Errors
///
/// The query does not bind.
pub fn underlying(cx: Context<'_>, query: &QueryExpression) -> Result<Option<Underlying>> {
    match query {
        QueryExpression::Spec(_) => of_spec(cx, query),
        QueryExpression::Values(_) => {
            let degree = bind_query_expression(cx, query, &Nesting::Query)?.columns.len();
            Ok(Some(Underlying { leaves: Vec::new(), columns: vec![Vec::new(); degree] }))
        }
        QueryExpression::SetOp(s) => {
            if !matches!(s.corresponding, Correspondence::Position) {
                return Ok(None);
            }
            let (Some(left), Some(right)) = (underlying(cx, &s.left)?, underlying(cx, &s.right)?) else { return Ok(None) };
            Ok(Some(merged(left, right)))
        }
    }
}

/// [ISO/IEC 9075:1992 §7.10 SR16d] What a set operation's columns are
/// drawn from, of its operands' `left` and `right`: each the i-th columns'
/// of both, its leaves both's.
fn merged(left: Underlying, right: Underlying) -> Underlying {
    let mut leaves = left.leaves;
    leaves.extend(right.leaves.into_iter().filter(|l| !leaves.contains(l)).collect::<Vec<_>>());
    let columns = left
        .columns
        .into_iter()
        .zip(right.columns)
        .map(|(mut from, also)| {
            from.extend(also.into_iter().filter(|d| !from.contains(d)).collect::<Vec<_>>());
            from
        })
        .collect();
    Underlying { leaves, columns }
}

/// What the columns of the `<query specification>` `query` are drawn
/// from, if it is known.
fn of_spec(cx: Context<'_>, query: &QueryExpression) -> Result<Option<Underlying>> {
    let Some(spec) = query.spec() else { return Ok(None) };
    let mut named = Vec::new();
    if !spec.table_expression.from.iter().all(|r| traced(r, &mut named)) {
        return Ok(None);
    }
    let (uses, selected) = (Uses::default(), Selected::default());
    let bound = bind_query_expression(Context { uses: Some(&uses), selected: Some(&selected), ..cx }, query, &Nesting::Query)?;
    let drawn = selected.into_inner();
    if drawn.len() != bound.columns.len() {
        return Ok(None);
    }
    let mut leaves = Vec::new();
    for t in named.iter().map(|t| qualify(t, cx.schema)) {
        let Some(of) = leaves_of(cx, &t) else { return Ok(None) };
        leaves.extend(of.into_iter().filter(|l| !leaves.contains(l)).collect::<Vec<_>>());
    }
    let mut columns = Vec::with_capacity(drawn.len());
    for item in drawn {
        let mut from = Vec::new();
        for (t, c) in item {
            let Some(each) = drawn_from(cx, &t, &c) else { return Ok(None) };
            from.extend(each.into_iter().filter(|d| !from.contains(d)).collect::<Vec<_>>());
        }
        columns.push(from);
    }
    Ok(Some(Underlying { leaves, columns }))
}

/// Whether `r` names tables and views only, joined without common columns:
/// those it names added to `named`.
fn traced<'a>(r: &'a TableReference, named: &mut Vec<&'a TableName>) -> bool {
    match r {
        TableReference::Table(t) => {
            named.push(&t.table);
            true
        }
        TableReference::Derived(_) => false,
        TableReference::Joined(j) => {
            let common = j.natural || matches!(j.on, Some(JoinSpecification::Using(_)));
            !common && traced(&j.left, named) && traced(&j.right, named)
        }
    }
}

/// The leaf underlying tables of `t`: itself, a base table, or those of a
/// view, if they are known.
fn leaves_of(cx: Context<'_>, t: &QualifiedName) -> Option<Vec<QualifiedName>> {
    if cx.db.catalog.table(t).is_some() {
        return Some(vec![t.clone()]);
    }
    cx.db.catalog.view(t)?.underlying.as_ref().map(|u| u.leaves.clone())
}

/// The columns of leaf underlying tables column `c` of `t` is drawn from:
/// itself, of a base table, or those a view's is drawn from.
fn drawn_from(cx: Context<'_>, t: &QualifiedName, c: &str) -> Option<Vec<(QualifiedName, String)>> {
    if cx.db.catalog.table(t).is_some() {
        return Some(vec![(t.clone(), c.to_owned())]);
    }
    let view = cx.db.catalog.view(t)?;
    let at = view.columns.iter().position(|column| column.name == c)?;
    view.underlying.as_ref()?.columns.get(at).cloned()
}
