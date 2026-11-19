//! [ISO/IEC 9075-2:2023 §7.6 SR8] A `<collection derived table>`, bound
//! and planned.
//!
//! It is a lateral derived table [SR8n]: its collections may reference
//! every table reference before it in its FROM clause [SR20a], as its
//! item's place in the FROM row has them before it. Its columns are its
//! elements', a row type's fields each one of its own, then the ordinal
//! if WITH ORDINALITY is written [SR8m].
//!
//! A FROM clause whose collection derived tables read nothing before them
//! is planned as any. One that reads is planned over the rows it reads
//! (`placed`): the plan of what is before it is the input of its
//! `LogicalPlan::Unnest`, of an inner join too, and of a LEFT join whose
//! second operand it is, whose condition it is then given. A RIGHT or FULL
//! join's second operand reads nothing of its first [§7.10 SR4].

use std::collections::BTreeSet;
use std::sync::Arc;

use super::composite::integer;
use super::from::{Bounds, From, FromItem, Join, Tree, leaf, renamed, tree_items};
use super::views::Source;
use super::{Binder, Context, Scope, rule};
use crate::ast::{JoinKind, UnnestTable};
use crate::catalog::{ColumnDesc, QualifiedName, TableDesc};
use crate::error::{Code, Result, SqlError};
use crate::json::JsonTable;
use crate::logical::{Cond, Expr, LogicalPlan, OuterJoin, Truth, Unnest, map_cond_columns, map_expr_columns};
use crate::value::DataType;

/// A collection derived table, bound: its collections, over the columns
/// of the FROM row before it.
#[derive(Clone)]
pub(super) struct Unnested {
    /// The `<collection value expression>`s.
    collections: Vec<Expr>,
    /// Of each, the degree of its element type if it is a row type.
    spread: Vec<Option<usize>>,
    /// WITH ORDINALITY.
    ordinality: bool,
    /// The columns of the FROM row its collections reference.
    pub(super) reads: BTreeSet<usize>,
    /// [ISO/IEC 9075-2:2023 §7.11] Of a `<JSON table>`, planned as a
    /// collection derived table is (`binder/json/table.rs`): the table,
    /// its operands being `collections`.
    json: Option<Arc<JsonTable>>,
}

impl Unnested {
    /// \[EXTENSION\] A table function written in Rust, of `width` columns,
    /// its invocation `call` the multiset of its rows, which reads the
    /// columns `reads` of the FROM row (`binder/builtin_table.rs`).
    pub(super) fn rows_of(call: Expr, width: usize, reads: BTreeSet<usize>) -> Unnested {
        Unnested { collections: vec![call], spread: vec![Some(width)], ordinality: false, reads, json: None }
    }

    /// [ISO/IEC 9075-2:2023 §7.11] The JSON table `table`, of the operands
    /// `operands`, which read the columns `reads` of the FROM row.
    pub(super) fn json(table: JsonTable, operands: Vec<Expr>, reads: BTreeSet<usize>) -> Unnested {
        Unnested { collections: operands, spread: Vec::new(), ordinality: false, reads, json: Some(Arc::new(table)) }
    }
}

/// [ISO/IEC 9075-2:2023 §7.6 SR8c, SR8e] The collections of `table`, bound
/// by `binder`, and the element type of each: each an array, or one
/// multiset alone, of no WITH ORDINALITY, as the array it is cast to.
fn collections_of(binder: &Binder<'_>, table: &UnnestTable) -> Result<(Vec<Expr>, Vec<DataType>)> {
    let scope = Scope::Rows("[ISO/IEC 9075-2:2023 §6.9 SR7] a set function in a <collection derived table> is over an outer reference");
    let (mut collections, mut types) = (Vec::new(), Vec::new());
    for collection in &table.collections {
        let (expr, value_type) = binder.value_expression(collection, scope)?;
        let element = if let DataType::Array(array) = &value_type {
            array.element.clone()
        } else if let Some(m) = value_type.multiset_type() {
            if table.collections.len() != 1 || table.ordinality {
                return Err(rule(
                    "[ISO/IEC 9075-2:2023 §7.6 SR8e i] a multiset is unnested alone, and WITH ORDINALITY gives it no ordinal".to_owned(),
                ));
            }
            m.element.clone()
        } else {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §7.6 SR8c] a <collection value expression> is an array or a multiset, not of {value_type}"
            )));
        };
        collections.push(expr);
        types.push(element);
    }
    Ok((collections, types))
}

