//! [ISO/IEC 9075-2:2023 §7.17 SR26, §10.10] The `<sort key>`s of an ORDER
//! BY. A key is a position or a name of a column of the result [X3.135-1986
//! §8.3 SR6], or any `<value expression>`. Of a simple table query, one
//! that reads a column of the table expression the result has not is
//! computed with the select list, a column of the sort table taken away
//! after the sort \[SR26 d i 10\]; one alike an item of the select list
//! sorts by that item's column \[SR26 d i 6\]; one of the result's columns
//! alone is computed of the result's rows \[SR26 d ii\]. Of any other
//! query expression each key is of the result's columns alone.

use super::cursor::result_column;
use super::from::{FromItem, column_of};
use super::query::{Nesting, bind_ordered};
use super::set_functions::column_references;
use super::set_operations::bind_query_expression;
use super::views::{Source, unconstrained};
use super::window::has_window_function;
use super::with::{site_of, with_scope};
use super::{Binder, Bound, Context, OutColumn, Scope, rule};
use crate::ast::{ColumnSpecification, QueryExpression, SelectItem, SortColumn, SortSpecification, ValueExpression, WindowOver};
use crate::catalog::QualifiedName;
use crate::error::{At, Code, Result, SqlError};
use crate::logical::{Expr, LogicalPlan, SortKey};
use crate::value::SortOrder;
use crate::value::collation::CollationRef;

/// A query expression's rows to be sorted: `plan`, of the columns `all`,
/// the first `visible` of them the result's and the rest computed for the
/// sort, and the keys that sort them, each by its collation if it has one.
pub(super) struct Ordered {
    pub(super) plan: LogicalPlan,
    pub(super) all: Vec<OutColumn>,
    pub(super) visible: usize,
    pub(super) keys: Vec<SortKey>,
    pub(super) collations: Vec<Option<CollationRef>>,
}

impl Ordered {
    /// Of `bound`, not sorted.
    pub(super) fn unsorted(bound: Bound) -> Ordered {
        let visible = bound.columns.len();
        Ordered { plan: bound.plan, all: bound.columns, visible, keys: Vec::new(), collations: Vec::new() }
    }

    /// [X3.135-1986 §8.3 GR3b] Its rows sorted, the sort keys of a
    /// collation made columns of the rows sorted, and every column but the
    /// result's taken away after [§7.17 GR5].
    pub(super) fn sorted(self) -> Bound {
        let Ordered { plan, mut all, visible, keys, collations } = self;
        let width = all.len();
        all.truncate(visible);
        if keys.is_empty() {
            return Bound { plan, columns: all };
        }
        let mut exprs: Vec<Expr> = (0..width).map(Expr::Column).collect();
        let mut sorted = Vec::with_capacity(keys.len());
        for (key, collation) in keys.into_iter().zip(&collations) {
            sorted.push(match collation {
                Some(c) => {
                    exprs.push(Expr::sort_key(Expr::Column(key.column), c));
                    SortKey { column: exprs.len().saturating_sub(1), ..key }
                }
                None => key,
            });
        }
        let keyed = if exprs.len() > width { LogicalPlan::Project { exprs: exprs.clone(), input: Box::new(plan) } } else { plan };
        let rows = LogicalPlan::Sort { keys: sorted, input: Box::new(keyed) };
        if exprs.len() == visible {
            return Bound { plan: rows, columns: all };
        }
        let kept = (0..visible).map(Expr::Column).collect();
        Bound { plan: LogicalPlan::Project { exprs: kept, input: Box::new(rows) }, columns: all }
    }

    /// Its rows, each with `extra`'s values of it after its columns.
    fn appending(&mut self, extra: Vec<(Expr, OutColumn)>) {
        if extra.is_empty() {
            return;
        }
        let mut exprs: Vec<Expr> = (0..self.all.len()).map(Expr::Column).collect();
        for (e, column) in extra {
            exprs.push(e);
            self.all.push(column);
        }
        let input = std::mem::replace(&mut self.plan, LogicalPlan::Values { rows: Vec::new(), types: Vec::new() });
        self.plan = LogicalPlan::Project { exprs, input: Box::new(input) };
    }
}

