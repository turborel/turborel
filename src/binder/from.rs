//! [X3.135-1986 §5.20; ISO/IEC 9075:1992 §6.3, §7.4, §7.5] The `<from
//! clause>`, bound: its table references, each a table or view, a derived
//! table, or a joined table, laid out one after another in the FROM row.
//! Split from binder/query.rs (RULES.md R-SIZE-2).
//!
//! Each table and derived table is a `FromItem`, at its place in the row; a
//! joined table is a `Tree` over the items of its operands, its row theirs
//! side by side [§7.5 SR5], and after them, for NATURAL or USING, the
//! columns COALESCE gives the corresponding join columns [SR6d], an item of
//! their own that no `<qualifier>` names. The columns a query sees
//! unqualified, and * gives, are those of the table references' results
//! (`From::visible_of`): a NATURAL join's result has its common columns once
//! [SR6f], not each operand's. Every correlation and table name of the FROM
//! clause, inside a joined table too, is exposed to the query [§6.3 SR2a].
//!
//! An inner or cross join is planned as the filter over the product its
//! rows are [§7.5 GR1b, GR1c], which the optimizer plans as any; an outer
//! join as `LogicalPlan::OuterJoin`.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::case::set_type;
use super::query::Nesting;
use super::set_operations::bind_query_expression;
use super::subquery::Outer;
use super::views::{Named, Reader, Source, ViewBase, named};
use super::{Binder, Context, Purpose, Scope, qualify, require, rule};
use crate::ast::{DerivedTable, JoinKind, JoinSpecification, JoinedTable, NamedTable, SearchCondition, TableReference};
use crate::catalog::{Actions, ColumnDesc, Horizon, QualifiedName, TableDesc};
use crate::error::{At, Code, Result, SqlError};
use crate::logical::{Cond, Expr, LogicalPlan, OuterJoin, Truth, and_all, cond_columns};
use crate::system::SystemRead;

/// A table, view or derived table of a `<from clause>`, or the common
/// columns of a NATURAL or USING join, at its place in the FROM row.
#[derive(Clone)]
pub(super) struct FromItem {
    /// The table or view named; or the correlation name, of no schema.
    pub(super) table: QualifiedName,
    /// Where its `<table reference>` is written.
    pub(super) at: usize,
    pub(super) correlation: Option<String>,
    /// Its description: a view's or derived table's columns.
    pub(super) desc: TableDesc,
    /// [X3.135-1986 §5.20 GR2b] position of this table's first column in the product row.
    pub(super) offset: usize,
    /// Where its rows come from.
    pub(super) source: Source,
    /// [ISO/IEC 9075:1992 §7.5 SR7, SR8] Whether an outer join may give it
    /// nulls, a column NOT NULL among them.
    pub(super) nullable: bool,
    /// Its columns that are common columns of a NATURAL or USING join it
    /// is in, which its `<qualifier>` does not reference [ISO/IEC
    /// 9075:1992 §6.4 SR2b, §7.9 SR4].
    pub(super) common: Vec<usize>,
}

impl FromItem {
    /// The table or view `table`, written at `at`, with no correlation
    /// name, alone at the start of the row: of `desc`, its rows from
    /// `source`.
    pub(super) fn alone(table: QualifiedName, at: usize, (desc, source): (TableDesc, Source)) -> FromItem {
        FromItem { table, at, correlation: None, desc, offset: 0, source, nullable: false, common: Vec::new() }
    }

    pub(super) fn designator(&self) -> &str {
        self.correlation.as_deref().unwrap_or(&self.table.ident)
    }

    /// The number of columns it puts in the product row: an updatable
    /// view's base table's.
    pub(super) fn width(&self) -> usize {
        match &self.source {
            Source::Base(vb) => vb.base.columns.len(),
            Source::Table | Source::Derived(_) | Source::Common | Source::System(_) => self.desc.columns.len(),
        }
    }

    /// The position of its column `i` among the columns it puts in the
    /// product row.
    pub(super) fn position(&self, i: usize) -> usize {
        match &self.source {
            Source::Base(vb) => vb.map.get(i).copied().unwrap_or(i),
            Source::Table | Source::Derived(_) | Source::Common | Source::System(_) => i,
        }
    }