/// [ISO/IEC 9075-2:2023 §7.6 SR8] The collection derived table `table`, its
/// first column at `offset`: its collections bound over every item of
/// `from` [SR20a], each an array, or one multiset alone [SR8c, SR8e], its
/// columns named by its derived column list, else as SR8m ii names them.
pub(super) fn unnest_item(bounds: Bounds<'_>, table: &UnnestTable, offset: usize, from: &mut From) -> Result<FromItem> {
    let binder = lateral_binder(bounds, from);
    let (collections, types) = collections_of(&binder, table)?;
    let reads = binder.used.into_inner();
    from.used.extend(reads.iter().copied());
    let spread = types.iter().map(|element| if let DataType::Row(row) = element { Some(row.fields.len()) } else { None }).collect();
    let columns = columns_of(&types, table)?;
    let u = Unnested { collections, spread, ordinality: table.ordinality, reads, json: None };
    Ok(lateral_item(&table.correlation, table.at, columns, offset, u))
}

/// [ISO/IEC 9075-2:2023 §7.6 SR20a] A binder of what a lateral table
/// reads: every table reference before it in `from`.
pub(super) fn lateral_binder<'a>(bounds: Bounds<'a>, from: &From) -> Binder<'a> {
    let mut ids: Vec<usize> = (0..from.items.len()).collect();
    ids.sort_by_key(|&i| from.items.get(i).map_or(0, |it| it.offset));
    let items: Vec<FromItem> = ids.iter().filter_map(|&i| from.items.get(i)).cloned().collect();
    let local = |item: usize| ids.iter().position(|&id| id == item);
    let seen = From::visible_of(&from.trees, &from.items).into_iter().chain(from.lateral.iter().copied());
    let visible = seen.filter_map(|(i, col)| Some((local(i)?, col))).collect();
    let names = items.iter().flat_map(FromItem::row_names).collect();
    // [ISO/IEC 9075-2:2023 §6.14 SR1] none of its rows draws a value.
    let cx = Context { draws: false, ..bounds.cx };
    Binder::new(cx, bounds.outer, (items, names, visible), BTreeSet::new())
}

/// The FROM item of a lateral table `u`, its correlation name
/// `correlation`, written at `at`, of `columns`, the first at `offset`.
pub(super) fn lateral_item(correlation: &str, at: usize, columns: Vec<ColumnDesc>, offset: usize, u: Unnested) -> FromItem {
    let name = QualifiedName { auth: String::new(), ident: correlation.to_owned() };
    let desc = TableDesc {
        periods: Vec::new(),
        typed: None,
        name: name.clone(),
        columns,
        constraints: Vec::new(),
        engine: String::new(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    FromItem {
        table: name,
        at,
        correlation: Some(correlation.to_owned()),
        desc,
        offset,
        source: Source::Unnest(Box::new(u)),
        nullable: false,
        common: Vec::new(),
        drawn: None,
        family: Vec::new(),
    }
}

/// [ISO/IEC 9075-2:2023 §7.6 SR8m] The columns of `u`, of collections of
/// the element types `types`: one of each element, or one of each field
/// of a row type, then the ordinal, `NORD`, exact numeric of scale 0
/// [SR8l]; named by the derived column list [SR8m i], else each field by
/// its name and the rest by its position [SR8m ii], no name twice
/// [§7.6 SR4, ISO/IEC 9075:1992 §6.3 SR6].
fn columns_of(types: &[DataType], u: &UnnestTable) -> Result<Vec<ColumnDesc>> {
    let mut named: Vec<(Option<String>, DataType)> = Vec::new();
    for t in types {
        if let DataType::Row(r) = t {
            named.extend(r.fields.iter().map(|f| (Some(f.name.clone()), f.data_type.clone())));
        } else {
            named.push((None, t.clone()));
        }
    }
    if u.ordinality {
        named.push((None, integer()));
    }
    let mut columns: Vec<ColumnDesc> = (1_usize..)
        .zip(named)
        .map(|(k, (name, data_type))| ColumnDesc {
            name: name.unwrap_or_else(|| format!("#{k}")),
            data_type,
            not_null: false,
            default: None,
            domain: None,
            collation: None,
            generated: None,
        })
        .collect();
    let listed = match &u.columns {
        Some(list) if list.len() != columns.len() => {
            let msg = format!("[ISO/IEC 9075-2:2023 §7.6 SR8m i 2] {} column names for {} columns", list.len(), columns.len());
            return Err(rule(msg));
        }
        Some(list) => list.clone(),
        None => columns.iter().map(|c| c.name.clone()).collect(),
    };
    renamed(&mut columns, &listed)?;
    Ok(columns)
}

/// [ISO/IEC 9075-2:2023 §7.10 SR4] Of a RIGHT or FULL join whose first
/// operand is the columns `first` of the FROM row, its second operand
/// `right`: no collection derived table of it references the first.
pub(super) fn not_lateral(right: &Tree, items: &[FromItem], first: std::ops::Range<usize>, refused: &str) -> Result<()> {
    let mut ids = Vec::new();
    tree_items(right, &mut ids);
    let reads = |it: &FromItem| if let Source::Unnest(u) = &it.source { u.reads.range(first.clone()).next().is_some() } else { false };
    if ids.iter().filter_map(|&i| items.get(i)).any(reads) {
        return Err(rule(refused));
    }
    Ok(())
}

/// A plan of columns of the FROM row: those from `base` on.
pub(super) struct Placed {
    pub(super) plan: LogicalPlan,
    pub(super) base: usize,
}

/// The first column of `tree` in the FROM row.
fn first(tree: &Tree, items: &[FromItem]) -> usize {
    let mut ids = Vec::new();
    tree_items(tree, &mut ids);
    ids.iter().filter_map(|&i| items.get(i)).map(|it| it.offset).min().unwrap_or(0)
}

/// Whether the collection derived tables of `tree` read nothing before it.
fn contained(tree: &Tree, items: &[FromItem]) -> bool {
    let mut ids = Vec::new();
    tree_items(tree, &mut ids);
    let begins = first(tree, items);
    let reach = |it: &FromItem| if let Source::Unnest(u) = &it.source { u.reads.first().copied() } else { None };
    ids.iter().filter_map(|&i| items.get(i)).filter_map(reach).all(|r| r >= begins)
}

/// The item of `tree` and its collections, if it is a collection derived
/// table.
fn unnest_of<'t>(tree: &Tree, items: &'t [FromItem]) -> Option<(&'t FromItem, &'t Unnested)> {
    let Tree::Item(i) = tree else { return None };
    let it = items.get(*i)?;
    if let Source::Unnest(u) = &it.source { Some((it, u)) } else { None }
}

/// \[DESIGN\] A broken invariant of the FROM row.
fn broken(what: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] {what}"))
}