/// [ISO/IEC 9075-2:2023 §7.17 SR26b, §6.14 SR1] A `<sort key>` `v` that is
/// not a column's name or position has a column reference, and no query
/// expression, set function or next value expression.
fn key_rules(v: &ValueExpression) -> Result<()> {
    if v.any(&|e| matches!(e, ValueExpression::Subquery(_))) || has_subquery(v) {
        return Err(rule("[ISO/IEC 9075-2:2023 §7.17 SR26b] a sort key contains no query expression"));
    }
    if v.any(&|e| matches!(e, ValueExpression::SetFunction(_))) {
        return Err(rule(
            "[ISO/IEC 9075-2:2023 §7.17 SR26b] a sort key contains no set function; name it in the select list, and sort by that column",
        ));
    }
    if v.any(&|e| matches!(e, ValueExpression::NextValue { .. })) {
        return Err(rule("[ISO/IEC 9075-2:2023 §6.14 SR1] a next value expression is in no sort key"));
    }
    let mut refs = Vec::new();
    column_references(v, &mut refs);
    if refs.is_empty() && !v.any(&partitioned) {
        return Err(rule("[ISO/IEC 9075-2:2023 §7.17 SR26b] a sort key contains a column reference"));
    }
    Ok(())
}

/// Whether `e` is a window function of a window partitioned by columns,
/// column references `column_references` does not walk.
fn partitioned(e: &ValueExpression) -> bool {
    let ValueExpression::Window(w) = e else { return false };
    match &w.over {
        WindowOver::Specification(s) => !s.partition.is_empty(),
        WindowOver::Name(..) => false,
    }
}

/// Whether `v` has a subquery anywhere, as of a CASE's condition or a
/// predicate's.
fn has_subquery(v: &ValueExpression) -> bool {
    let mut found = false;
    v.for_each_subquery(&mut |_| found = true);
    found
}

