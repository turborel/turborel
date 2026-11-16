//! [ISO/IEC 9075-2:2023 §9.18 SR12b to SR12d] The `<table argument>`s of
//! an invocation of a polymorphic table function: each one's row type and
//! range variables, its partitioning and ordering columns found among the
//! columns of its range variables, whether it is pruned, and its
//! `<co-partition specification>`; its rows, each with the keys its
//! partitioning compares and its ordering sorts by after its columns.

use super::super::collation::of_column;
use super::super::composite::{grouping_operand, ordering_operand};
use super::super::from::{Bounds, column_of};
use super::super::query::Nesting;
use super::super::set_operations::bind_query_expression;
use super::super::{Context, OutColumn, qualify, rule};
use super::{BoundPtf, Range, bound_ptf};
use crate::ast::{
    ColumnSpecification, GenericTable, NamedTable, QueryExpression, QuerySpecification, SelectList, TableArgument, TableArgumentProper,
    TableExpression, TableName, TableReference, TableSemantics,
};
use crate::catalog::{ColumnDesc, QualifiedName};
use crate::db::ptf::TableInput;
use crate::error::{At, Result, SqlError};
use crate::logical::{Expr, LogicalPlan, SortKey};
use crate::value::SortOrder;

/// A `<table argument>`, bound.
pub(super) struct BoundTable {
    /// Its rows, each of its row type's columns then its keys.
    pub(super) plan: LogicalPlan,
    /// Its row type's columns [SR12b ii].
    pub(super) columns: Vec<ColumnDesc>,
    /// Its range variables.
    pub(super) ranges: Vec<Range>,
    /// What its execution reads of it.
    pub(super) input: TableInput,
}

/// An exception of [ISO/IEC 9075-2:2023 §9.18] `rule_of`.
fn broken(rule_of: &str, msg: &str) -> SqlError {
    rule(format!("[ISO/IEC 9075-2:2023 §9.18 {rule_of}] {msg}"))
}

/// SELECT * FROM `table`: the rows of a `<table or query name>` [SR12b ii
/// 2 A I].
fn all_of(table: &NamedTable) -> QueryExpression {
    let from = vec![TableReference::Table(table.clone())];
    let table_expression = TableExpression { from, where_clause: None, group_by: None, having: None, window: Vec::new() };
    QueryExpression::Spec(Box::new(QuerySpecification { distinct: false, select_list: SelectList::Star, table_expression }))
}

/// [ISO/IEC 9075-2:2023 §9.18 SR12b] The `<table argument>` `ta` of the
/// generic table parameter `g`, bound in `b`, as an argument of an
/// invocation nested `depth` deep.
pub(super) fn table_argument(b: Bounds<'_>, ta: &TableArgument, g: GenericTable, depth: usize) -> Result<BoundTable> {
    let (plan, columns, ranges) = match &ta.proper {
        TableArgumentProper::Invocation(i) => {
            // [SR12b ii 1] TABLE ( PTFI ) AS TACN (TAPDCL): its row type
            // and range variables.
            let BoundPtf { plan, columns, ranges } = bound_ptf(b, (i, ta.correlation.as_deref(), ta.columns.as_deref()), depth)?;
            (plan, columns, ranges)
        }
        TableArgumentProper::Named(n) => {
            let query = all_of(n);
            let range =
                Range::named(ta.correlation.as_deref().map_or_else(|| Some(qualify(&n.table, b.cx.schema)), |_| None), ta, n.table.at);
            queried(b, (&query, ta), range)?
        }
        TableArgumentProper::Query(q) => queried(b, (&q.query, ta), Range::named(None, ta, q.at))?,
    };
    let mut bound = BoundTable { plan, columns, ranges, input: unpartitioned(g) };
    partitioned(b.cx, &mut bound, ta, g).or_at(ta.at)?;
    Ok(bound)
}

/// A table argument of `g` as yet of no partitioning, ordering or rows.
fn unpartitioned(g: GenericTable) -> TableInput {
    TableInput {
        columns: Vec::new(),
        partition: Vec::new(),
        keys: Vec::new(),
        order: Vec::new(),
        sort: Vec::new(),
        pass_through: g.pass_through,
        semantics: g.semantics,
        pruned: matches!(g.semantics, TableSemantics::Set { prune: true }),
        copartitioned: None,
    }
}

