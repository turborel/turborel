//! [X3.135-1986 §5.25] `<query specification>`: its FROM, WHERE, GROUP BY,
//! HAVING and select list, bound and built into a plan in the order the
//! General Rules give.

use std::cell::RefCell;

use super::collation::{any_of, of_column};
use super::from::{Bounds, From, bind_from, from_plan};
use super::order_by::{Keys, Ordered, hidden_rules};
use super::set_operations::ObjectColumn;
use super::{Binder, Bound, Context, FromItem, Grouping, OutColumn, Outer, Scope, nondeterministic, rule};
use crate::ast::{
    ColumnSpecification, GroupingColumn, QuerySpecification, SelectItem, SelectList, SelectSublist, SortColumn, SortSpecification,
    TableExpression, TableName, ValueExpression,
};
use crate::db::State;
use crate::error::{At, Result};
use crate::logical::{Expr, LogicalPlan};
use crate::value::collation::CollationRef;

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

/// A query specification, bound as `nesting` says, sorted by `order_by`.
pub(super) fn bind_query(
    cx: Context<'_>,
    spec: &QuerySpecification,
    order_by: &[SortSpecification],
    nesting: &Nesting<'_>,
) -> Result<Bound> {
    Ok(bind_ordered(cx, spec, order_by, nesting)?.sorted())
}

/// A query specification, bound as `nesting` says: its rows and the keys
/// of `order_by` that sort them, not yet sorted.
pub(super) fn bind_ordered(
    cx: Context<'_>,
    spec: &QuerySpecification,
    order_by: &[SortSpecification],
    nesting: &Nesting<'_>,
) -> Result<Ordered> {
    let te = &spec.table_expression;
    let (outer, result) = match nesting {
        Nesting::Query | Nesting::Source { .. } => (None, None),
        Nesting::Subquery { outer, result } => (Some(*outer), result.clone()),
    };
    // [X3.135-1986 §5.24 SR1, §5.25 SR1] SELECT on each table.
    let clause = if outer.is_some() { "§5.24 SR1" } else { "§5.25 SR1" };
    let From { items, trees, used, .. } = bind_from(Bounds { cx, clause, outer }, &te.from)?;
    let visible = From::visible_of(&trees, &items);
    let names = items.iter().flat_map(FromItem::row_names).collect();
    // [ISO/IEC 9075-2:2023 §6.14 SR2] none of DISTINCT draws a value.
    let binder = Binder::new(Context { draws: cx.draws && !spec.distinct, ..cx }, outer, (items, names, visible), used);
    let selected = match result {
        Some(r) => vec![r],
        None => select_items(&binder, &spec.select_list)?,
    };
    // [ISO/IEC 9075-2:2023 §7.15, §6.10 SR2] its <window clause>, and the
    // window functions of its select list and its sort keys.
    if !te.window.is_empty() || selected.iter().any(|i| super::window::has_window_function(&i.value)) || keyed_windows(order_by) {
        *binder.windowing.borrow_mut() = Some(super::window::Windowing::new(&te.window)?);
    }

    // [X3.135-1986 §5.21] <where clause>: applied to rows.
    let where_scope = Scope::Rows(
        "[X3.135-1986 §5.21 SR3] a <set function specification> is in a <where clause> only in a <having clause>'s <subquery>, of an outer reference",
    );
    let where_cond = te.where_clause.as_ref().map(|c| binder.search_condition(c, where_scope)).transpose()?;

    // R is a grouped table if GROUP BY or HAVING is present ([X3.135-1986 §5.22, §5.23 GR1]).
    // Otherwise, set functions in the select list make R one argument [X3.135-1986 §5.25 GR1].
    let grouped = te.group_by.is_some() || te.having.is_some();
    let ((grouping, exprs, columns), keys, width) = selected_and_keys(&binder, &selected, (te, grouped, spec.distinct), order_by)?;
    let own = columns.get(..width).unwrap_or_default();
    if let Some(g) = &grouping {
        super::grouping_sets::checked(g, te.group_by.as_ref(), own)?;
    }
    if cx.purpose.constrains() && spec.distinct && own.iter().any(|c| c.data_type.is_character()) {
        return Err(nondeterministic("§7.9 SR13a", "DISTINCT with a character string column"));
    }

    // [X3.135-1986 §5.23] <having clause>: applied to groups.
    let having_rule = "[X3.135-1986 §5.23 SR1] outside a set function, HAVING may only reference grouping columns";
    let having = match (&te.having, &grouping) {
        (Some(h), Some(g)) => Some(binder.search_condition(h, Scope::Groups(g, having_rule))?),
        // HAVING always makes the query grouped, so this is only "no HAVING".
        _ => None,
    };

    let mut plan = from_plan(&binder, &trees)?;
    if let Some(cond) = where_cond {
        plan = LogicalPlan::Filter { cond, input: Box::new(plan) };
    }
    // [X3.135-1986 §5.25 GR2]
    let (mut ordered, places) = keys.ordered((exprs, columns, width), |exprs, columns| {
        selected_plan(&binder, plan, (grouping, having), (exprs, columns, spec.distinct))
    })?;
    ordered.keyed(binder.context(), order_by, places)?;
    Ok(ordered)
}

