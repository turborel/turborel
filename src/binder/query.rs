//! [X3.135-1986 §5.25] `<query specification>`: its FROM, WHERE, GROUP BY,
//! HAVING and select list, bound and built into a plan in the order the
//! General Rules give. Split from binder.rs (RULES.md R-SIZE-2).

use std::cell::RefCell;

use super::from::{Bounds, From, bind_from, from_plan};
use super::set_functions::is_aggregate_call;
use super::set_operations::ObjectColumn;
use super::{Binder, Bound, Context, FromItem, Grouping, OutColumn, Outer, Scope, nondeterministic, rule, sort_keys, with_sort};
use crate::ast::{
    ColumnSpecification, QuerySpecification, SelectItem, SelectList, SelectSublist, SortSpecification, TableExpression, TableName,
    ValueExpression,
};
use crate::error::{At, Result};
use crate::logical::{Cond, Expr, LogicalPlan};
use crate::storage::Row;
use crate::value::Value;

/// [X3.135-1986 §5.25] `<query specification>`, with the ORDER BY of its cursor if it is the
/// whole `<query expression>`.
pub(super) fn bind_query_specification(cx: Context<'_>, spec: &QuerySpecification, order_by: &[SortSpecification]) -> Result<Bound> {
    bind_query(cx, spec, order_by, &Nesting::Query)
}

/// What a query is bound as: a query, the query of an INSERT, or a
/// subquery of the query `outer` binds, its `*` replaced by `result` if
/// given (`binder/subquery.rs`).
pub(super) enum Nesting<'a> {
    /// A query of its own.
    Query,
    /// The query of an INSERT into `columns`, the object columns
    /// (`binder/set_operations.rs`).
    Source { columns: &'a [ObjectColumn] },
    /// A subquery.
    Subquery { outer: &'a Outer<'a>, result: Option<SelectItem> },
}

impl<'a> Nesting<'a> {
    /// How an operand of a set operation it binds is bound: as it is, but
    /// for the `*` of a subquery under EXISTS, which only a `<query
    /// specification>` the subquery is has [ISO/IEC 9075:1992 §7.9 SR3a].
    pub(super) fn operand(&self) -> Nesting<'a> {
        match self {
            Nesting::Query => Nesting::Query,
            Nesting::Source { columns } => Nesting::Source { columns },
            Nesting::Subquery { outer, .. } => Nesting::Subquery { outer, result: None },
        }
    }
}

/// A query specification, bound as `nesting` says.
pub(super) fn bind_query(
    cx: Context<'_>,
    spec: &QuerySpecification,
    order_by: &[SortSpecification],
    nesting: &Nesting<'_>,
) -> Result<Bound> {
    let te = &spec.table_expression;
    let (outer, result) = match nesting {
        Nesting::Query | Nesting::Source { .. } => (None, None),
        Nesting::Subquery { outer, result } => (Some(*outer), result.clone()),
    };
    // [X3.135-1986 §5.24 SR1, §5.25 SR1] SELECT on each table.
    let clause = if outer.is_some() { "§5.24 SR1" } else { "§5.25 SR1" };
    let From { items, trees, used } = bind_from(Bounds { cx, clause, outer }, &te.from)?;
    let visible = From::visible_of(&trees, &items);
    let names = items.iter().flat_map(FromItem::row_names).collect();
    let binder = Binder::new(cx, outer, (items, names, visible), used);
    let selected = match result {
        Some(r) => vec![r],
        None => select_items(&binder, &spec.select_list)?,
    };
    let select: Vec<ValueExpression> = selected.iter().map(|i| i.value.clone()).collect();

    // [X3.135-1986 §5.21] <where clause>: applied to rows.
    let where_scope = Scope::Rows(
        "[X3.135-1986 §5.21 SR3] a <set function specification> in a <where clause> is over an outer reference, in a <subquery> of a <having clause>",
    );
    let where_cond = te.where_clause.as_ref().map(|c| binder.search_condition(c, where_scope)).transpose()?;

    // R is a grouped table if GROUP BY or HAVING is present ([X3.135-1986 §5.22, §5.23 GR1]).
    // Otherwise, set functions in the select list make R one argument [X3.135-1986 §5.25 GR1].
    let grouped = !te.group_by.is_empty() || te.having.is_some();
    let (grouping, exprs, columns) = bind_selected(&binder, &selected, (te, grouped))?;
    if cx.purpose.constrains() && spec.distinct && columns.iter().any(|c| c.data_type.is_character()) {
        return Err(nondeterministic("§7.9 SR13a", "DISTINCT with a character string column"));
    }

    // [X3.135-1986 §5.23] <having clause>: applied to groups.
    let having_rule = "[X3.135-1986 §5.23 SR1] outside a set function, HAVING may only reference grouping columns";
    let having = match (&te.having, &grouping) {
        (Some(h), Some(g)) => Some(binder.search_condition(h, Scope::Groups(g, having_rule))?),
        // HAVING always makes the query grouped, so this is only "no HAVING".
        _ => None,
    };
    let keys = sort_keys(order_by, &columns, |cs| order_by_column(&binder, &columns, cs).or_at(cs.at))?;

    let mut plan = from_plan(&binder, &trees)?;
    if let Some(cond) = where_cond {
        plan = LogicalPlan::Filter { cond, input: Box::new(plan) };
    }
    if let Some(g) = grouping {
        plan = group(plan, g, having, grouped && select.iter().all(|v| is_aggregate_call(v, cx.db)))?;
    }
    // [X3.135-1986 §5.25 GR2]
    plan = LogicalPlan::Project { exprs, input: Box::new(plan) };
    if spec.distinct {
        plan = LogicalPlan::Distinct { input: Box::new(plan) };
    }
    Ok(with_sort(Bound { plan, columns }, keys))
}

