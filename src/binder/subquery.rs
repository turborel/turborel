//! [X3.135-1986 §5.24] `<subquery>`, bound: a query expression of its own,
//! whose column references are looked up in its own FROM clause first and
//! then, outward, in those of the queries that contain it, the most local
//! scope first [X3.135-1986 §5.7 SR3]. A reference found outward is an
//! "outer reference" [X3.135-1986 §5.7 SR4]: it is bound as a slot,
//! `Expr::Outer`, and the subquery keeps the expression over the enclosing
//! query's rows (or groups) that fills it (`logical/subquery.rs`).

use std::cell::RefCell;
use std::sync::Arc;

use super::collation::sort_collations;
use super::composite::anonymous;
use super::composite::field_of;
use super::cursor::{result_column, sort_keys};
use super::query::Nesting;
use super::set_functions::column_references;
use super::set_operations::bind_query_expression;
use super::{Binder, NOT_AGGREGATED, Scope, rule};
use crate::ast::{
    self, ColumnSpecification, Composite, Literal, SelectItem, SelectList, SortSpecification, TableName, ValueExpression,
    ValueSpecification,
};
use crate::error::{At, Result, SqlState};
use crate::functions::{AggregateFunction, AggregateRef, ArrayAggregate, RowOf, ScalarRef};
use crate::logical::{AggCall, Expr, LogicalPlan, Subquery, render_expr};
use crate::value::types::{Field, MAX_ARRAY_CARDINALITY};
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

impl<'a> Outer<'a> {
    /// The enclosing query's binder.
    pub(super) fn binder(&self) -> &'a Binder<'a> {
        self.binder
    }
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
pub(super) fn capture(outer: &Outer<'_>, e: Expr, name: String) -> Expr {
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
        Ok(Some((self.note(item, i)?, col.data_type.clone(), col.not_null && !item.nullable)))
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
        if let Some(field) = self.catalog_chain(cs, scope)? {
            return Ok(field);
        }
        if let Some(field) = self.field_chain(cs, scope)? {
            return Ok(field);
        }
        if let Some((pos, t, _)) = self.local(cs)? {
            return Ok((Expr::Column(self.column_in(pos, &t.clone(), scope)?), t));
        }
        let Some(outer) = self.outer else {
            // In no scope at all: the error the lookup gives.
            let (pos, t) = self.resolve(cs)?;
            return Ok((Expr::Column(self.column_in(pos, &t.clone(), scope)?), t));
        };
        let (e, t) = outer.binder.column_expr(cs, outer.scope)?;
        Ok((capture(outer, e, written(cs)), t))
    }

    /// Whether the argument `v` of a set function has an outer reference: a
    /// column of no table of this query's FROM clause.
    pub(super) fn is_outer_argument(&self, v: &ValueExpression) -> Result<bool> {
        let mut columns = Vec::new();
        column_references(v, &mut columns);
        for r in columns {
            if !self.is_local(r)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether the column reference `r` references a column of a table of
    /// this query's FROM clause: as a column, or as the basis of the chain
    /// of a column of a row type and its fields [ISO/IEC 9075-2:2023 §6.6
    /// SR9b iv 2].
    fn is_local(&self, r: &ValueExpression) -> Result<bool> {
        if let ValueExpression::Column(cs) = r {
            return Ok(self.local(cs)?.is_some() || self.chain_basis(cs).is_some());
        }
        if let ValueExpression::Composite(c, at) = r
            && let Composite::Chain(parts) = &**c
            && let (Some(first), Some(second)) = (parts.first(), parts.get(1))
        {
            return Ok(self.row_basis((first, second), *at).is_some());
        }
        Ok(false)
    }

    /// The basis of `cs` if its first two `<identifier>`s name a column
    /// of a row type of a table of this query, and the rest that column's
    /// fields [ISO/IEC 9075-2:2023 §6.6 SR9b iv 2]: of three, or of four
    /// whose first may be the catalog's name.
    fn chain_basis(&self, cs: &ColumnSpecification) -> Option<ColumnSpecification> {
        let Some(TableName { auth: Some(second), ident: third, .. }) = &cs.qualifier else { return None };
        match &cs.catalog {
            Some(first) => self.row_basis((first, second), cs.at),
            None => self.row_basis((second, third), cs.at),
        }
    }

    /// The column `first`.`second`, written at `at`, if it is a column of
    /// a row type of a table of this query.
    pub(super) fn row_basis(&self, (first, second): (&str, &str), at: usize) -> Option<ColumnSpecification> {
        let qualifier = Some(TableName { auth: None, ident: first.to_owned(), at });
        let basis = ColumnSpecification { qualifier, column: second.to_owned(), catalog: None, at };
        matches!(self.local(&basis), Ok(Some((_, DataType::Row(_), _)))).then_some(basis)
    }

    /// The column `basis` of a row type, then each of its `fields` in turn
    /// [ISO/IEC 9075-2:2023 §6.6 SR11, §6.16].
    pub(super) fn fields_of(&self, basis: &ColumnSpecification, fields: &[String], scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let bound = self.column_expr(basis, scope)?;
        fields.iter().try_fold(bound, |b, f| field_of(b, f).or_at(basis.at))
    }

    /// [ISO/IEC 9075-2:2023 §6.6 SR9b iv 2, v, SR9c] `cs` of four
    /// `<identifier>`s whose first may be the catalog's name: if instead
    /// its first two name a column of a row type of a table of this query,
    /// that column's fields the other two; none if they do not, so it is a
    /// column's. Exactly one of the two is its basis.
    fn catalog_chain(&self, cs: &ColumnSpecification, scope: Scope<'_>) -> Result<Option<(Expr, DataType)>> {
        let (Some(first), Some(TableName { auth: Some(second), ident: third, .. })) = (&cs.catalog, &cs.qualifier) else { return Ok(None) };
        let Some(basis) = self.row_basis((first, second), cs.at) else { return Ok(None) };
        if matches!(self.local(cs), Ok(Some(_))) {
            let column = &cs.column;
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §6.6 SR9c] {first}.{second}.{third}.{column} is both a column of table {second}.{third} and a field of column {first}.{second}"
            );
            return Err(rule(msg));
        }
        self.fields_of(&basis, &[third.clone(), cs.column.clone()], scope).map(Some)
    }