/// Whether a sort key of `order_by` has a window function [ISO/IEC
/// 9075-2:2023 §6.10 SR2a].
fn keyed_windows(order_by: &[SortSpecification]) -> bool {
    order_by.iter().any(|s| match &s.key {
        SortColumn::Value(v) => super::window::has_window_function(v),
        SortColumn::Position(_) | SortColumn::Column(_) => false,
    })
}

/// A select list bound with its grouping, as `bind_selected` binds it.
type SelectedList = (Option<Grouping>, Vec<Expr>, Vec<OutColumn>);

/// The select list `selected` of a query of table expression `te`, bound
/// with its grouping, and the keys of `order_by` placed. Any select items
/// they add [ISO/IEC 9075-2:2023 §7.17 SR26 d i 10] are bound with it,
/// after its own, whose number it gives too.
fn selected_and_keys(
    binder: &Binder<'_>,
    selected: &[SelectItem],
    (te, grouped, distinct): (&TableExpression, bool, bool),
    order_by: &[SortSpecification],
) -> Result<(SelectedList, Keys, usize)> {
    let own = bind_selected(binder, selected, (te, grouped))?;
    let keys = Keys::of(binder, order_by, selected, &own.2)?;
    let width = own.2.len();
    let Some(first) = keys.hidden.first() else {
        return Ok((own, keys, width));
    };
    hidden_rules(binder, selected, (grouped, distinct)).or_at(first.at)?;
    let all: Vec<SelectItem> = selected.iter().chain(&keys.hidden).cloned().collect();
    binder.sorting_from(Some(width));
    let with_added = bind_selected(binder, &all, (te, grouped));
    binder.sorting_from(None);
    Ok((with_added?, keys, width))
}

/// [X3.135-1986 §5.25 GR2, GR3] The select list `exprs`, of `columns`, of
/// the rows `plan`, grouped by `grouping` and `having` if so, DISTINCT if
/// `once`: of a grouped windowed query, its groups are [ISO/IEC 9075-2:2023
/// §9.20 SR22] (`grouping_sets::grouped`).
fn selected_plan(
    binder: &Binder<'_>,
    plan: LogicalPlan,
    (grouping, having): (Option<Grouping>, Option<crate::logical::Cond>),
    (exprs, columns, once): (Vec<Expr>, &[OutColumn], bool),
) -> Result<LogicalPlan> {
    let windowed = grouping.is_some() && binder.has_windows();
    let plan = match grouping {
        Some(g) => super::grouping_sets::grouped(binder, plan, (g, having), (exprs, columns, once)),
        None => super::window::windowed(binder.take_windows(), plan, exprs),
    };
    if !once {
        return Ok(plan);
    }
    // [ISO/IEC 9075-2:2023 §9.12 SR2e] each column an operand of a
    // grouping operation.
    columns.iter().try_for_each(|c| super::composite::grouping_operand(&c.data_type, "DISTINCT"))?;
    Ok(if windowed { plan } else { distinct(plan, (binder.db, binder.session_collation), columns) })
}