/// Valid SQL beyond what this implementation plans.
fn beyond(what: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[DESIGN] {what} is valid SQL but is not implemented"))
}

/// [ISO/IEC 9075:1992 §7.5 GR1, GR5, GR6; ISO/IEC 9075-2:2023 §7.6 SR8n]
/// The plan of `prefix`, the columns of the FROM row before `tree`, if
/// any, and of `tree`: their product, or, where a collection derived table
/// of `tree` reads `prefix`, it over `prefix`'s rows.
pub(super) fn placed(binder: &Binder<'_>, tree: &Tree, prefix: Option<Placed>) -> Result<Placed> {
    let items = &binder.items;
    let prefix = match prefix {
        Some(p) if contained(tree, items) => {
            let own = placed(binder, tree, None)?;
            return Ok(Placed { plan: LogicalPlan::Product { left: Box::new(p.plan), right: Box::new(own.plan) }, base: p.base });
        }
        other => other,
    };
    match tree {
        Tree::Item(i) => {
            let it = items.get(*i).ok_or_else(|| broken("a FROM item is gone"))?;
            if let Some((collection, u)) = unnest_of(tree, items) {
                let (input, base) = prefix.map_or_else(|| (one_row(), collection.offset), |p| (p.plan, p.base));
                return Ok(Placed { plan: unnested(binder, (collection, u), (input, base), None)?, base });
            }
            match prefix {
                Some(_) => Err(broken("a table reads the columns before it")),
                None => Ok(Placed { plan: leaf(binder, it)?, base: it.offset }),
            }
        }
        Tree::Join(j) => joined(binder, j, first(tree, items), prefix),
        // [§7.6 SR15a] its first item's plan gives its columns all.
        Tree::Group(ids) => {
            let it = ids.first().and_then(|&i| items.get(i)).ok_or_else(|| broken("a FROM item is gone"))?;
            match prefix {
                Some(_) => Err(broken("a table reads the columns before it")),
                None => Ok(Placed { plan: leaf(binder, it)?, base: it.offset }),
            }
        }
    }
}