/// The result's column `name` names, if one alone has that name.
fn named(columns: &[OutColumn], name: &str) -> Option<usize> {
    let mut found = columns.iter().enumerate().filter(|(_, c)| c.name.as_deref() == Some(name)).map(|(i, _)| i);
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// Whether the column reference `r` names a column of the result, unqualified.
fn names_result(r: &ValueExpression, columns: &[OutColumn]) -> bool {
    let ValueExpression::Column(cs) = r else { return false };
    cs.qualifier.is_none() && named(columns, &cs.column).is_some()
}

/// Whether every column reference of `v` names a column of the result.
fn of_result_alone(v: &ValueExpression, columns: &[OutColumn]) -> bool {
    let mut refs = Vec::new();
    column_references(v, &mut refs);
    refs.iter().all(|r| names_result(r, columns))
}

/// Where a sort key of a simple table query is.
enum Placed {
    /// A column of the result.
    Column(usize),
    /// Past the result's columns, the `n`th select item added for the sort.
    Hidden(usize),
    /// \[SR26 d ii\] Of the result's columns alone, computed of its rows.
    Result(ValueExpression),
}

/// The sort keys of a simple table query, placed, and the select items
/// added for them, which are bound with its select list \[SR26 d i 10\].
pub(super) struct Keys {
    placed: Vec<Placed>,
    pub(super) hidden: Vec<SelectItem>,
}

impl Keys {
    /// The keys of `order_by`, of the query `binder` binds, whose select
    /// list `selected` gives `columns`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a key breaks a rule above, or names no column.
    pub(super) fn of(binder: &Binder<'_>, order_by: &[SortSpecification], selected: &[SelectItem], columns: &[OutColumn]) -> Result<Keys> {
        let mut keys = Keys { placed: Vec::new(), hidden: Vec::new() };
        for s in order_by {
            let placed = match &s.key {
                SortColumn::Position(n) => Placed::Column(position(*n, columns.len())?),
                SortColumn::Column(cs) => match result_named(binder, columns, cs).or_at(cs.at)? {
                    Some(i) => Placed::Column(i),
                    None => keys.hide(ValueExpression::Column(cs.clone()), s.at),
                },
                SortColumn::Value(v) => {
                    key_rules(v).or_at(s.at)?;
                    if has_window_function(v) || !of_result_alone(v, columns) {
                        keys.hide(v.clone(), s.at)
                    } else {
                        Placed::Result(v.clone())
                    }
                }
            };
            keys.placed.push(placed);
        }
        // [SR26 d i 6] Of a result's column named, the select item's value.
        let names = columns.iter().zip(selected).filter_map(|(c, item)| {
            let name = c.name.as_deref()?;
            named(columns, name).map(|_| (name.to_owned(), item.value.clone()))
        });
        binder.sort_names.borrow_mut().names = names.collect();
        Ok(keys)
    }

    /// A key `v`, written at `at`, a select item added for the sort.
    fn hide(&mut self, v: ValueExpression, at: usize) -> Placed {
        self.hidden.push(SelectItem { value: v, alias: None, at });
        Placed::Hidden(self.hidden.len().saturating_sub(1))
    }

    /// The query's rows, `exprs` of the select list and the items added for
    /// the sort, `columns`, the first `visible` of them its own, made of
    /// `plan`: ordered by the keys, each key alike an item of the select
    /// list sorting by its column and the items not needed taken away.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a key is not of a type a sort takes, or a key of the
    /// result's columns breaks a rule binding it.
    pub(super) fn ordered(
        self,
        (exprs, mut columns, visible): (Vec<Expr>, Vec<OutColumn>, usize),
        plan: impl FnOnce(Vec<Expr>, &[OutColumn]) -> Result<LogicalPlan>,
    ) -> Result<(Ordered, Vec<SortKeyPlace>)> {
        let (own, added) = exprs.split_at(visible.min(exprs.len()));
        let mut kept: Vec<usize> = Vec::new();
        let mut places = Vec::with_capacity(self.placed.len());
        for p in self.placed {
            places.push(match p {
                Placed::Column(i) => SortKeyPlace::Column(i),
                Placed::Hidden(n) => match added.get(n).and_then(|a| own.iter().position(|o| alike(o, a))) {
                    Some(i) => SortKeyPlace::Column(i),
                    None => SortKeyPlace::Column(visible.saturating_add(keep(&mut kept, n))),
                },
                Placed::Result(v) => SortKeyPlace::Result(v),
            });
        }
        let mut exprs = exprs.clone();
        let hidden: Vec<Expr> = kept.iter().filter_map(|&n| exprs.get(visible.saturating_add(n)).cloned()).collect();
        let hidden_columns: Vec<OutColumn> = kept.iter().filter_map(|&n| columns.get(visible.saturating_add(n)).cloned()).collect();
        exprs.truncate(visible);
        exprs.extend(hidden);
        columns.truncate(visible);
        columns.extend(hidden_columns);
        let plan = plan(exprs, &columns)?;
        Ok((Ordered { plan, all: columns, visible, keys: Vec::new(), collations: Vec::new() }, places))
    }
}

/// [ISO/IEC 9075-2:2023 §7.17 SR26 d i 6] Whether `a` and `b` are alike:
/// the same operations on the same columns and values, so of every row the
/// same value. Any part this cannot tell of, a CASE, a predicate, a
/// subquery, a parameter or a function that is not the same one, is not.
fn alike(a: &Expr, b: &Expr) -> bool {
    match (a, b) {
        (Expr::Column(left), Expr::Column(right)) | (Expr::Outer(left, _), Expr::Outer(right, _)) => left == right,
        (Expr::Const(left), Expr::Const(right)) => format!("{left:?}") == format!("{right:?}"),
        (Expr::Neg(left), Expr::Neg(right)) => alike(left, right),
        (Expr::Arith(l1, op1, l2), Expr::Arith(r1, op2, r2)) => op1 == op2 && alike(l1, r1) && alike(l2, r2),
        (Expr::Call(f1, left), Expr::Call(f2, right)) => {
            std::sync::Arc::ptr_eq(&f1.0, &f2.0) && left.len() == right.len() && left.iter().zip(right).all(|(l, r)| alike(l, r))
        }
        (Expr::Cast(left, t1), Expr::Cast(right, t2)) => t1 == t2 && alike(left, right),
        (Expr::Domain(left, d1), Expr::Domain(right, d2)) => std::sync::Arc::ptr_eq(d1, d2) && alike(left, right),
        (Expr::SortKey(left, c1), Expr::SortKey(right, c2)) => std::sync::Arc::ptr_eq(&c1.collation, &c2.collation) && alike(left, right),
        (
            Expr::Column(_)
            | Expr::Outer(..)
            | Expr::Const(_)
            | Expr::Neg(_)
            | Expr::Arith(..)
            | Expr::Call(..)
            | Expr::Cast(..)
            | Expr::Domain(..)
            | Expr::SortKey(..)
            | Expr::Dynamic(_)
            | Expr::Case(..)
            | Expr::Subquery(..)
            | Expr::Truth(_)
            | Expr::NextValue(_),
            _,
        ) => false,
    }
}

/// The place among the kept added items of the `n`th, kept now if not yet.
fn keep(kept: &mut Vec<usize>, n: usize) -> usize {
    if let Some(k) = kept.iter().position(|&m| m == n) {
        return k;
    }
    kept.push(n);
    kept.len().saturating_sub(1)
}

/// Where a sort key is, its query bound.
pub(super) enum SortKeyPlace {
    /// A column of the rows sorted.
    Column(usize),
    /// \[SR26 d ii\] Of the result's columns alone.
    Result(ValueExpression),
}

/// [X3.135-1986 §8.3 SR6] The column at position `n`, from 1, of `width`.
fn position(n: u32, width: usize) -> Result<usize> {
    usize::try_from(n)
        .ok()
        .and_then(|n| n.checked_sub(1))
        .filter(|&i| i < width)
        .ok_or_else(|| rule(format!("[X3.135-1986 §8.3 SR6] ORDER BY {n} must be between 1 and {width}")))
}

/// [X3.135-1986 §8.3 SR6] The result column the `<column specification>`
/// `cs` names, if one does. Qualified, it is the one of that column of the
/// FROM clause, and unqualified the one of that name.
///
/// # Errors
///
/// SQLCODE -103 if it names more than one, or qualified names no column.
fn result_named(binder: &Binder<'_>, columns: &[OutColumn], cs: &ColumnSpecification) -> Result<Option<usize>> {
    let matches: Vec<usize> = match &cs.qualifier {
        Some(_) => {
            let (pos, _) = binder.resolve(cs)?;
            columns.iter().enumerate().filter(|(_, c)| c.origin == Some(pos)).map(|(i, _)| i).collect()
        }
        None => columns.iter().enumerate().filter(|(_, c)| c.name.as_deref() == Some(&cs.column)).map(|(i, _)| i).collect(),
    };
    match matches.as_slice() {
        [] => Ok(None),
        [i] => Ok(Some(*i)),
        [_, _, ..] => Err(rule(format!("[X3.135-1986 §8.3 SR6] ORDER BY {} matches more than one result column", cs.column))),
    }
}

/// The query expression `q`, bound as `nesting` says, and the keys of
/// `order_by` that sort it: of a simple table query, one after a `<with
/// clause>` too, in its scope, keys of its table expression's columns; of
/// any other, of its result's \[SR26 d i 1\]. Its select list draws values
/// if `draws` [§6.14 SR1a ii].
///
/// # Errors
///
/// Any Syntax Rule the query or a key breaks, or a missing privilege.
pub(super) fn ordered_query(
    cx: Context<'_>,
    q: &QueryExpression,
    (order_by, nesting): (&[SortSpecification], &Nesting<'_>),
    draws: bool,
) -> Result<Ordered> {
    match q {
        QueryExpression::Spec(spec) => bind_ordered(Context { draws, ..cx }, spec, order_by, nesting),
        QueryExpression::With(w) => {
            with_scope(cx, w, (site_of(nesting), false), |inner| ordered_query(inner, &w.body, (order_by, nesting), draws))
        }
        QueryExpression::Values(_) | QueryExpression::SetOp(_) => of_result(cx, bind_query_expression(cx, q, nesting)?, order_by),
    }
}

/// [ISO/IEC 9075-2:2023 §7.17 SR26 d ii] The sort keys of `order_by` of a
/// query expression that is not a simple table query, bound as `cx`, of
/// the columns of its result alone: `bound` with each key that is no column
/// of it computed of its rows.
///
/// # Errors
///
/// SQLCODE -103 if a key names no column of the result, breaks a rule
/// above, or is of a type a sort cannot take.
pub(super) fn of_result(cx: Context<'_>, bound: Bound, order_by: &[SortSpecification]) -> Result<Ordered> {
    let mut places = Vec::with_capacity(order_by.len());
    for s in order_by {
        places.push(match &s.key {
            SortColumn::Position(n) => SortKeyPlace::Column(position(*n, bound.columns.len())?),
            SortColumn::Column(cs) => SortKeyPlace::Column(result_column(&bound.columns, cs).or_at(cs.at)?),
            SortColumn::Value(v) => {
                key_rules(v).or_at(s.at)?;
                if has_window_function(v) {
                    return Err(
                        rule("[ISO/IEC 9075-2:2023 §6.10 SR2a] a window function in ORDER BY is of a simple table query").or_at(s.at)
                    );
                }
                if !of_result_alone(v, &bound.columns) {
                    return Err(rule(
                        "[ISO/IEC 9075-2:2023 §7.17 SR26 d i 1] ORDER BY of a column not of the result is of a simple table query alone",
                    )
                    .or_at(s.at));
                }
                SortKeyPlace::Result(v.clone())
            }
        });
    }
    let mut ordered = Ordered::unsorted(bound);
    ordered.keyed(cx, order_by, places)?;
    Ok(ordered)
}

impl Ordered {
    /// Sorted by the keys of `order_by`, each at its place in `places`,
    /// each of the result's columns alone computed of its rows, bound as
    /// `cx`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if a key is not of a type a sort takes, or one of the
    /// result's columns breaks a rule binding it.
    pub(super) fn keyed(&mut self, cx: Context<'_>, order_by: &[SortSpecification], places: Vec<SortKeyPlace>) -> Result<()> {
        let result: Vec<OutColumn> = self.all.iter().take(self.visible).cloned().collect();
        let mut extra = Vec::new();
        let mut keys = Vec::with_capacity(places.len());
        let width = self.all.len();
        for (s, place) in order_by.iter().zip(places) {
            let column = match place {
                SortKeyPlace::Column(i) => i,
                SortKeyPlace::Result(v) => {
                    extra.push(over_result(cx, &result, &v).or_at(s.at)?);
                    width.saturating_add(extra.len()).saturating_sub(1)
                }
            };
            keys.push(SortKey::new(column, SortOrder::of(s.descending, s.nulls_first)));
        }
        self.appending(extra);
        for k in &keys {
            if let Some(c) = self.all.get(k.column) {
                super::composite::ordering_operand(&c.data_type, "ORDER BY")?;
            }
        }
        self.collations = super::collation::sort_collations((cx.db, cx.auth, cx.collation), order_by, &keys, &self.all)?;
        self.keys = keys;
        Ok(())
    }
}

/// [ISO/IEC 9075-2:2023 §7.17 SR26 d ii] The sort key `v`, of the columns
/// `columns` of a result alone, bound as `cx` over a row of them: its value
/// and the column it makes.
fn over_result(cx: Context<'_>, columns: &[OutColumn], v: &ValueExpression) -> Result<(Expr, OutColumn)> {
    let name = QualifiedName { auth: String::new(), ident: String::new() };
    let descs = columns.iter().enumerate().map(|(k, c)| column_of(k, c)).collect();
    let desc = crate::catalog::TableDesc { columns: descs, ..unconstrained(&name) };
    let source = Source::Derived(LogicalPlan::Values { rows: Vec::new(), types: Vec::new() });
    let item = FromItem::alone(name, 0, (desc, source));
    let visible = (0..columns.len()).map(|k| (0, k)).collect();
    let names = columns.iter().enumerate().map(|(k, c)| column_of(k, c).name).collect();
    let binder = Binder::new(Context { draws: false, ..cx }, None, (vec![item], names, visible), std::collections::BTreeSet::new());
    let scope = Scope::Rows("[ISO/IEC 9075-2:2023 §7.17 SR26b] a sort key contains no set function");
    let (e, data_type) = binder.value_expression(v, scope)?;
    let collation = if data_type.is_character() { binder.result_collation(v) } else { None };
    Ok((e, OutColumn { name: None, collation, data_type, not_null: false, origin: None }))
}

/// [ISO/IEC 9075-2:2023 §7.17 SR26 d i 6] While a select item added for
/// the sort is bound, the select items the result's columns' names name:
/// such a name, unqualified, is that item's value.
#[derive(Default)]
pub(super) struct SortNames {
    /// Each name of one column of the result, and its select item's value.
    pub(super) names: Vec<(String, ValueExpression)>,
    /// The place of the first select item added for the sort, while the
    /// select list and they are bound.
    pub(super) from: Option<usize>,
    /// Whether the select item bound now is one of them.
    on: bool,
}

impl Binder<'_> {
    /// Notes that the select item bound next is the `i`th.
    pub(super) fn sort_item(&self, i: usize) {
        let mut names = self.sort_names.borrow_mut();
        names.on = names.from.is_some_and(|from| i >= from);
    }

    /// The value of the select item the `<column specification>` `cs`
    /// names, if it names the result's column of one while a select item
    /// added for the sort is bound; the names off while it is.
    pub(super) fn sort_named(&self, cs: &ColumnSpecification) -> Option<ValueExpression> {
        let names = self.sort_names.borrow();
        if !names.on || cs.qualifier.is_some() {
            return None;
        }
        names.names.iter().find(|(n, _)| *n == cs.column).map(|(_, v)| v.clone())
    }

    /// `f`, the names off while it runs.
    pub(super) fn unnamed<T>(&self, f: impl FnOnce() -> T) -> T {
        let on = std::mem::take(&mut self.sort_names.borrow_mut().on);
        let found = f();
        self.sort_names.borrow_mut().on = on;
        found
    }
}

/// [ISO/IEC 9075-2:2023 §7.17 SR26 d i 9] A query sorted by a key that adds
/// a select item is not DISTINCT, and its select list has no set function
/// of its own, `selected`; not `grouped`, Feature F868.
///
/// # Errors
///
/// SQLCODE -103 if it is DISTINCT or has such a set function \[9 B II\];
/// -900 if it is grouped.
pub(super) fn hidden_rules(binder: &Binder<'_>, selected: &[SelectItem], (grouped, distinct): (bool, bool)) -> Result<()> {
    if distinct {
        return Err(rule("[ISO/IEC 9075-2:2023 §7.17 SR26 d i 9 B II] SELECT DISTINCT is ordered by columns of its result alone"));
    }
    if selected.iter().any(|i| binder.has_own_aggregate(&i.value)) {
        return Err(rule(
            "[ISO/IEC 9075-2:2023 §7.17 SR26 d i 9 B II] a query whose select list has a set function is ordered by columns of its result alone",
        ));
    }
    if grouped {
        return Err(SqlError::new(
            Code::Unsupported,
            "[ISO/IEC 9075-2:2023 §7.17 SR26 d i 9 B I] ORDER BY of a grouped query of what its select list leaves out is Feature F868, which turborel does not support",
        ));
    }
    Ok(())
}

impl Binder<'_> {
    /// Notes that the select items from the `from`th on, if any, are added
    /// for the sort; none bound now.
    pub(super) fn sorting_from(&self, from: Option<usize>) {
        let mut names = self.sort_names.borrow_mut();
        (names.from, names.on) = (from, false);
    }
}

#[cfg(test)]
mod tests;