/// [SR12b ii 2] The rows, row type and range variable `range` of the
/// `<table argument>` `ta` whose proper is the query `query`: its columns
/// renamed by its `<table argument parenthesized derived column list>`
/// [SR12b ii 2 D].
fn queried(
    b: Bounds<'_>,
    (query, ta): (&QueryExpression, &TableArgument),
    mut range: Range,
) -> Result<(LogicalPlan, Vec<ColumnDesc>, Vec<Range>)> {
    let nesting = match b.outer {
        Some(outer) => Nesting::Subquery { outer, result: None },
        None => Nesting::Query,
    };
    let bound = bind_query_expression(Context { draws: false, ..b.cx }, query, &nesting).or_at(ta.at)?;
    let mut columns: Vec<ColumnDesc> = bound.columns.iter().enumerate().map(|(k, c)| column_of(k, c)).collect();
    if let Some(names) = &ta.columns {
        if names.len() != columns.len() {
            let msg = format!("{} column names for the {} columns of the table argument", names.len(), columns.len());
            return Err(broken("SR12b ii 2 D I 1", &msg).or_at(ta.at));
        }
        for (k, name) in names.iter().enumerate() {
            if names.iter().take(k).any(|n| n == name) {
                return Err(broken("SR12b ii 2 D I 2", &format!("column {name} is named twice")).or_at(ta.at));
            }
        }
        for (c, name) in columns.iter_mut().zip(names) {
            c.name.clone_from(name);
        }
    }
    range.columns = (0..columns.len()).collect();
    Ok((bound.plan, columns, vec![range]))
}

/// [SR12b iii, SR12b vi] The column `spec` of a partitioning or ordering
/// of `bound` references: of its range variable, or of the one of them
/// alone with a column of the name, by its place among its columns.
fn column(bound: &BoundTable, spec: &ColumnSpecification, schema: &str, rule_of: &str) -> Result<usize> {
    let named = |r: &&Range| spec.qualifier.as_ref().is_none_or(|q| r.designated_by(q, schema));
    let found: Vec<usize> = bound
        .ranges
        .iter()
        .filter(named)
        .flat_map(|r| r.columns.iter().copied())
        .filter(|&c| bound.columns.get(c).is_some_and(|d| d.name == spec.column))
        .collect();
    match found.as_slice() {
        [c] if spec.catalog.is_none() => Ok(*c),
        [] | [_] => Err(broken(rule_of, &format!("column {} is no column of a range variable of the table argument", spec.column))),
        [_, _, ..] => Err(broken(rule_of, &format!("column {} is ambiguous among the range variables of the table argument", spec.column))),
    }
}

/// [SR12b iii 1, SR12b iv, SR12b vi 1] That `ta`, an argument of `g`, is
/// partitioned and ordered only if `g` is of set semantics, and pruned or
/// kept only if `g` is of KEEP ON EMPTY.
fn semantics_allow(ta: &TableArgument, g: GenericTable) -> Result<()> {
    let set = matches!(g.semantics, TableSemantics::Set { .. });
    let keep = matches!(g.semantics, TableSemantics::Set { prune: false });
    if ta.partition.is_some() && !set {
        return Err(broken("SR12b iii 1", "a table argument of a parameter of row semantics has no PARTITION BY"));
    }
    if ta.prune.is_some() && !keep {
        return Err(broken("SR12b iv", "a table argument's PRUNE or KEEP WHEN EMPTY is of a parameter of set semantics KEEP WHEN EMPTY"));
    }
    if !ta.order.is_empty() && !set {
        return Err(broken("SR12b vi 1", "a table argument of a parameter of row semantics has no ORDER BY"));
    }
    Ok(())
}

/// The place among `exprs` of what the column `c` of `columns` is compared
/// by, bound in `cx`: of a collation, its sort key, added to `exprs`; else
/// the column itself.
fn keyed(cx: Context<'_>, columns: &[ColumnDesc], c: usize, exprs: &mut Vec<Expr>) -> usize {
    let Some(d) = columns.get(c) else { return c };
    let out = OutColumn {
        name: Some(d.name.clone()),
        collation: d.collation.clone(),
        data_type: d.data_type.clone(),
        not_null: d.not_null,
        origin: None,
    };
    match of_column(cx.db, cx.collation, &out) {
        Some(collation) => {
            exprs.push(Expr::sort_key(Expr::Column(c), &collation));
            exprs.len().saturating_sub(1)
        }
        None => c,
    }
}

