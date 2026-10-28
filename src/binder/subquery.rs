//! [X3.135-1986 §5.24] `<subquery>`, bound: a query expression of its
//! own, whose column references are looked up in its own FROM clause first
//! and then, outward, in those of the queries that contain it, the most
//! local scope first [X3.135-1986 §5.7 SR3]. A reference found outward is an
//! "outer reference" [X3.135-1986 §5.7 SR4]: it is bound as a slot,
//! `Expr::Outer`, and the subquery keeps the expression over the enclosing
//! query's rows (or groups) that fills it (`logical/subquery.rs`). Split
//! from binder.rs (RULES.md R-SIZE-2).

use std::cell::RefCell;
use std::sync::Arc;

use super::query::Nesting;
use super::set_functions::column_specifications;
use super::set_operations::bind_query_expression;
use super::{Binder, NOT_AGGREGATED, Scope, rule};
use crate::ast::{self, ColumnSpecification, Literal, SelectItem, SelectList, ValueExpression, ValueSpecification};
use crate::error::{At, Result};
use crate::functions::AggregateRef;
use crate::logical::{Expr, Subquery};
use crate::value::{DataType, Decimal};

/// The query a subquery is bound in, as its subquery sees it: the binder,
/// the scope the subquery's predicate is in, and the outer references the
/// subquery has made, in slot order.
pub(super) struct Outer<'a> {
    /// The enclosing query's binder.
    binder: &'a Binder<'a>,
    /// What its rows are where the subquery is: rows, or groups.
    scope: Scope<'a>,
    /// The expressions, over its rows, that fill the subquery's slots.
    captured: RefCell<Vec<Expr>>,
}

/// What a subquery's rows are for, which decides what its select list
/// means.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Use {
    /// Values compared, of the subquery's degree [ISO/IEC 9075:1992 §7.11].
    Rows,
    /// An `<exists predicate>`: any rows, a select list of * standing for
    /// a literal [ISO/IEC 9075:1992 §7.9 SR3a].
    Exists,
}

/// Whether two outer expressions, each a column or a slot, are the same.
fn same_slot(a: &Expr, b: &Expr) -> bool {
    match (a, b) {
        (Expr::Column(x), Expr::Column(y)) | (Expr::Outer(x, _), Expr::Outer(y, _)) => x == y,
        _ => false,
    }
}

/// [X3.135-1986 §5.8 SR5, §5.21 SR3, §5.23 GR3; ISO/IEC 9075:1992 §6.5
/// SR4, SR5, §7.8 GR3] A set function over an outer reference `v`, the
/// only column reference of its argument. Its argument source is the
/// group of the query its column is of: it is bound there, as the
/// enclosing query sees it, and its value fills a slot here. It must be
/// in that query's select list, which it makes aggregated if it is not
/// grouped (`bind_selected`), or in a subquery of its HAVING clause.
pub(super) fn outer_aggregate(outer: &Outer<'_>, func: AggregateRef, v: &ValueExpression, distinct: bool) -> Result<(Expr, DataType)> {
    if !outer.binder.is_outer_argument(v)? {
        if let Scope::Select = outer.scope {
            outer.binder.aggregated.set(true);
            return Err(rule(format!("[ISO/IEC 9075:1992 §7.9 SR7] {NOT_AGGREGATED}")));
        }
        if let Scope::Rows(_) = outer.scope {
            return Err(rule(
                "[ISO/IEC 9075:1992 §6.5 SR5; X3.135-1986 §5.8 SR5, §5.21 SR3] a set function over an outer reference is allowed only in the select list of the query of its column, or in a <subquery> of its <having clause>",
            ));
        }
    }
    let (e, t) = outer.binder.aggregate(func, Some(v), distinct, outer.scope)?;
    let label = outer_label(&e, outer.scope);
    Ok((capture(outer, e, label), t))
}

/// The display name of the enclosing query's set function `e`, bound in
/// `scope`: its call as written there, or the name of its own slot.
fn outer_label(e: &Expr, scope: Scope<'_>) -> String {
    if let Expr::Outer(_, name) = e {
        return name.clone();
    }
    let (Expr::Column(c), Scope::Groups(g, _)) = (e, scope) else { return "<set function>".to_owned() };
    g.aggs.borrow().get(c.saturating_sub(g.columns.len())).map_or_else(|| "<set function>".to_owned(), |a| a.label.clone())
}

/// The slot of the enclosing query's expression `e` in the subquery
/// `outer` gives the values of, displayed as `name`: one already made for
/// it, or a new one.
fn capture(outer: &Outer<'_>, e: Expr, name: String) -> Expr {
    let mut captured = outer.captured.borrow_mut();
    let slot = captured.iter().position(|c| same_slot(c, &e)).unwrap_or_else(|| {
        captured.push(e);
        captured.len().saturating_sub(1)
    });
    Expr::Outer(slot, name)
}