/// The items of a `<select list>`. [X3.135-1986 §5.25 SR4] "*" is a
/// `<column specification>` for every column of R, in order; [ISO/IEC
/// 9075:1992 §7.9 SR3b] those of the FROM clause's results, a NATURAL
/// join's common columns once. [SR4] `<qualifier>`.* is one for every
/// column of the table it designates.
fn select_items(binder: &Binder<'_>, list: &SelectList) -> Result<Vec<SelectItem>> {
    let item_column = |i: usize, k: usize| {
        let it = binder.items.get(i)?;
        let at = it.at;
        let qualifier = match &it.correlation {
            Some(c) => TableName { auth: None, ident: c.clone(), at },
            None => TableName { auth: Some(it.table.auth.clone()), ident: it.table.ident.clone(), at },
        };
        let column = ColumnSpecification { qualifier: Some(qualifier), column: it.desc.columns.get(k)?.name.clone(), at };
        Some(SelectItem { value: ValueExpression::Column(column), alias: None, at })
    };
    let SelectList::Values(sublists) = list else {
        return Ok(binder.visible.iter().filter_map(|&(i, k)| item_column(i, k)).collect());
    };
    let mut items = Vec::new();
    for sublist in sublists {
        match sublist {
            SelectSublist::Derived(item) => items.push(item.clone()),
            SelectSublist::All(q) => {
                let i = binder.items.iter().position(|it| it.designated_by(q, binder.schema)).ok_or_else(|| {
                    rule(format!("[ISO/IEC 9075:1992 §7.9 SR4, §6.4 SR2] no table or correlation name {} is in scope", q.ident)).or_at(q.at)
                })?;
                let (width, common) = binder.items.get(i).map_or((0, &[][..]), |it| (it.desc.columns.len(), it.common.as_slice()));
                items.extend((0..width).filter(|k| !common.contains(k)).filter_map(|k| item_column(i, k)));
            }
        }
    }
    Ok(items)
}

/// The select list `selected` of a query of table expression `te`, bound
/// with its grouping, if the query is `grouped` or its select list has a set
/// function of its own: or, if a subquery of it has a set function over an
/// outer reference to the query, which aggregates it [ISO/IEC 9075:1992
/// §6.5 SR5a, §7.9 SR7], bound again as one group. The grouping it is bound
/// with, its expressions and its columns.
fn bind_selected(
    binder: &Binder<'_>,
    selected: &[SelectItem],
    (te, grouped): (&TableExpression, bool),
) -> Result<(Option<Grouping>, Vec<Expr>, Vec<OutColumn>)> {
    let grouping = if grouped || selected.iter().any(|i| binder.has_own_aggregate(&i.value)) {
        // [X3.135-1986 §5.22 SR2] each grouping column references a column of T.
        let columns = te.group_by.iter().map(|cs| binder.resolve(cs).map(|(p, _)| p)).collect::<Result<Vec<_>>>()?;
        Some(Grouping { columns, aggs: RefCell::new(Vec::new()) })
    } else {
        None
    };
    let first = bind_select_list(binder, selected, select_scope(grouping.as_ref(), grouped));
    if first.is_ok() || grouping.is_some() || !binder.aggregated.get() {
        return first.map(|(exprs, columns)| (grouping, exprs, columns));
    }
    let one = Some(Grouping { columns: Vec::new(), aggs: RefCell::new(Vec::new()) });
    let (exprs, columns) = bind_select_list(binder, selected, select_scope(one.as_ref(), grouped))?;
    Ok((one, exprs, columns))
}

