//! [ISO/IEC 9075-2:2023 §7.6, §7.7] A `<table primary>` of a `<row pattern
//! recognition clause and name>` bound: its row pattern input table, as a
//! table primary of its own named by the row pattern input name, which
//! the row pattern partitioning and ordering alone see [§7.6 SR5]; its
//! measures and definitions over the row pattern variables' rows; and its
//! row pattern output table, named by the row pattern output name.

use std::collections::BTreeSet;

use super::common;
use super::{Recognizing, Shape};
use crate::ast::{EmptyMatches, RecognizedTable, RowPatternRecognition, RowsPerMatch as Written, ValueExpression};
use crate::binder::composite::{grouping_operand, ordering_operand};
use crate::binder::dependencies::Known;
use crate::binder::from::{Bounds, From, FromItem, bind_reference, from_plan};
use crate::binder::views::{Source, unconstrained};
use crate::binder::{Binder, Context, rule};
use crate::catalog::{ColumnDesc, QualifiedName, TableDesc};
use crate::error::{At, Code, Result, SqlError};
use crate::logical::{Expr, LogicalPlan, Recognize, RowsPerMatch, Slotted, SortKey};
use crate::value::SortOrder;
use crate::value::collation::CollationRef;

/// The row pattern input table bound: the binder of its table primary, its
/// columns, the position of each in that binder's row, and its plan, of
/// those columns in order.
struct Input<'a> {
    binder: Binder<'a>,
    columns: Vec<ColumnDesc>,
    positions: Vec<usize>,
    plan: LogicalPlan,
}

/// The row pattern partitioning and ordering bound: of each partitioning
/// column, its column of the input and its collation if it has one; of
/// each sort key, its column, its collation and its order.
struct Keys {
    partition: Vec<(usize, Option<CollationRef>)>,
    order: Vec<(usize, Option<CollationRef>, SortOrder)>,
}

/// [ISO/IEC 9075-2:2023 §7.6 SR8b i, GR1, §7.7] The table primary `r`,
/// its first column at `offset`: its row pattern output table, named by
/// its row pattern output name and derived column list if it has them.
pub(in crate::binder) fn recognized(b: Bounds<'_>, r: &RecognizedTable, offset: usize) -> Result<FromItem> {
    let clause = &r.clause;
    let input = input(b, r)?;
    let keys = keys(&input, clause)?;
    let (recognize, measured) = recognition(b.cx, &input, &keys, clause)?;
    let (exprs, mut columns) = output(&input.columns, &keys, clause.rows, measured);
    // [§7.7 SR11] of positive degree.
    if columns.is_empty() {
        return Err(rule(
            "[ISO/IEC 9075-2:2023 §7.7 SR11] the row pattern output table has a column: of ONE ROW PER MATCH, a partitioning column or a measure",
        ));
    }
    if let Some(names) = &r.columns {
        crate::binder::from::renamed(&mut columns, names)?;
    }
    let plan = LogicalPlan::Project { exprs, input: Box::new(LogicalPlan::Recognize(Box::new(recognize))) };
    // [§7.6 SR8b i 7] an implicit row pattern output name, distinct from
    // every other (UV081): none a <correlation name> can be.
    let name = QualifiedName { auth: String::new(), ident: String::new() };
    let desc = TableDesc { columns, ..unconstrained(&name) };
    let mut item = FromItem::alone(name, r.at, (desc, Source::Derived(plan)));
    item.correlation.clone_from(&r.correlation);
    item.offset = offset;
    Ok(item)
}

/// [ISO/IEC 9075-2:2023 §7.6 SR8b i 2 to 5] The row pattern input table of
/// `r`, bound as its table primary, named by its row pattern input name.
fn input<'a>(b: Bounds<'a>, r: &RecognizedTable) -> Result<Input<'a>> {
    let mut from = From { items: Vec::new(), trees: Vec::new(), used: BTreeSet::new(), lateral: Vec::new() };
    let tree = bind_reference(b, &r.input, 0, &mut from)?;
    let trees = vec![tree];
    let visible = From::visible_of(&trees, &from.items);
    let names = from.items.iter().flat_map(FromItem::row_names).collect();
    // [ISO/IEC 9075-2:2023 §6.14 SR1] none of its rows draws a value.
    let binder = Binder::new(Context { draws: false, ..b.cx }, b.outer, (from.items, names, visible.clone()), from.used);
    let (mut columns, mut positions) = (Vec::new(), Vec::new());
    for (i, c) in visible {
        let Some(item) = binder.items.get(i) else { return Err(SqlError::new(Code::Api, "[DESIGN] a column of no table primary")) };
        let Some(column) = item.desc.columns.get(c) else { return Err(SqlError::new(Code::Api, "[DESIGN] a table primary lost a column")) };
        positions.push(binder.note(item, c)?);
        // One table primary, of no join, so of no row extended with nulls.
        columns.push(ColumnDesc { default: None, generated: None, ..column.clone() });
    }
    let read = from_plan(&binder, &trees)?;
    let plan = LogicalPlan::Project { exprs: positions.iter().map(|&p| Expr::Column(p)).collect(), input: Box::new(read) };
    Ok(Input { binder, columns, positions, plan })
}

