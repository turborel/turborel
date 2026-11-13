//! [ISO/IEC 9075-2:2023 §7.10] Partitioned join tables (Feature F403): an
//! operand of an outer join PARTITION BY some of its columns, joined with
//! the other operand partition by partition, a row of the other that
//! matches none of a partition's rows given that partition's values.
//!
//! \[DESIGN\] It is planned as an outer join of what the operands are
//! made, so it spills, reads by batches and is costed as any. The
//! partitioned operand S gives its partitions, G, its partitioning
//! columns DISTINCT [GR4b, GR4c]. The other operand, O, is joined with
//! every row of G, and S, its rows marked, is matched with the rows of
//! that product on the join's condition and on each partitioning column
//! IS NOT DISTINCT FROM G's, so each row of S matches only the rows of its
//! own partition, and a null partitions with a null. A row of O with a row
//! of G that no row of S matches is XA of that partition [GR4h], its
//! partitioning columns G's, the rest null; a row of S no row matches,
//! its mark there, is XB [GR4i]. An empty S has no partitions, so O
//! joined with none is empty too [NOTE 239, NOTE 240]. S is read twice,
//! which gives the same rows, as a derived table draws no sequence value.

use std::collections::BTreeSet;

use super::collation::of_column;
use super::from::{Bounds, From, FromItem, Tree, columns, tree_items};
use super::query::distinct;
use super::unnest::not_lateral;
use super::{Binder, OutColumn, rule};
use crate::ast::{JoinKind, JoinedTable, Partitioning};
use crate::error::{At, Result};
use crate::logical::{Cond, Expr, LogicalPlan, OuterJoin, and_all, map_cond_columns};
use crate::value::Value;

/// The join partitioning columns of an operand, bound.
pub(super) struct Partitioned {
    /// Their places among the operand's columns, in order.
    pub(super) at: Vec<usize>,
    /// Each one's FROM item and column of it.
    pub(super) of: Vec<(usize, usize)>,
    /// Their descriptors, of their partitions' columns.
    pub(super) columns: Vec<OutColumn>,
}

/// [ISO/IEC 9075-2:2023 §7.10 SR9] The partitioning of each operand of the
/// joined table `j`, `left` its first, of `width` columns from `start`,
/// and `right` its second.
///
/// # Errors
///
/// SQLCODE -103 if a partitioned operand is not of an outer join [SR9a],
/// the first of one not RIGHT or FULL [SR9c], or the second of one not
/// LEFT or FULL, or a collection derived table in it reading the first
/// [SR9d]; or if a column it names is no one column of it [SR9b].
pub(super) fn bound(
    b: Bounds<'_>,
    j: &JoinedTable,
    (left, right): (&Tree, &Tree),
    (start, width): (usize, usize),
    from: &mut From,
) -> Result<[Option<Partitioned>; 2]> {
    let [first, second] = &j.partitioned;
    let Some(written) = first.as_ref().or(second.as_ref()) else { return Ok([None, None]) };
    let (outer, preserves_left, preserves_right) = match j.kind {
        JoinKind::Left => (true, true, false),
        JoinKind::Right => (true, false, true),
        JoinKind::Full => (true, true, true),
        JoinKind::Cross | JoinKind::Inner | JoinKind::Union => (false, false, false),
    };
    if !outer {
        return Err(rule("[ISO/IEC 9075-2:2023 §7.10 SR9a] a join of a partitioned join table is an outer join")).or_at(written.at);
    }
    if let Some(p) = first.as_ref().filter(|_| !preserves_right) {
        return Err(rule("[ISO/IEC 9075-2:2023 §7.10 SR9c] a join whose first operand is partitioned is RIGHT or FULL")).or_at(p.at);
    }
    if let Some(p) = second {
        if !preserves_left {
            return Err(rule("[ISO/IEC 9075-2:2023 §7.10 SR9d] a join whose second operand is partitioned is LEFT or FULL")).or_at(p.at);
        }
        let refused = "[ISO/IEC 9075-2:2023 §7.10 SR9d] a partitioned second operand references the first in a collection derived table";
        not_lateral(right, &from.items, start..start.saturating_add(width), refused).or_at(p.at)?;
    }
    let first = first.as_ref().map(|p| partitioning(b, p, left, from)).transpose()?;
    let second = second.as_ref().map(|p| partitioning(b, p, right, from)).transpose()?;
    Ok([first, second])
}