/// How a `<column specification>` was written, for display.
fn written(cs: &ColumnSpecification) -> String {
    match &cs.qualifier {
        Some(q) => format!("{}.{}", q.ident, cs.column),
        None => cs.column.clone(),
    }
}

impl Binder<'_> {
    /// [X3.135-1986 §5.7 SR3] `<column specification>` `cs` as a column of
    /// this query's FROM clause, the most local scope: its position, type
    /// and whether it is NOT NULL; or none if no table here is a possible
    /// qualifier, so it is to be looked up outward. A qualifier here whose
    /// table lacks the column, or two possible qualifiers here, is an error.
    pub(super) fn local(&self, cs: &ColumnSpecification) -> Result<Option<(usize, DataType, bool)>> {
        let here = match &cs.qualifier {
            Some(q) => self.items.iter().any(|it| it.designated_by(q, self.schema)),
            None => self.visible_has(&cs.column),
        };
        if !here {
            return Ok(None);
        }
        let (item, i, col) = match &cs.qualifier {
            Some(q) => self.qualified(q, &cs.column)?,
            None => self.unqualified(&cs.column)?,
        };
        Ok(Some((self.note(item, i)?, col.data_type, col.not_null && !item.nullable)))
    }

    /// [X3.135-1986 §5.7] `<column specification>` `cs` where `scope` says:
    /// a column of this query, or an outer reference [SR4], bound as a slot
    /// filled by the enclosing query's expression for it, which must be
    /// allowed where the subquery is: in a HAVING clause, a grouping column
    /// [X3.135-1986 §5.23 SR2].
    pub(super) fn column_expr(&self, cs: &ColumnSpecification, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        self.column_at(cs, scope).or_at(cs.at)
    }

    /// `column_expr`, its errors not yet placed.
    fn column_at(&self, cs: &ColumnSpecification, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        if let Some((pos, t, _)) = self.local(cs)? {
            return Ok((Expr::Column(self.column_in(pos, t, scope)?), t));
        }
        let Some(outer) = self.outer else {
            // In no scope at all: the error the lookup gives.
            let (pos, t) = self.resolve(cs)?;
            return Ok((Expr::Column(self.column_in(pos, t, scope)?), t));
        };
        let (e, t) = outer.binder.column_expr(cs, outer.scope)?;
        Ok((capture(outer, e, written(cs)), t))
    }

    /// Whether the argument `v` of a set function has an outer reference: a
    /// column of no table of this query's FROM clause.
    pub(super) fn is_outer_argument(&self, v: &ValueExpression) -> Result<bool> {
        let mut columns = Vec::new();
        column_specifications(v, &mut columns);
        for cs in columns {
            if self.local(cs)?.is_none() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// [X3.135-1986 §5.24; ISO/IEC 9075:1992 §7.11] `<subquery>` `s` in
    /// `scope`, used as `usage` says: bound, and the types of its columns.
    /// SQL-92 lifts SQL-86's rules that a subquery has one column
    /// [X3.135-1986 §5.24 SR2a] and, compared, no grouping [SR9]: those of
    /// Entry SQL alone [ISO/IEC 9075:1992 §7.11, Leveling Rules].
    pub(super) fn subquery(&self, s: &ast::Subquery, scope: Scope<'_>, usage: Use) -> Result<(Arc<Subquery>, Vec<DataType>)> {
        self.subquery_at(s, scope, usage).or_at(s.at)
    }

    /// `subquery`, its errors not yet placed.
    fn subquery_at(&self, s: &ast::Subquery, scope: Scope<'_>, usage: Use) -> Result<(Arc<Subquery>, Vec<DataType>)> {
        let cx = self.context();
        // [§7.9 SR3a] under EXISTS, * is an arbitrary <literal>: 1.
        let star = s.query.spec().is_some_and(|spec| matches!(spec.select_list, SelectList::Star));
        let result = (usage == Use::Exists && star).then(|| SelectItem {
            value: ValueExpression::Value(ValueSpecification::Literal(Literal::Exact(Decimal::new(1_i128, 0_u32)))),
            alias: None,
            at: s.at,
        });
        let outer = Outer { binder: self, scope, captured: RefCell::new(Vec::new()) };
        let nesting = Nesting::Subquery { outer: &outer, result };
        let bound = bind_query_expression(cx, &s.query, &nesting)?;
        let types = bound.columns.iter().map(|c| c.data_type).collect();
        let subquery = Subquery { plan: bound.plan, outer: outer.captured.into_inner() };
        Ok((Arc::new(subquery), types))
    }
}