impl Input<'_> {
    /// The column of the input `v` references, if it is a column reference
    /// to one, and its type.
    fn column(&self, v: &ValueExpression) -> Result<Option<(usize, crate::value::DataType)>> {
        let ValueExpression::Column(cs) = v else { return Ok(None) };
        let Some((pos, t, _)) = self.binder.local(cs)? else { return Ok(None) };
        Ok(self.positions.iter().position(|&p| p == pos).map(|k| (k, t)))
    }
}

/// [ISO/IEC 9075-2:2023 §7.7 SR3, SR4] The row pattern partitioning
/// columns and ordering of `clause`, over `input`: each a column reference
/// to a column of the input, an operand of a grouping or ordering
/// operation, of its collation [§9.12, §9.14].
fn keys(input: &Input<'_>, clause: &RowPatternRecognition) -> Result<Keys> {
    let b = &input.binder;
    let mut partition = Vec::new();
    for g in &clause.partition {
        let column = ValueExpression::Column(g.column.clone());
        let Some((k, t)) = input.column(&column).or_at(g.column.at)? else {
            return Err(SqlError::at(
                Code::SyntaxRule,
                g.column.at,
                format!(
                    "[ISO/IEC 9075-2:2023 §7.7 SR3a ii 1] the row pattern partitioning column {} is no column of the row pattern input table",
                    g.column.column
                ),
            ));
        };
        grouping_operand(&t, "a <row pattern partition by>").or_at(g.column.at)?;
        if g.collate.is_some() && !t.is_character() {
            return Err(SqlError::at(
                Code::SyntaxRule,
                g.column.at,
                format!("[ISO/IEC 9075-2:2023 §7.7 SR3a ii 2 A] a row pattern partitioning column of {t} has no COLLATE"),
            ));
        }
        let collation = if t.is_character() { b.chosen(g.collate.as_ref(), b.derived(&column)?)? } else { None };
        partition.push((k, collation));
    }
    let mut order = Vec::new();
    for o in &clause.order {
        let Some((k, t)) = input.column(&o.key)? else {
            return Err(rule(
                "[ISO/IEC 9075-2:2023 §7.7 SR4a iii] a sort key of a <row pattern order by> is a column reference to a column of the row pattern input table",
            ));
        };
        ordering_operand(&t, "a <row pattern order by>")?;
        let collation = if t.is_character() { b.collation_of(&[b.derived(&o.key)?])? } else { None };
        order.push((k, collation, SortOrder::of(o.descending, o.nulls_first)));
    }
    Ok(Keys { partition, order })
}

/// The `Recognize` of `clause` over `input` partitioned and ordered by
/// `keys`, and its measures' columns.
fn recognition(
    cx: Context<'_>,
    input: &Input<'_>,
    keys: &Keys,
    clause: &RowPatternRecognition,
) -> Result<(Recognize<LogicalPlan>, Vec<ColumnDesc>)> {
    // [§7.9 SR3a, SR22] INITIAL, SEEK, and {- -} of WITH UNMATCHED ROWS
    // the parser refuses.
    let common = &clause.common;
    let (sorted, partition, order) = sorted(input, keys);
    let variables = common::variables(common)?;
    let width = input.columns.len();
    let shape = Shape {
        names: common::names(&variables),
        width,
        classifier: sorted.width(),
        table: true,
        partition: keys.partition.iter().map(|(k, _)| *k).collect(),
        known: Known::of(cx.db, &input.plan),
    };
    let binder = super::window::pattern_binder(cx, &input.columns, shape)?;
    let Some(r) = binder.recognizing.as_deref() else { return Err(SqlError::new(Code::Api, "[DESIGN] a row pattern binder recognizes")) };
    let defines = common::defines(&binder, r, common, &variables)?;
    let (exprs, columns) = measures(&binder, r, &clause.measures)?;
    let measures = Slotted { expr: exprs, values: r.take_slots() };
    let matching = common::matching(common, variables, defines, false)?;
    let rows = match clause.rows {
        Written::One => RowsPerMatch::One,
        Written::All(EmptyMatches::Show) => RowsPerMatch::AllShowEmpty,
        Written::All(EmptyMatches::Omit) => RowsPerMatch::AllOmitEmpty,
        Written::All(EmptyMatches::Unmatched) => RowsPerMatch::AllWithUnmatched,
    };
    let names = clause.measures.iter().map(|m| m.name.clone()).collect();
    let recognize = Recognize { partition, order, matching, measures, names, rows, input: sorted };
    super::window::no_variable_in_subquery(&recognize.matching, &recognize.measures)?;
    Ok((recognize, columns))
}