/// [ISO/IEC 9075-2:2023 §7.10 SR9b] The columns `p` names of the operand
/// `tree`, each uniquely one of the table its `<table primary>` gives, no
/// outer reference; each is added to `from.used`.
fn partitioning(b: Bounds<'_>, p: &Partitioning, tree: &Tree, from: &mut From) -> Result<Partitioned> {
    let mut ids = Vec::new();
    tree_items(tree, &mut ids);
    ids.sort_by_key(|&i| from.items.get(i).map_or(0, |it| it.offset));
    let start = ids.iter().filter_map(|&i| from.items.get(i)).map(|it| it.offset).min().unwrap_or(0);
    let items: Vec<FromItem> = ids
        .iter()
        .filter_map(|&i| from.items.get(i))
        .map(|it| FromItem { offset: it.offset.saturating_sub(start), ..it.clone() })
        .collect();
    let local = |i: usize| ids.iter().position(|&x| x == i);
    let visible = columns(tree, &from.items).into_iter().filter_map(|(i, col)| Some((local(i)?, col))).collect();
    let names = items.iter().flat_map(FromItem::row_names).collect();
    let binder = Binder::new(b.cx, None, (items, names, visible), BTreeSet::new());
    let mut out = Partitioned { at: Vec::new(), of: Vec::new(), columns: Vec::new() };
    for cs in &p.columns {
        let found = match &cs.qualifier {
            Some(q) => binder.qualified(q, &cs.column),
            None => binder.unqualified(&cs.column),
        };
        let Ok((item, i, col)) = found else {
            let name = cs.qualifier.as_ref().map_or_else(|| cs.column.clone(), |q| format!("{}.{}", q.ident, cs.column));
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.10 SR9b] {name} is not one column of the table it partitions"))).or_at(cs.at);
        };
        out.at.push(binder.note(item, i)?);
        out.of.extend(binder.items.iter().position(|it| std::ptr::eq(it, item)).and_then(|n| ids.get(n)).map(|&n| (n, i)));
        out.columns.push(OutColumn {
            name: Some(col.name.clone()),
            collation: col.collation.clone(),
            data_type: col.data_type.clone(),
            not_null: false,
            origin: None,
        });
    }
    from.used.extend(binder.used.into_inner().into_iter().map(|c| c.saturating_add(start)));
    Ok(out)
}

/// [ISO/IEC 9075-2:2023 §7.10 SR10d] That no corresponding join column of
/// a NATURAL or USING join is a join partitioning column of `partitioned`,
/// its common columns marked among `items`.
///
/// # Errors
///
/// SQLCODE -103 if one is.
pub(super) fn not_common(partitioned: &[Option<Partitioned>; 2], items: &[FromItem], at: usize) -> Result<()> {
    for p in partitioned.iter().flatten() {
        for &(item, column) in &p.of {
            if let Some(it) = items.get(item).filter(|it| it.common.contains(&column)) {
                let name = it.desc.columns.get(column).map_or("", |c| c.name.as_str());
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §7.10 SR10d] {name} is a join partitioning column and a corresponding join column"
                )))
                .or_at(at);
            }
        }
    }
    Ok(())
}

/// Where the columns of a partitioned outer join's operands are in the
/// row of the outer join it is planned as: the first operand, its mark if
/// partitioned, the second's partitions if it is partitioned; then the
/// first's partitions if it is partitioned, the second operand, its mark
/// if partitioned.
struct Layout {
    /// The first operand's width.
    left: usize,
    /// The second operand's.
    right: usize,
    /// The number of the first's join partitioning columns, and the
    /// second's.
    parts: (usize, usize),
    /// Whether each operand is partitioned, so marked.
    marked: (bool, bool),
}

impl Layout {
    /// The first operand's mark.
    fn left_mark(&self) -> usize {
        self.left
    }

    /// The `j`-th column of the second operand's partitions.
    fn right_part(&self, j: usize) -> usize {
        self.left.saturating_add(usize::from(self.marked.0)).saturating_add(j)
    }

    /// The width of the outer join's first operand.
    fn first_width(&self) -> usize {
        self.right_part(self.parts.1)
    }

    /// The `i`-th column of the first operand's partitions.
    fn left_part(&self, i: usize) -> usize {
        self.first_width().saturating_add(i)
    }

    /// The `r`-th column of the second operand.
    fn right_column(&self, r: usize) -> usize {
        self.left_part(self.parts.0).saturating_add(r)
    }

    /// The second operand's mark.
    fn right_mark(&self) -> usize {
        self.right_column(self.right)
    }
}