    /// The display names of the columns it puts in the product row: an
    /// updatable view's names, and the base table's for its other columns.
    pub(super) fn row_names(&self) -> Vec<String> {
        let designator = if matches!(self.source, Source::Common) { "" } else { self.designator() };
        let named = |i: usize| {
            self.desc.columns.get(i).map(|c| if designator.is_empty() { c.name.clone() } else { format!("{designator}.{}", c.name) })
        };
        match &self.source {
            Source::Base(vb) => (0..)
                .zip(&vb.base.columns)
                .map(|(b, c)| {
                    vb.map.iter().position(|&m| m == b).and_then(named).unwrap_or_else(|| format!("{}.{}", vb.base.name.ident, c.name))
                })
                .collect(),
            Source::Table | Source::Derived(_) | Source::Common | Source::System(_) => {
                (0..self.desc.columns.len()).filter_map(named).collect()
            }
        }
    }

    /// Does `<qualifier>` q designate this table? A `<correlation name>` hides the
    /// `<table name>` ([X3.135-1986 §5.20 SR1] only table references without one are exposed).
    pub(super) fn designated_by(&self, q: &crate::ast::TableName, auth: &str) -> bool {
        match &self.correlation {
            Some(c) => q.auth.is_none() && q.ident == *c,
            None => qualify(q, auth) == self.table,
        }
    }
}

/// A bound `<table reference>`: an item, or a joined table.
pub(super) enum Tree {
    /// The rows of the item at this index.
    Item(usize),
    /// A joined table.
    Join(Box<Join>),
}

/// [ISO/IEC 9075:1992 §7.5] A joined table, bound.
pub(super) struct Join {
    kind: JoinKind,
    left: Tree,
    right: Tree,
    /// Its condition, over its operands' columns side by side; none if
    /// every pair of rows is a row of it.
    cond: Option<Cond>,
    /// For NATURAL or USING: the item of its common columns, and the
    /// expression of each over its operands' columns [SR6d].
    common: Option<(usize, Vec<Expr>)>,
}

/// A bound `<from clause>`: its items, its table references, and the
/// columns of the FROM row its join conditions use.
pub(super) struct From {
    pub(super) items: Vec<FromItem>,
    pub(super) trees: Vec<Tree>,
    pub(super) used: BTreeSet<usize>,
}

impl From {
    /// The columns a query over `trees` of `items` sees unqualified, and *
    /// gives, in order: each an item and a column of it.
    pub(super) fn visible_of(trees: &[Tree], items: &[FromItem]) -> Vec<(usize, usize)> {
        trees.iter().flat_map(|t| columns(t, items)).collect()
    }
}

/// The columns of the result of `tree`, in order [§7.5 SR5, SR6f]: a
/// NATURAL or USING join's common columns, then each operand's others.
fn columns(tree: &Tree, items: &[FromItem]) -> Vec<(usize, usize)> {
    match tree {
        Tree::Item(i) => (0..items.get(*i).map_or(0, |it| it.desc.columns.len())).map(|c| (*i, c)).collect(),
        Tree::Join(j) => {
            let (left, right) = (columns(&j.left, items), columns(&j.right, items));
            let Some((common, _)) = &j.common else { return [left, right].concat() };
            let names: Vec<&str> =
                items.get(*common).map(|it| it.desc.columns.iter().map(|c| c.name.as_str()).collect()).unwrap_or_default();
            let other = |&(i, c): &(usize, usize)| !names.contains(&column_name(items, i, c));
            columns(&Tree::Item(*common), items)
                .into_iter()
                .chain(left.into_iter().filter(other))
                .chain(right.into_iter().filter(other))
                .collect()
        }
    }
}

/// The name of column `c` of item `i`.
fn column_name(items: &[FromItem], i: usize, c: usize) -> &str {
    items.get(i).and_then(|it| it.desc.columns.get(c)).map_or("", |col| col.name.as_str())
}

/// The items of `tree`, its common columns' among them.
fn tree_items(tree: &Tree, out: &mut Vec<usize>) {
    match tree {
        Tree::Item(i) => out.push(*i),
        Tree::Join(j) => {
            tree_items(&j.left, out);
            tree_items(&j.right, out);
            out.extend(j.common.as_ref().map(|(i, _)| *i));
        }
    }
}