/// The input of a `Recognize` over `input`: its columns, then the sort key
/// of each partitioning column and sort key of `keys` of a collation; and
/// the column of each.
fn sorted(input: &Input<'_>, keys: &Keys) -> (LogicalPlan, Vec<usize>, Vec<SortKey>) {
    let width = input.columns.len();
    let mut exprs: Vec<Expr> = (0..width).map(Expr::Column).collect();
    let mut place = |k: usize, collation: Option<&CollationRef>| match collation {
        Some(c) => {
            exprs.push(Expr::sort_key(Expr::Column(k), c));
            exprs.len().saturating_sub(1)
        }
        None => k,
    };
    let partition = keys.partition.iter().map(|(k, c)| place(*k, c.as_ref())).collect();
    let order = keys.order.iter().map(|(k, c, o)| SortKey::new(place(*k, c.as_ref()), *o)).collect();
    let plan = if exprs.len() == width { input.plan.clone() } else { LogicalPlan::Project { exprs, input: Box::new(input.plan.clone()) } };
    (plan, partition, order)
}

/// [ISO/IEC 9075-2:2023 §7.8 SR4] The `<row pattern measure expression>`s
/// `measures` bound by `binder` with `r`, and the row pattern measure
/// column each defines.
pub(super) fn measures(binder: &Binder<'_>, r: &Recognizing, measures: &[crate::ast::Measure]) -> Result<(Vec<Expr>, Vec<ColumnDesc>)> {
    let (mut exprs, mut columns) = (Vec::new(), Vec::new());
    for m in measures {
        let (e, data_type) = binder.value_expression(&m.expr, r.scope()).or_at(m.at)?;
        let collation = if data_type.is_character() { binder.result_collation(&m.expr) } else { None };
        exprs.push(e);
        columns.push(ColumnDesc {
            name: m.name.clone(),
            data_type,
            not_null: false,
            default: None,
            domain: None,
            collation,
            generated: None,
        });
    }
    Ok((exprs, columns))
}

/// [ISO/IEC 9075-2:2023 §7.7 SR7 to SR10] The columns of the row pattern
/// output table, over a `Recognize`'s rows of an input of `columns` then
/// its measures `measured`: the partitioning columns; of ALL ROWS PER
/// MATCH, the ordering's columns; the measures; and of ALL ROWS PER MATCH,
/// the input's other columns. Each as an expression and a column.
fn output(columns: &[ColumnDesc], keys: &Keys, rows: Written, measured: Vec<ColumnDesc>) -> (Vec<Expr>, Vec<ColumnDesc>) {
    let all = matches!(rows, Written::All(_));
    let mut out: Vec<usize> = keys.partition.iter().map(|(k, _)| *k).collect();
    if all {
        out.extend(keys.order.iter().map(|(k, _, _)| *k));
    }
    let mut exprs: Vec<Expr> = out.iter().map(|&k| Expr::Column(k)).collect();
    let mut descs: Vec<ColumnDesc> = out.iter().filter_map(|&k| columns.get(k).cloned()).collect();
    let width = sorted_width(columns, keys);
    for (m, desc) in measured.into_iter().enumerate() {
        exprs.push(Expr::Column(width.saturating_add(m)));
        descs.push(desc);
    }
    if all {
        for (k, desc) in columns.iter().enumerate().filter(|(k, _)| !out.contains(k)) {
            exprs.push(Expr::Column(k));
            descs.push(desc.clone());
        }
    }
    (exprs, descs)
}

/// The width of the input of a `Recognize` over `columns` sorted by
/// `keys` (`sorted`).
fn sorted_width(columns: &[ColumnDesc], keys: &Keys) -> usize {
    let collated = keys
        .partition
        .iter()
        .filter(|(_, c)| c.is_some())
        .count()
        .saturating_add(keys.order.iter().filter(|(_, c, _)| c.is_some()).count());
    columns.len().saturating_add(collated)
}