/// [ISO/IEC 9075-2:2023 §7.10 GR4, GR5] The plan of the outer join `kind`
/// on `on` of `left`, its own columns from `own`, and `right`, partitioned
/// as `partitioned` says: of the joined table's columns, the first
/// operand's then the second's.
pub(super) fn plan(
    binder: &Binder<'_>,
    (kind, on): (OuterJoin, Cond),
    (left, own): (LogicalPlan, usize),
    right: LogicalPlan,
    partitioned: &[Option<Partitioned>; 2],
) -> LogicalPlan {
    let [first, second] = partitioned;
    let left_at: Vec<usize> = first.as_ref().map(|p| p.at.iter().map(|&a| a.saturating_add(own)).collect()).unwrap_or_default();
    let right_at: Vec<usize> = second.as_ref().map(|p| p.at.clone()).unwrap_or_default();
    let layout = Layout {
        left: left.width(),
        right: right.width(),
        parts: (left_at.len(), right_at.len()),
        marked: (first.is_some(), second.is_some()),
    };
    let left_parts = first.as_ref().map(|p| partitions(binder, &left, &left_at, &p.columns));
    let right_parts = second.as_ref().map(|p| partitions(binder, &right, &right_at, &p.columns));
    let first_operand = with_parts(marked(left, first.is_some()), right_parts, false);
    let second_operand = with_parts(marked(right, second.is_some()), left_parts, true);
    // The join's condition, its second operand's columns moved after the
    // first's partitions; then each partitioning column not distinct from
    // its partition's [GR4b ii 2), GR4c i 2)].
    let moved = map_cond_columns(on, &|c| if c < layout.left { c } else { layout.right_column(c.saturating_sub(layout.left)) });
    let mut conds = vec![moved];
    for (i, (&at, col)) in left_at.iter().zip(first.iter().flat_map(|p| &p.columns)).enumerate() {
        conds.push(not_distinct(binder, col, at, layout.left_part(i)));
    }
    for (j, (&at, col)) in right_at.iter().zip(second.iter().flat_map(|p| &p.columns)).enumerate() {
        conds.push(not_distinct(binder, col, layout.right_column(at), layout.right_part(j)));
    }
    let cond = and_all(conds).unwrap_or(Cond::Const(crate::logical::Truth::True));
    let joined = LogicalPlan::OuterJoin { kind, cond, left: Box::new(first_operand), right: Box::new(second_operand) };
    // [GR4h, GR4i] A partitioning column of a row the other operand's
    // unmatched row is extended with is its partition's.
    let filled = |column: usize, mark: usize, parts: &[usize], part: &dyn Fn(usize) -> usize| match parts.iter().position(|&p| p == column)
    {
        Some(n) => Expr::Case(vec![(Cond::IsNull(Expr::Column(mark), false), Expr::Column(part(n)))], Box::new(Expr::Column(column))),
        None => Expr::Column(column),
    };
    let lefts = (0..layout.left).map(|c| filled(c, layout.left_mark(), &left_at, &|n| layout.left_part(n)));
    let right_columns: Vec<usize> = right_at.iter().map(|&r| layout.right_column(r)).collect();
    let rights = (0..layout.right).map(|r| filled(layout.right_column(r), layout.right_mark(), &right_columns, &|n| layout.right_part(n)));
    LogicalPlan::Project { exprs: lefts.chain(rights).collect(), input: Box::new(joined) }
}

/// [ISO/IEC 9075-2:2023 §7.10 GR4b ii 2), GR4c i 2)] The partitions of
/// `operand` by its columns `at`, of `columns`: their values DISTINCT, so
/// no two values of one partition are distinct, by each column's
/// collation.
fn partitions(binder: &Binder<'_>, operand: &LogicalPlan, at: &[usize], columns: &[OutColumn]) -> LogicalPlan {
    let projected = LogicalPlan::Project { exprs: at.iter().map(|&c| Expr::Column(c)).collect(), input: Box::new(operand.clone()) };
    distinct(projected, (binder.db, binder.session_collation), columns)
}

/// `operand`, with a column after its own that is never null if `mark`:
/// whether a row of the outer join is of a row of it.
fn marked(operand: LogicalPlan, mark: bool) -> LogicalPlan {
    if !mark {
        return operand;
    }
    let exprs = (0..operand.width()).map(Expr::Column).chain([Expr::Const(Value::Bool(true))]).collect();
    LogicalPlan::Project { exprs, input: Box::new(operand) }
}

/// `operand` joined with every row of `parts`, if given: after it, or
/// before it if `before`.
fn with_parts(operand: LogicalPlan, parts: Option<LogicalPlan>, before: bool) -> LogicalPlan {
    let Some(p) = parts else { return operand };
    let (left, right) = if before { (p, operand) } else { (operand, p) };
    LogicalPlan::Product { left: Box::new(left), right: Box::new(right) }
}

/// [ISO/IEC 9075-2:2023 §8.15] Column `own`, of the partitioning column
/// `col`, NOT DISTINCT FROM `part`, its partition's: by its collation's
/// sort keys if it is a character string of one.
fn not_distinct(binder: &Binder<'_>, col: &OutColumn, own: usize, part: usize) -> Cond {
    let keyed = |c: usize| match of_column(binder.db, binder.session_collation, col) {
        Some(collation) => Expr::sort_key(Expr::Column(c), &collation),
        None => Expr::Column(c),
    };
    Cond::Not(Box::new(Cond::Distinct(keyed(own), keyed(part))))
}