/// The plan of the joined table `j`, its first column `begins`, over
/// `prefix` if given.
fn joined(binder: &Binder<'_>, j: &Join, begins: usize, prefix: Option<Placed>) -> Result<Placed> {
    let items = &binder.items;
    let left = match j.kind {
        JoinKind::Cross | JoinKind::Inner | JoinKind::Left => placed(binder, &j.left, prefix)?,
        JoinKind::Right | JoinKind::Full | JoinKind::Union => {
            if prefix.is_some() || !contained(&j.left, items) || !contained(&j.right, items) {
                return Err(beyond("a collection derived table that references a table reference outside the RIGHT or FULL join it is in"));
            }
            placed(binder, &j.left, None)?
        }
    };
    let base = left.base;
    // The join's condition is over its own columns, from `begins`.
    let shifted = |c: &Cond| map_cond_columns(c.clone(), &|col| col.saturating_add(begins).saturating_sub(base));
    let on = j.cond.as_ref().map_or(Cond::Const(Truth::True), shifted);
    let (kind, left) = match j.kind {
        JoinKind::Cross | JoinKind::Inner => {
            let right = placed(binder, &j.right, Some(left))?;
            let plan = match &j.cond {
                Some(_) => LogicalPlan::Filter { cond: on, input: Box::new(right.plan) },
                None => right.plan,
            };
            return Ok(commons(j, Placed { plan, base }, begins));
        }
        // [GR1d] the rows of a LEFT join's second operand, a collection
        // derived table, that its condition makes true, else nulls.
        JoinKind::Left => {
            if let Some(found) = unnest_of(&j.right, items).filter(|_| j.partitioned[1].is_none()) {
                let plan = unnested(binder, found, (left.plan, base), Some(on))?;
                return Ok(commons(j, Placed { plan, base }, begins));
            }
            if !contained(&j.right, items) {
                return Err(beyond("a collection derived table that references the first operand of a LEFT join it is nested in"));
            }
            (OuterJoin::Left, left)
        }
        JoinKind::Right => (OuterJoin::Right, left),
        JoinKind::Full => (OuterJoin::Full, left),
        // [GR1a, GR5e] UNION: no pair of rows is a row of it.
        JoinKind::Union => {
            let right = placed(binder, &j.right, None)?;
            let (l, r) = (Box::new(left.plan), Box::new(right.plan));
            let plan = LogicalPlan::OuterJoin { kind: OuterJoin::Full, cond: Cond::Const(Truth::False), left: l, right: r };
            return Ok(commons(j, Placed { plan, base }, begins));
        }
    };
    let right = placed(binder, &j.right, None)?;
    let plan = match &j.partitioned {
        [None, None] => LogicalPlan::OuterJoin { kind, cond: on, left: Box::new(left.plan), right: Box::new(right.plan) },
        partitioned => super::partitioned::plan(binder, (kind, on), (left.plan, begins.saturating_sub(base)), right.plan, partitioned),
    };
    Ok(commons(j, Placed { plan, base }, begins))
}

/// [ISO/IEC 9075:1992 §7.5 GR6a] `placed`, the plan of the joined table
/// `j`, then, of NATURAL or USING, the columns COALESCE gives its
/// corresponding join columns, theirs over its columns from `begins`.
fn commons(j: &Join, placed: Placed, begins: usize) -> Placed {
    let Some((_, coalesced)) = &j.common else { return placed };
    let shift = |e: &Expr| map_expr_columns(e.clone(), &|col| col.saturating_add(begins).saturating_sub(placed.base));
    let exprs = (0..placed.plan.width()).map(Expr::Column).chain(coalesced.iter().map(shift)).collect();
    Placed { plan: LogicalPlan::Project { exprs, input: Box::new(placed.plan) }, base: placed.base }
}

/// One row of no column: what a collection derived table that reads
/// nothing before it is over.
fn one_row() -> LogicalPlan {
    LogicalPlan::Values { rows: vec![Vec::new()], types: Vec::new() }
}

/// The plan of the collection derived table `u`, of the item `it`, over
/// `input`, the columns of the FROM row from `base`, of a LEFT join if
/// `outer` is its condition, over those columns and its own.
fn unnested(
    binder: &Binder<'_>,
    (it, u): (&FromItem, &Unnested),
    (input, base): (LogicalPlan, usize),
    outer: Option<Cond>,
) -> Result<LogicalPlan> {
    if u.reads.first().is_some_and(|&r| r < base) {
        return Err(broken("a collection derived table reads columns it is not over"));
    }
    let end = it.offset.saturating_add(it.width());
    let names = binder.names.get(it.offset..end).ok_or_else(|| broken("the FROM row lost a column name"))?.to_vec();
    let collections = u.collections.iter().map(|e| map_expr_columns(e.clone(), &|c| c.saturating_sub(base))).collect();
    let unnest = Unnest { collections, spread: u.spread.clone(), ordinality: u.ordinality, outer, names, input, json: u.json.clone() };
    Ok(LogicalPlan::Unnest(Box::new(unnest)))
}