    /// [ISO/IEC 9075-2:2023 §6.6 SR9b iv 2, SR9c, SR11] The `<identifier
    /// chain>` `parts`, four or more, at `at`, whose first is not the
    /// catalog's name: the second a column of a row type of the table the
    /// first names, found in this query or, outward, in those containing
    /// it, the rest its fields in turn.
    pub(super) fn chain(&self, parts: &[String], at: usize, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (Some(first), Some(second)) = (parts.first(), parts.get(1)) else {
            return Err(rule("[ISO/IEC 9075-2:2023 §6.6] an identifier chain of a field reference has a column and a field"));
        };
        if let Some(basis) = self.row_basis((first, second), at) {
            return self.fields_of(&basis, parts.get(2..).unwrap_or_default(), scope);
        }
        let Some(outer) = self.outer else {
            let written = parts.join(".");
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §6.6 SR9c] {written} has no basis: no table {first} in scope has a column {second} of a row type"
            );
            // Of four, its first is not the catalog's name either [ISO/IEC
            // 9075:1992 §5.4 SR20].
            return Err(if parts.len() == 4 { rule(msg).with_state(SqlState::INVALID_CATALOG_NAME) } else { rule(msg) });
        };
        let (e, t) = outer.binder.chain(parts, at, outer.scope)?;
        Ok((capture(outer, e, parts.join(".")), t))
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
        let types = bound.columns.iter().map(|c| c.data_type.clone()).collect();
        self.note_subquery(s, &bound.columns);
        let subquery = Subquery { plan: bound.plan, outer: outer.captured.into_inner() };
        Ok((Arc::new(subquery), types))
    }

    /// [ISO/IEC 9075-2:2023 §6.49 SR3, GR3] `<array value constructor by
    /// query>`: its query of degree one \[SR3a\], aggregated as ARRAY_AGG
    /// aggregates, ordered by the `<order by clause>` it ends with, each
    /// `<sort key>` its one column [§7.17, NOTE 207]; of no rows, an empty
    /// array.
    pub(super) fn array_query(&self, s: &ast::Subquery, order_by: &[SortSpecification], scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let cx = self.context();
        let outer = Outer { binder: self, scope, captured: RefCell::new(Vec::new()) };
        let nesting = Nesting::Subquery { outer: &outer, result: None };
        let bound = bind_query_expression(cx, &s.query, &nesting).or_at(s.at)?;
        self.note_subquery(s, &bound.columns);
        let [column] = bound.columns.as_slice() else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.49 SR3a] the query of ARRAY is of degree one, not {}", bound.columns.len())));
        };
        let keys = sort_keys(order_by, &bound.columns, |cs| result_column(&bound.columns, cs).or_at(cs.at))?;
        super::composite::ordering_operand(&column.data_type, "ORDER BY")?;
        let collations = sort_collations((cx.db, cx.auth, cx.collation), order_by, &keys, &bound.columns)?;
        let element = column.data_type.clone();
        let parts = keys.len().saturating_add(1);
        let fields = (0..parts).map(|i| Field { name: anonymous(i), data_type: element.clone(), unnamed: true }).collect();
        let arg = Expr::Call(ScalarRef(Arc::new(RowOf(DataType::row(fields)))), vec![Expr::Column(0); parts]);
        let descending = keys.iter().map(|k| k.descending).collect();
        let func = ArrayAggregate { descending, collations, by_query: true };
        let result = super::composite::bounded(DataType::array(element, MAX_ARRAY_CARDINALITY))?;
        let label = format!("ARRAY({}{})", render_expr(&Expr::Column(0), &[]), func.label_suffix());
        let agg = AggCall { func: AggregateRef(Arc::new(func)), arg: Some(arg), distinct: false, result: result.clone(), label };
        let plan = LogicalPlan::Aggregate { groups: Vec::new(), collations: Vec::new(), aggs: vec![agg], input: Box::new(bound.plan) };
        let subquery = Subquery { plan, outer: outer.captured.into_inner() };
        Ok((Expr::Subquery(Arc::new(subquery), 0), result))
    }
}