/// The number of columns `tree` puts in the FROM row.
fn width(tree: &Tree, items: &[FromItem]) -> usize {
    let mut ids = Vec::new();
    tree_items(tree, &mut ids);
    ids.iter().filter_map(|&i| items.get(i)).map(FromItem::width).sum()
}

/// What a `<from clause>` is bound in: the statement's context, the rule
/// its SELECT privileges cite, and the query a subquery's references go
/// out to.
#[derive(Clone, Copy)]
pub(super) struct Bounds<'a> {
    pub(super) cx: Context<'a>,
    pub(super) clause: &'a str,
    pub(super) outer: Option<&'a Outer<'a>>,
}

/// [X3.135-1986 §5.20; ISO/IEC 9075:1992 §7.4] The table references of a
/// `<from clause>`, bound one after another in the FROM row, no two
/// exposing the same name [§6.3 SR3, SR4].
pub(super) fn bind_from(b: Bounds<'_>, refs: &[TableReference]) -> Result<From> {
    let mut from = From { items: Vec::new(), trees: Vec::new(), used: BTreeSet::new() };
    let mut start = 0_usize;
    for r in refs {
        let tree = bind_reference(b, r, start, &mut from)?;
        start = start.saturating_add(width(&tree, &from.items));
        from.trees.push(tree);
    }
    for (n, item) in from.items.iter().enumerate().filter(|(_, it)| !matches!(it.source, Source::Common)) {
        let earlier = from.items.iter().take(n).filter(|it| !matches!(it.source, Source::Common));
        if let Some(e) = earlier.into_iter().find_map(|e| exposure_conflict(item, e)) {
            return Err(e.or_at(item.at));
        }
    }
    Ok(from)
}

/// The `<table reference>` `r`, its first column at `start` of the FROM row.
fn bind_reference(b: Bounds<'_>, r: &TableReference, start: usize, from: &mut From) -> Result<Tree> {
    let item = match r {
        TableReference::Table(t) => from_item(b, t, start).or_at(t.table.at)?,
        TableReference::Derived(d) => derived(b, d, start).or_at(d.query.at)?,
        TableReference::Joined(j) => return bind_join(b, j, start, from).or_at(j.at),
    };
    from.items.push(item);
    Ok(Tree::Item(from.items.len().saturating_sub(1)))
}

/// The table or view `t` names, its first column at `offset` of the FROM
/// row: one that exists, which may be read as the Syntax Rule `clause`
/// requires, its columns renamed by a derived column
/// list [ISO/IEC 9075:1992 §6.3 SR6, SR7]. An error binding a view's query
/// is placed at the view's name, not in the view's own text.
fn from_item(b: Bounds<'_>, t: &NamedTable, offset: usize) -> Result<FromItem> {
    let cx = b.cx;
    let name = qualify(&t.table, cx.schema);
    let Named { desc, source } = named(cx.db, &name, Reader::of(&cx)).map_err(|e| SqlError { pos: None, ..e })?;
    if cx.purpose == Purpose::Constraint {
        // [ISO/IEC 9075:1992 §11.9 AR1] REFERENCES, not SELECT, for a
        // constraint: on each column it names [AR1a] (`Binder::note`), or
        // on one at least, if it names none [AR1b].
        super::referencing::table_read(&cx, &name)?;
    } else if !matches!(source, Source::System(_)) {
        // [ISO/IEC 9075:1992 §21.1] SELECT on a system table is PUBLIC's.
        require(cx.db, cx.auth, &desc, &Actions { select: true, ..Actions::default() }, b.clause)?;
    }
    let (mut desc, source) = seen(cx.horizon, &name, (desc, source));
    if let Some(names) = &t.columns {
        renamed(&mut desc.columns, names)?;
    }
    Ok(FromItem {
        table: name,
        at: t.table.at,
        correlation: t.correlation.clone(),
        desc,
        offset,
        source,
        nullable: false,
        common: Vec::new(),
    })
}