/// What a column reference in the select list means: a row, or a group of a
/// grouped or aggregated query.
fn select_scope(grouping: Option<&Grouping>, grouped: bool) -> Scope<'_> {
    let Some(g) = grouping else {
        // Not aggregated, so the select list has no set function to refuse.
        return Scope::Select;
    };
    let why = if grouped {
        "[X3.135-1986 §5.25 SR7] outside a set function, a grouped query may only reference grouping columns"
    } else {
        "[X3.135-1986 §5.25 SR7] when the select list has a set function, every column reference must be inside one"
    };
    Scope::Groups(g, why)
}

/// [X3.135-1986 §5.25] `<select list>`: an expression and a result column for each item.
fn bind_select_list(binder: &Binder<'_>, select: &[SelectItem], scope: Scope<'_>) -> Result<(Vec<Expr>, Vec<OutColumn>)> {
    let mut exprs = Vec::new();
    let mut columns = Vec::new();
    // Bound again, aggregated, it notes them again (`bind_selected`).
    let noted = binder.selected.zip(binder.uses);
    if let Some((selected, _)) = noted {
        selected.borrow_mut().clear();
    }
    for item in select {
        let v = &item.value;
        let start = noted.map_or(0, |(_, uses)| uses.borrow().len());
        let (e, data_type) = binder.value_expression(v, scope).or_at(item.at)?;
        if let Some((selected, uses)) = noted {
            selected.borrow_mut().push(uses.borrow().get(start..).unwrap_or_default().to_vec());
        }
        // [X3.135-1986 §5.25 SR9] a named column only for a single <column specification>,
        // or [ISO/IEC 9075:1992 §7.9 SR9a] one with an <as clause>;
        // [X3.135-1986 §5.25 SR10] NOT NULL only if it is a named column of a NOT NULL column.
        let (name, not_null, origin) = match v {
            // A column of this query; or an outer reference, in a subquery.
            ValueExpression::Column(cs) => match binder.local(cs)? {
                Some((pos, _, not_null)) => (Some(cs.column.clone()), not_null, Some(pos)),
                None => (Some(cs.column.clone()), false, None),
            },
            ValueExpression::Dyadic(..)
            | ValueExpression::Monadic(..)
            | ValueExpression::Value(_)
            | ValueExpression::SetFunction(_)
            | ValueExpression::Nested(_)
            | ValueExpression::Call { .. }
            | ValueExpression::Function { .. }
            | ValueExpression::Case(_)
            | ValueExpression::Cast { .. }
            | ValueExpression::Datetime(_)
            | ValueExpression::Extract { .. }
            | ValueExpression::Difference { .. }
            | ValueExpression::AtZone { .. }
            | ValueExpression::Subquery(_) => (None, false, None),
        };
        exprs.push(e);
        columns.push(OutColumn { name: item.alias.clone().or(name), data_type, not_null, origin });
    }
    Ok((exprs, columns))
}

/// [X3.135-1986 §8.3 SR6] The result column an ORDER BY `<column specification>` names.
fn order_by_column(binder: &Binder<'_>, columns: &[OutColumn], cs: &ColumnSpecification) -> Result<usize> {
    let matches: Vec<usize> = match &cs.qualifier {
        // Qualified: the named result column that comes from that FROM column.
        Some(_) => {
            let (pos, _) = binder.resolve(cs)?;
            columns.iter().enumerate().filter(|(_, c)| c.origin == Some(pos)).map(|(i, _)| i).collect()
        }
        // Unqualified: the named result column with that name.
        None => columns.iter().enumerate().filter(|(_, c)| c.name.as_deref() == Some(&cs.column)).map(|(i, _)| i).collect(),
    };
    match matches.as_slice() {
        [i] => Ok(*i),
        [] => Err(rule(format!("[X3.135-1986 §8.3 SR6] ORDER BY {} is not a named column of the result", cs.column))),
        [_, _, ..] => Err(rule(format!("[X3.135-1986 §8.3 SR6] ORDER BY {} matches more than one result column", cs.column))),
    }
}

/// [X3.135-1986 §5.22, §5.23, §5.25 GR1-GR3] Groups `plan` and computes the
/// set functions, keeps the groups HAVING accepts, and, if `default_row`,
/// gives one row of set-function results when there are no groups.
fn group(plan: LogicalPlan, g: Grouping, having: Option<Cond>, default_row: bool) -> Result<LogicalPlan> {
    let aggs = g.aggs.into_inner();
    let mut empty_row: Row = vec![Value::Null; g.columns.len()];
    for a in &aggs {
        empty_row.push(a.func.0.accumulator(a.result).finish()?);
    }
    let mut plan = LogicalPlan::Aggregate { groups: g.columns, aggs, input: Box::new(plan) };
    if let Some(cond) = having {
        plan = LogicalPlan::Filter { cond, input: Box::new(plan) };
    }
    if default_row {
        plan = LogicalPlan::DefaultRow { row: empty_row, input: Box::new(plan) };
    }
    Ok(plan)
}