/// [SR12b iii to SR12b vi] The partitioning, pruning and ordering of `ta`,
/// an argument of `g`, made those of `bound`, and its rows each with the
/// keys they need after its columns: of a partitioning or ordering column
/// of a collation, its sort key.
fn partitioned(cx: Context<'_>, bound: &mut BoundTable, ta: &TableArgument, g: GenericTable) -> Result<()> {
    semantics_allow(ta, g)?;
    let width = bound.columns.len();
    let mut exprs: Vec<Expr> = (0..width).map(Expr::Column).collect();
    let mut input = unpartitioned(g);
    for spec in ta.partition.iter().flatten() {
        let c = column(bound, spec, cx.schema, "SR12b iii 2")?;
        if let Some(d) = bound.columns.get(c) {
            grouping_operand(&d.data_type, &format!("partitioning column {}", d.name))?;
        }
        if input.partition.contains(&c) {
            return Err(broken("SR12b iii 2", &format!("partitioning column {} is named twice", spec.column)));
        }
        input.partition.push(c);
        input.keys.push(keyed(cx, &bound.columns, c, &mut exprs));
    }
    for (spec, descending, nulls) in &ta.order {
        let c = column(bound, spec, cx.schema, "SR12b vi 2")?;
        if let Some(d) = bound.columns.get(c) {
            ordering_operand(&d.data_type, &format!("ordering column {}", d.name))?;
        }
        let order = SortOrder::of(*descending, *nulls);
        input.order.push((c, order.descending, order.nulls_first));
        input.sort.push(SortKey::new(keyed(cx, &bound.columns, c, &mut exprs), order));
    }
    input.pruned = input.pruned || ta.prune == Some(true);
    input.columns = bound.columns.iter().map(|c| (c.name.clone(), c.data_type.clone())).collect();
    if exprs.len() > width {
        let rows = std::mem::replace(&mut bound.plan, LogicalPlan::Values { rows: Vec::new(), types: Vec::new() });
        bound.plan = LogicalPlan::Project { exprs, input: Box::new(rows) };
    }
    bound.input = input;
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §9.18 SR12c, SR12d] That no two range variables
/// of the table arguments `tables` are alike, and the `<co-partition
/// clause>` `copartition` names each table argument once at most, of as
/// many partitioning columns, at least one, each comparable with its
/// counterparts: each one's `<co-partition specification>` noted.
pub(super) fn copartitioned(tables: &mut [BoundTable], copartition: &[Vec<(TableName, usize)>], schema: &str) -> Result<()> {
    let names: Vec<(QualifiedName, usize)> =
        tables.iter().flat_map(|t| t.ranges.iter()).filter_map(|r| r.name().map(|n| (n, r.at))).collect();
    for (k, (name, at)) in names.iter().enumerate() {
        if names.iter().take(k).any(|(n, _)| n == name) {
            return Err(broken("SR12c", &format!("range variable {} is of two table arguments", name.ident)).or_at(*at));
        }
    }
    let mut seen: Vec<usize> = Vec::new();
    for (s, spec) in copartition.iter().enumerate() {
        let mut members: Vec<usize> = Vec::with_capacity(spec.len());
        for (name, at) in spec {
            let found = tables.iter().position(|t| t.ranges.iter().any(|r| r.designated_by(name, schema)));
            let Some(t) = found else {
                return Err(broken("SR12d ii 2", &format!("{} is no range variable of a table argument", name.ident)).or_at(*at));
            };
            if seen.contains(&t) {
                return Err(broken("SR12d i, iii", &format!("the table argument of {} is co-partitioned twice", name.ident)).or_at(*at));
            }
            seen.push(t);
            members.push(t);
        }
        let first = members.first().and_then(|&t| tables.get(t)).map(|t| t.input.partition.clone()).unwrap_or_default();
        for &t in &members {
            let Some(table) = tables.get(t) else { continue };
            if first.is_empty() || table.input.partition.len() != first.len() {
                return Err(broken("SR12d ii 3", "co-partitioned table arguments have as many partitioning columns, at least one"));
            }
            let lead = members.first().and_then(|&m| tables.get(m));
            let comparable = table.input.partition.iter().zip(&first).all(|(&c, &f)| {
                let mine = table.columns.get(c).map(|d| &d.data_type);
                let theirs = lead.and_then(|l| l.columns.get(f)).map(|d| &d.data_type);
                mine.zip(theirs).is_some_and(|(a, b)| a.comparable(b))
            });
            if !comparable {
                return Err(broken("SR12d ii 4", "corresponding partitioning columns of co-partitioned table arguments are comparable"));
            }
        }
        for &t in &members {
            if let Some(table) = tables.get_mut(t) {
                table.input.copartitioned = Some(s);
            }
        }
    }
    Ok(())
}