/// [ISO/IEC 9075:1992 §6.3 SR6, SR7] `columns` named as the derived
/// column list `names` says: one name each, none twice.
fn renamed(columns: &mut [ColumnDesc], names: &[String]) -> Result<()> {
    if names.len() != columns.len() {
        return Err(rule(format!("[ISO/IEC 9075:1992 §6.3 SR7] {} column names for {} columns", names.len(), columns.len())));
    }
    for (n, name) in names.iter().enumerate() {
        if names.iter().take(n).any(|earlier| earlier == name) {
            return Err(rule(format!("[ISO/IEC 9075:1992 §6.3 SR6] column {name} is named twice")));
        }
    }
    for (c, name) in columns.iter_mut().zip(names) {
        c.name.clone_from(name);
    }
    Ok(())
}

/// [ISO/IEC 9075:1992 §11.11 GR2, note] Base table `name`, of `desc` and
/// read from `source`, as `horizon` sees it: if it sees fewer columns than
/// the table has, it is read as an updatable view of those is, its first
/// columns, so its rows keep every column at its place.
fn seen(horizon: &Horizon, name: &QualifiedName, (desc, source): (TableDesc, Source)) -> (TableDesc, Source) {
    let Some(width) = horizon.width(name).filter(|&w| w < desc.columns.len() && matches!(source, Source::Table)) else {
        return (desc, source);
    };
    let mut first = desc.clone();
    first.columns.truncate(width);
    let base = ViewBase { base: desc, map: (0..width).collect(), own: None, cond: None, below: None };
    (first, Source::Base(Box::new(base)))
}