/// `plan`, of `columns`, DISTINCT: rows the same by each column's collation
/// [ISO/IEC 9075:1992 §8.2 GR3d], grouped if one is not the default, which
/// gives the first of each, as DISTINCT does.
pub(super) fn distinct(plan: LogicalPlan, (db, session): (&State, Option<&CollationRef>), columns: &[OutColumn]) -> LogicalPlan {
    let collations = any_of(columns.iter().map(|c| of_column(db, session, c)).collect());
    if collations.is_empty() {
        return LogicalPlan::Distinct { input: Box::new(plan) };
    }
    LogicalPlan::Aggregate { groups: (0..columns.len()).collect(), collations, aggs: Vec::new(), seeds: Vec::new(), input: Box::new(plan) }
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
        let column = ColumnSpecification { qualifier: Some(qualifier), column: it.desc.columns.get(k)?.name.clone(), catalog: None, at };
        Some(SelectItem { value: ValueExpression::Column(column), alias: None, at })
    };
    let SelectList::Values(sublists) = list else {
        return Ok(binder.visible.iter().filter_map(|&(i, k)| item_column(i, k)).collect());
    };
    let mut items = Vec::new();
    for sublist in sublists {
        match sublist {
            SelectSublist::Derived(item) => items.push(item.clone()),
            SelectSublist::Fields { value, names, at } => items.extend(binder.all_fields(value, names.as_deref(), *at)?),
            SelectSublist::All(q) => {
                let table = binder.items.iter().position(|it| it.designated_by(q, binder.schema));
                // [ISO/IEC 9075-2:2023 §7.16 SR10f iv, vii, SR10g ii] else
                // a column of a row type its correlation name qualifies.
                if let Some(fields) = binder.row_asterisk(q, table.is_some())? {
                    items.extend(fields);
                    continue;
                }
                let i = table.ok_or_else(|| {
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
        Some(super::grouping_sets::grouping(binder, te.group_by.as_ref())?)
    } else {
        None
    };
    // [ISO/IEC 9075-2:2023 §6.14 SR1a, SR2] its items draw values, if it
    // may and is not grouped.
    let draws = binder.draws && grouping.is_none();
    let first = binder.drawing(draws, || bind_select_list(binder, selected, select_scope(grouping.as_ref(), grouped)));
    if first.is_ok() || grouping.is_some() || !binder.aggregated.get() {
        return first.map(|(exprs, columns)| (grouping, exprs, columns));
    }
    let one = Some(Grouping {
        columns: Vec::new(),
        collations: Vec::new(),
        aggs: RefCell::new(Vec::new()),
        collated: RefCell::new(Vec::new()),
        sets: None,
    });
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

/// [X3.135-1986 §5.25] `<select list>`: an expression and a result column
/// for each item, its window functions bound with it [ISO/IEC 9075-2:2023
/// §6.10 SR2b] (`binder/window.rs`).
fn bind_select_list(binder: &Binder<'_>, select: &[SelectItem], scope: Scope<'_>) -> Result<(Vec<Expr>, Vec<OutColumn>)> {
    binder.open_windows();
    let bound = bound_select_list(binder, select, scope)?;
    binder.close_windows(scope)?;
    Ok(bound)
}

/// The items `select` of a select list bound in `scope`.
fn bound_select_list(binder: &Binder<'_>, select: &[SelectItem], scope: Scope<'_>) -> Result<(Vec<Expr>, Vec<OutColumn>)> {
    let mut exprs = Vec::new();
    let mut columns = Vec::new();
    // Bound again, aggregated, it notes them again (`bind_selected`).
    let noted = binder.selected.zip(binder.uses);
    if let Some((selected, _)) = noted {
        selected.borrow_mut().clear();
    }
    for (i, item) in select.iter().enumerate() {
        binder.sort_item(i);
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
            // [ISO/IEC 9075-2:2023 §7.13 SR17 m iii 1 A II] null in a
            // grouping set that leaves it out.
            ValueExpression::Column(cs) => match binder.local(cs)? {
                Some((pos, _, not_null)) => (Some(cs.column.clone()), not_null && !left_out(scope, pos), Some(pos)),
                None => (Some(cs.column.clone()), false, None),
            },
            ValueExpression::Dyadic(..)
            | ValueExpression::Monadic(..)
            | ValueExpression::Value(_)
            | ValueExpression::SetFunction(_)
            | ValueExpression::Nested(_)
            | ValueExpression::Collated(..)
            | ValueExpression::CollationFor(_)
            | ValueExpression::Call { .. }
            | ValueExpression::Function { .. }
            | ValueExpression::Case(_)
            | ValueExpression::Cast { .. }
            | ValueExpression::Datetime(_)
            | ValueExpression::Extract { .. }
            | ValueExpression::Difference { .. }
            | ValueExpression::AtZone { .. }
            | ValueExpression::Boolean(_)
            | ValueExpression::Subquery(_)
            | ValueExpression::Composite(..)
            | ValueExpression::Window(_)
            | ValueExpression::NestedWindow(_)
            | ValueExpression::NextValue { .. } => (None, false, None),
        };
        exprs.push(e);
        let collation = if data_type.is_character() { binder.result_collation(v) } else { None };
        columns.push(OutColumn { name: item.alias.clone().or(name), collation, data_type, not_null, origin });
    }
    Ok((exprs, columns))
}

/// Whether a grouping set of the groups `scope` is of leaves out the
/// FROM row's column `pos`.
fn left_out(scope: Scope<'_>, pos: usize) -> bool {
    match scope {
        Scope::Groups(g, _) => g.sets.as_ref().is_some_and(|s| s.left_out(pos)),
        Scope::Rows(_) | Scope::Select => false,
    }
}

impl Binder<'_> {
    /// [ISO/IEC 9075:1992 §7.7] The collation `g` groups by.
    pub(super) fn grouped_by(&self, g: &GroupingColumn) -> Result<Option<CollationRef>> {
        let (_, t) = self.resolve(&g.column)?;
        if !t.is_character() {
            return Ok(None);
        }
        let implicit = self.derived(&ValueExpression::Column(g.column.clone()))?;
        self.chosen(g.collate.as_ref(), implicit)
    }
}