/// [ISO/IEC 9075:1992 §6.3, §7.11] A `<derived table>`, its first column at
/// `offset`: its query bound as a subquery of the query's own enclosing
/// query, if it has one, its columns named by the derived column list or
/// as the query names them; an unnamed column is named by its position.
fn derived(b: Bounds<'_>, d: &DerivedTable, offset: usize) -> Result<FromItem> {
    let nesting = match b.outer {
        Some(outer) => Nesting::Subquery { outer, result: None },
        None => Nesting::Query,
    };
    let bound = bind_query_expression(b.cx, &d.query.query, &nesting)?;
    let mut columns: Vec<ColumnDesc> = bound
        .columns
        .iter()
        .enumerate()
        .map(|(k, c)| ColumnDesc {
            name: c.name.clone().unwrap_or_else(|| format!("#{}", k.saturating_add(1))),
            data_type: c.data_type,
            not_null: c.not_null,
            default: None,
            domain: None,
        })
        .collect();
    if let Some(names) = &d.columns {
        renamed(&mut columns, names)?;
    }
    let name = QualifiedName { auth: String::new(), ident: d.correlation.clone() };
    let desc = TableDesc {
        name: name.clone(),
        columns,
        constraints: Vec::new(),
        engine: String::new(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    Ok(FromItem {
        table: name,
        at: d.query.at,
        correlation: Some(d.correlation.clone()),
        desc,
        offset,
        source: Source::Derived(bound.plan),
        nullable: false,
        common: Vec::new(),
    })
}

/// [ISO/IEC 9075:1992 §7.5] The joined table `j`, its first column at
/// `start`: its operands, its condition or common columns, and the items
/// of an operand an outer join may give nulls marked so [SR7, SR8].
fn bind_join(b: Bounds<'_>, j: &JoinedTable, start: usize, from: &mut From) -> Result<Tree> {
    let left = bind_reference(b, &j.left, start, from)?;
    let lw = width(&left, &from.items);
    let right = bind_reference(b, &j.right, start.saturating_add(lw), from)?;
    let rw = width(&right, &from.items);
    let (mut cond, mut common) = (None, None);
    match (&j.on, j.natural) {
        (Some(JoinSpecification::On(c)), _) => cond = Some(bind_on(b, c, (&left, &right), start, from)?),
        (Some(JoinSpecification::Using(names)), _) => {
            (cond, common) = natural(j, Some(names), (&left, &right), (start, start.saturating_add(lw).saturating_add(rw)), from)?;
        }
        (None, true) => (cond, common) = natural(j, None, (&left, &right), (start, start.saturating_add(lw).saturating_add(rw)), from)?,
        (None, false) => {}
    }
    let mark = |tree: &Tree, bound: &mut From| {
        let mut ids = Vec::new();
        tree_items(tree, &mut ids);
        for i in ids {
            if let Some(it) = bound.items.get_mut(i) {
                it.nullable = true;
            }
        }
    };
    if matches!(j.kind, JoinKind::Right | JoinKind::Full | JoinKind::Union) {
        mark(&left, from);
    }
    if matches!(j.kind, JoinKind::Left | JoinKind::Full | JoinKind::Union) {
        mark(&right, from);
    }
    Ok(Tree::Join(Box::new(Join { kind: j.kind, left, right, cond, common })))
}

/// [ISO/IEC 9075:1992 §7.5 SR4] The `<join condition>` `c` of the joined
/// table of `left` and `right`, starting at `start`, over their columns
/// side by side: it may reference theirs [SR4a], and outer references, a
/// set function only over one [SR4b]. The columns it uses are added to
/// `from.used`.
fn bind_on(b: Bounds<'_>, c: &SearchCondition, (left, right): (&Tree, &Tree), start: usize, from: &mut From) -> Result<Cond> {
    let mut ids = Vec::new();
    tree_items(left, &mut ids);
    tree_items(right, &mut ids);
    ids.sort_by_key(|&i| from.items.get(i).map_or(0, |it| it.offset));
    let items: Vec<FromItem> = ids
        .iter()
        .filter_map(|&i| from.items.get(i))
        .map(|it| FromItem { offset: it.offset.saturating_sub(start), ..it.clone() })
        .collect();
    let local = |i: usize| ids.iter().position(|&x| x == i);
    let visible: Vec<(usize, usize)> =
        columns(left, &from.items).into_iter().chain(columns(right, &from.items)).filter_map(|(i, col)| Some((local(i)?, col))).collect();
    let names = items.iter().flat_map(FromItem::row_names).collect();
    let cx = b.cx;
    let binder = Binder::new(cx, b.outer, (items, names, visible), BTreeSet::new());
    let cond = binder.search_condition(
        c,
        Scope::Rows("[ISO/IEC 9075:1992 §7.5 SR4b] a set function in a <join condition> is over an outer reference"),
    )?;
    from.used.extend(binder.used.into_inner().into_iter().map(|p| p.saturating_add(start)));
    Ok(cond)
}

/// The condition and common columns of a NATURAL join (`using` none) or a
/// USING join `j` of `left` and `right`, the first starting at `start` and
/// the common columns at `end` [ISO/IEC 9075:1992 §7.5 SR6, SR9, GR1d].
#[expect(clippy::type_complexity, reason = "the condition and the item of the common columns, made together")]
fn natural(
    j: &JoinedTable,
    using: Option<&[String]>,
    (left, right): (&Tree, &Tree),
    (start, end): (usize, usize),
    from: &mut From,
) -> Result<(Option<Cond>, Option<(usize, Vec<Expr>)>)> {
    let (lcols, rcols) = (columns(left, &from.items), columns(right, &from.items));
    let called = |cols: &[(usize, usize)], name: &str| {
        cols.iter().filter(|&&(i, c)| column_name(&from.items, i, c) == name).copied().collect::<Vec<_>>()
    };
    let names: Vec<String> = match using {
        Some(list) => list.to_vec(),
        None => lcols.iter().map(|&(i, c)| column_name(&from.items, i, c).to_owned()).filter(|n| !called(&rcols, n).is_empty()).collect(),
    };
    let mut equal = Vec::new();
    let mut exprs = Vec::new();
    let mut descs = Vec::new();
    let mut joined = Vec::new();
    for name in &names {
        let (in_left, in_right) = (called(&lcols, name), called(&rcols, name));
        let ([first], [second]) = (in_left.as_slice(), in_right.as_slice()) else {
            return Err(rule(format!("[ISO/IEC 9075:1992 §7.5 SR6a, SR6b] {name} is not the name of exactly one column of each operand")));
        };
        let (cond, coalesced, desc, positions) = corresponding(j.kind, (&from.items, *first, *second), start)?;
        from.used.extend(positions);
        joined.extend([*first, *second]);
        equal.push(cond);
        exprs.push(coalesced);
        descs.push(desc);
    }
    for (item, column) in joined {
        from.items.get_mut(item).into_iter().for_each(|it| it.common.push(column));
    }
    if descs.is_empty() {
        return Ok((None, None));
    }
    let name = QualifiedName { auth: String::new(), ident: format!("#{}", from.items.len()) };
    let desc = TableDesc { columns: descs, ..super::views::unconstrained(&name) };
    from.items.push(FromItem {
        correlation: Some(name.ident.clone()),
        table: name,
        at: j.at,
        desc,
        offset: end,
        source: Source::Common,
        nullable: false,
        common: Vec::new(),
    });
    Ok((and_all(equal), Some((from.items.len().saturating_sub(1), exprs))))
}

/// [ISO/IEC 9075:1992 §7.5 SR6c, SR6d, SR9] A pair of corresponding join
/// columns of a join of `kind`, each an item and a column of it, the join
/// starting at `start`: their equality and COALESCE over the join's
/// columns, its common column's descriptor, and their places in the FROM
/// row.
fn corresponding(
    kind: JoinKind,
    (items, (left_item, left_column), (right_item, right_column)): (&[FromItem], (usize, usize), (usize, usize)),
    start: usize,
) -> Result<(Cond, Expr, ColumnDesc, [usize; 2])> {
    let lost = || SqlError::new(Code::Api, "[DESIGN] a join operand lost a column");
    let (first, second) = (items.get(left_item).ok_or_else(lost)?, items.get(right_item).ok_or_else(lost)?);
    let (c1, c2) = (first.desc.columns.get(left_column).ok_or_else(lost)?, second.desc.columns.get(right_column).ok_or_else(lost)?);
    // [SR6c] comparable.
    let common = set_type(&[c1.data_type, c2.data_type], "§7.5 SR6c")?;
    let at1 = first.offset.saturating_add(first.position(left_column));
    let at2 = second.offset.saturating_add(second.position(right_column));
    let (local1, local2) = (at1.saturating_sub(start), at2.saturating_sub(start));
    let cast = |e: Expr, own| super::case::brought(e, own, common);
    // [SR6d] COALESCE (TA.C, TB.C).
    let coalesced = Expr::Case(
        vec![(Cond::IsNull(Expr::Column(local1), true), cast(Expr::Column(local1), c1.data_type))],
        Box::new(cast(Expr::Column(local2), c2.data_type)),
    );
    // [SR9] possibly nullable only if an outer join may null it on the side it is.
    let (null1, null2) = (!c1.not_null || first.nullable, !c2.not_null || second.nullable);
    let nullable = (matches!(kind, JoinKind::Right | JoinKind::Full | JoinKind::Union) && null1)
        || (matches!(kind, JoinKind::Left | JoinKind::Full | JoinKind::Union) && null2);
    let desc = ColumnDesc { name: c1.name.clone(), data_type: common, not_null: !nullable, default: None, domain: None };
    let equal = Cond::Compare(Expr::Column(local1), crate::value::CompOp::Eq, Expr::Column(local2));
    Ok((equal, coalesced, desc, [at1, at2]))
}

/// [X3.135-1986 §5.20 SR2, SR3; ISO/IEC 9075:1992 §6.3 SR3, SR4] The rule
/// two table references of one FROM clause break, if any: the same exposed
/// table, the same correlation name, or a correlation name equal to an
/// exposed table's.
fn exposure_conflict(a: &FromItem, b: &FromItem) -> Option<SqlError> {
    let same_name = |x: &String| rule(format!("[X3.135-1986 §5.20 SR3] correlation name {x} equals an exposed table identifier"));
    match (&a.correlation, &b.correlation) {
        (None, None) => (a.table == b.table).then(|| rule(format!("[X3.135-1986 §5.20 SR2] table {} is exposed more than once", a.table))),
        (Some(x), Some(y)) => (x == y).then(|| rule(format!("[X3.135-1986 §5.20 SR3] correlation name {x} is used twice"))),
        (Some(x), None) => (*x == b.table.ident).then(|| same_name(x)),
        (None, Some(x)) => (*x == a.table.ident).then(|| same_name(x)),
    }
}

/// [X3.135-1986 §5.20; ISO/IEC 9075:1992 §7.4 GR1] The plan of the table
/// references `trees`: their product, in FROM order. Each scan is told
/// which of its columns the query uses.
pub(super) fn from_plan(binder: &Binder<'_>, trees: &[Tree]) -> Result<LogicalPlan> {
    let mut plan: Option<LogicalPlan> = None;
    for tree in trees {
        let next = tree_plan(binder, tree)?;
        plan = Some(match plan {
            None => next,
            Some(left) => LogicalPlan::Product { left: Box::new(left), right: Box::new(next) },
        });
    }
    plan.ok_or_else(|| rule("[X3.135-1986 §5.20 F] a <from clause> has at least one <table reference>"))
}

/// The plan of `tree` [ISO/IEC 9075:1992 §7.5 GR1, GR5, GR6].
fn tree_plan(binder: &Binder<'_>, tree: &Tree) -> Result<LogicalPlan> {
    let j = match tree {
        Tree::Item(i) => {
            return leaf(binder, binder.items.get(*i).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a FROM item is gone"))?);
        }
        Tree::Join(j) => j,
    };
    let (left, right) = (Box::new(tree_plan(binder, &j.left)?), Box::new(tree_plan(binder, &j.right)?));
    let outer = match j.kind {
        JoinKind::Cross | JoinKind::Inner => None,
        JoinKind::Left => Some((OuterJoin::Left, j.cond.clone())),
        JoinKind::Right => Some((OuterJoin::Right, j.cond.clone())),
        JoinKind::Full => Some((OuterJoin::Full, j.cond.clone())),
        // [GR1a, GR5e] UNION: no pair of rows is a row of it.
        JoinKind::Union => Some((OuterJoin::Full, Some(Cond::Const(Truth::False)))),
    };
    let joined = match (outer, j.cond.clone()) {
        (Some((kind, cond)), _) => LogicalPlan::OuterJoin { kind, cond: cond.unwrap_or(Cond::Const(Truth::True)), left, right },
        (None, Some(cond)) => LogicalPlan::Filter { cond, input: Box::new(LogicalPlan::Product { left, right }) },
        (None, None) => LogicalPlan::Product { left, right },
    };
    let Some((_, coalesced)) = &j.common else { return Ok(joined) };
    // [GR6a] SELECT SLCC, SLT1, SLT2: here, every column, and SLCC after.
    let exprs = (0..joined.width()).map(Expr::Column).chain(coalesced.iter().cloned()).collect();
    Ok(LogicalPlan::Project { exprs, input: Box::new(joined) })
}

/// The plan of a table, view or derived table `it`.
fn leaf(binder: &Binder<'_>, it: &FromItem) -> Result<LogicalPlan> {
    let used = binder.used.borrow();
    let end = it.offset.saturating_add(it.width());
    let item_names =
        binder.names.get(it.offset..end).ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] the FROM row lost a column name"))?;
    let mut needed: Vec<usize> = used.range(it.offset..end).map(|p| p.saturating_sub(it.offset)).collect();
    Ok(match &it.source {
        Source::Table => LogicalPlan::Scan { table: it.table.clone(), names: item_names.to_vec(), needed },
        // The views' conditions read columns of their own.
        Source::Base(vb) => {
            if let Some(c) = &vb.cond {
                cond_columns(c, &mut needed);
            }
            needed.sort_unstable();
            needed.dedup();
            let scan = LogicalPlan::Scan { table: vb.base.name.clone(), names: item_names.to_vec(), needed };
            match &vb.cond {
                Some(cond) => LogicalPlan::Filter { cond: cond.clone(), input: Box::new(scan) },
                None => scan,
            }
        }
        Source::Derived(view) => view.clone(),
        Source::Common => return Err(SqlError::new(Code::Api, "[DESIGN] common columns are a join's, not a table's")),
        Source::System(table) => {
            let read = SystemRead { table: Arc::clone(table), user: binder.user.to_owned(), own: binder.own.clone() };
            LogicalPlan::System { read, names: item_names.to_vec() }
        }
    })
}
