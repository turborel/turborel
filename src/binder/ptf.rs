//! [ISO/IEC 9075-2:2023 §7.6 SR7, SR12a, SR15a; §9.18 SR12] A `<PTF
//! derived table>` in a `<from clause>`: the invocation of a polymorphic
//! table function bound and compiled (`call.rs`), its table arguments
//! bound (`tables.rs`); its range variables, exposed, each an item of the
//! FROM row, together a group whose first item's plan gives them all.
//!
//! Its columns are its proper result columns, of its correlation name;
//! then of each table argument, of PASS THROUGH its columns, else its
//! partitioning columns, each of the range variable of the argument whose
//! column it is.

mod call;
mod graph;
mod tables;

use std::sync::Arc;

use super::from::{Bounds, From, FromItem, Tree};
use super::views::{Source, unconstrained};
use super::{qualify, rule};
use crate::ast::{PtfInvocation, PtfTable, TableArgument, TableName};
use crate::catalog::{ColumnDesc, QualifiedName, TableDesc};
use crate::db::State;
use crate::db::ptf::Invoked;
use crate::error::{At, Code, Result, SqlError};
use crate::exec::{Subqueries, SubqueryRows};
use crate::logical::{Expr, LogicalPlan, Polymorphic, Subquery};
use crate::txn::TxId;
use crate::value::Value;

/// [§7.6, §9.18 SR12b ii 1] How deep one `<PTF derived table>` may invoke
/// polymorphic table functions in its table arguments, each in the one
/// before.
const MAX_NESTED_INVOCATIONS: usize = 32;

/// [ISO/IEC 9075-2:2023 §9.24] The database and transaction of a
/// statement that runs: where a PTF describe component procedure reads
/// SQL-data as the statement is bound.
#[derive(Clone)]
pub struct Compile {
    /// The database the statement runs against.
    pub db: Arc<State>,
    /// The transaction it runs in.
    pub tx: TxId,
}

/// \[DESIGN\] Where no statement runs, as a definition is bound: a PTF
/// describe component procedure that reads SQL-data reads none there.
struct Undescribed;

impl Subqueries for Undescribed {
    fn rows(&self, _: &Subquery, _: &[Value], _: Option<usize>) -> Result<SubqueryRows> {
        Err(SqlError::new(
            Code::Unsupported,
            "[DESIGN] a PTF describe component procedure reads SQL-data as a definition is bound, where no statement runs: it is valid SQL but is not implemented",
        ))
    }
}

/// [ISO/IEC 9075-2:2023 §9.18 SR12b ii, §7.6 SR15a] A range variable of a
/// `<table argument>` or a `<PTF derived table>`, and its associated
/// column list, by the columns' places among those of the table it
/// ranges over.
#[derive(Debug, Clone)]
pub(super) struct Range {
    /// Its `<correlation name>`, if it is one.
    pub(super) correlation: Option<String>,
    /// Else the `<table or query name>` it is, if it is one; of neither,
    /// it is implementation-dependent (UV095), and named by nothing.
    pub(super) table: Option<QualifiedName>,
    /// Where it is written.
    pub(super) at: usize,
    /// Its associated column list.
    pub(super) columns: Vec<usize>,
}

impl Range {
    /// The range variable of `ta`: its correlation name, else `table`.
    pub(super) fn named(table: Option<QualifiedName>, ta: &TableArgument, at: usize) -> Range {
        Range { correlation: ta.correlation.clone(), table: if ta.correlation.is_some() { None } else { table }, at, columns: Vec::new() }
    }

    /// Its name, if it has one.
    pub(super) fn name(&self) -> Option<QualifiedName> {
        match &self.correlation {
            Some(c) => Some(QualifiedName { auth: String::new(), ident: c.clone() }),
            None => self.table.clone(),
        }
    }

    /// Whether the `<qualifier>` `q`, of the schema `schema` if written
    /// without one, names it.
    pub(super) fn designated_by(&self, q: &TableName, schema: &str) -> bool {
        match (&self.correlation, &self.table) {
            (Some(c), _) => q.auth.is_none() && q.ident == *c,
            (None, Some(t)) => qualify(q, schema) == *t,
            (None, None) => false,
        }
    }
}

/// A `<PTF derived table>`, bound: its rows, its columns, and its range
/// variables, each of a run of its columns, in order.
pub(super) struct BoundPtf {
    /// Its rows.
    pub(super) plan: LogicalPlan,
    /// Its columns.
    pub(super) columns: Vec<ColumnDesc>,
    /// Its range variables: its correlation name's first, unless it
    /// returns only pass-through columns, then its table arguments'.
    pub(super) ranges: Vec<Range>,
}

/// A column of a `<PTF derived table>`, possibly nullable [§7.6 SR12a iv
/// 1], of no default.
fn column(name: String, data_type: crate::value::DataType) -> ColumnDesc {
    ColumnDesc { name, data_type, not_null: false, default: None, domain: None, collation: None, generated: None }
}

/// [ISO/IEC 9075-2:2023 §7.6 SR7d, SR12a, SR15a] The `<PTF derived table>`
/// TABLE ( `inv` ), of the correlation name `correlation` and derived
/// column list `names` if written, bound in `b`, nested `depth` deep.
///
/// # Errors
///
/// SQLCODE -103 if it nests past `MAX_NESTED_INVOCATIONS`, has a
/// correlation name and returns only pass-through columns or has none and
/// returns proper result columns, its correlation name is a table
/// argument's range variable, or its derived column list is not one name
/// for each proper result column, none twice; its invocation's.
pub(super) fn bound_ptf(
    b: Bounds<'_>,
    (inv, correlation, names): (&PtfInvocation, Option<&str>, Option<&[String]>),
    depth: usize,
) -> Result<BoundPtf> {
    if graph::is_graph_algorithm(inv) {
        return graph::bound_graph_algorithm(b, (inv, correlation, names), depth);
    }
    if depth > MAX_NESTED_INVOCATIONS {
        let msg =
            format!("[DESIGN] a <PTF derived table> nests more than {MAX_NESTED_INVOCATIONS} invocations of polymorphic table functions");
        return Err(SqlError::new(Code::Unsupported, msg).or_at(inv.at));
    }
    let call::Bound { desc, call, tables, arguments } = call::bound(b, inv, depth.saturating_add(1))?;
    let only = desc.ptf.as_ref().is_some_and(|p| p.only_pass_through);
    let mut columns: Vec<ColumnDesc> = call.compiled.proper.iter().map(|(n, t)| column(n.clone(), t.clone())).collect();
    let mut ranges = named(b, (only, correlation, names, inv.at), &tables, &mut columns).or_at(inv.at)?;
    let (node, order) = derived(&tables, &mut columns, &mut ranges);
    let node_names: Vec<String> = node.iter().map(|c| c.name.clone()).collect();
    let inputs = tables.into_iter().map(|t| t.plan).collect();
    let call = Arc::new(Invoked::Routine(Box::new(call)));
    let mut plan = LogicalPlan::Polymorphic(Box::new(Polymorphic { call, arguments, inputs, names: node_names }));
    if order.iter().enumerate().any(|(k, &c)| k != c) || order.len() != node.len() {
        plan = LogicalPlan::Project { exprs: order.into_iter().map(Expr::Column).collect(), input: Box::new(plan) };
    }
    Ok(BoundPtf { plan, columns, ranges })
}

/// [§7.6 SR7d] The proper result columns `proper` of a `<PTF derived
/// table>` of the table arguments `tables`, of RETURNS ONLY PASS THROUGH if
/// `only`, named by its correlation name `correlation` and derived column
/// list `names`, written at `at`: none if it returns only pass-through
/// columns; its range variable of them, if it has one.
fn named(
    b: Bounds<'_>,
    (only, correlation, names, at): (bool, Option<&str>, Option<&[String]>, usize),
    tables: &[tables::BoundTable],
    proper: &mut Vec<ColumnDesc>,
) -> Result<Vec<Range>> {
    let c = match (only, correlation) {
        (true, Some(_)) => {
            return Err(rule(
                "[ISO/IEC 9075-2:2023 §7.6 SR7d i] a polymorphic table function of RETURNS ONLY PASS THROUGH has no correlation name",
            ));
        }
        (false, None) => return Err(rule("[ISO/IEC 9075-2:2023 §7.6 SR7d ii] a <PTF derived table> has a correlation name")),
        (true, None) => {
            proper.clear();
            return Ok(Vec::new());
        }
        (false, Some(c)) => c,
    };
    let designated = |q: &TableName| tables.iter().flat_map(|t| t.ranges.iter()).any(|r| r.designated_by(q, b.cx.schema));
    if designated(&TableName { auth: None, ident: c.to_owned(), at }) {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.6 SR7d ii 1] correlation name {c} is a range variable of a table argument")));
    }
    if let Some(names) = names {
        renamed(proper, names)?;
    }
    for (k, p) in proper.iter().enumerate() {
        if proper.iter().take(k).any(|q| q.name == p.name) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.6 SR7d ii 4] the <PTF derived table> has two columns {}", p.name)));
        }
    }
    Ok(vec![Range { correlation: Some(c.to_owned()), table: None, at, columns: (0..proper.len()).collect() }])
}

/// [§7.6 SR12a iv 2, SR15a ii] Of the table arguments `tables`, the
/// columns each gives a `<PTF derived table>` whose proper result columns
/// `columns` holds: of PASS THROUGH all its columns, else its partitioning
/// columns, in its order. Gives the columns of the node that holds them
/// after the proper ones, and the place among them of each of
/// `columns`, which takes those of each range variable, each one of
/// `ranges` too.
fn derived(tables: &[tables::BoundTable], columns: &mut Vec<ColumnDesc>, ranges: &mut Vec<Range>) -> (Vec<ColumnDesc>, Vec<usize>) {
    let mut node: Vec<ColumnDesc> = columns.clone();
    let mut order: Vec<usize> = (0..columns.len()).collect();
    for t in tables {
        let own: Vec<usize> = if t.input.pass_through {
            (0..t.columns.len()).collect()
        } else {
            let mut p = t.input.partition.clone();
            p.sort_unstable();
            p
        };
        let base = node.len();
        node.extend(own.iter().filter_map(|&c| t.columns.get(c)).map(|c| column(c.name.clone(), c.data_type.clone())));
        for r in &t.ranges {
            let mut of_range = Vec::new();
            for c in r.columns.iter().filter_map(|c| own.iter().position(|d| d == c)) {
                of_range.push(columns.len());
                order.push(base.saturating_add(c));
                columns.extend(node.get(base.saturating_add(c)).cloned());
            }
            ranges.push(Range { columns: of_range, ..r.clone() });
        }
    }
    (node, order)
}

/// [§7.6 SR7d ii 3 A] The proper result columns `proper` named by the
/// derived column list `names`.
fn renamed(proper: &mut [ColumnDesc], names: &[String]) -> Result<()> {
    if names.len() != proper.len() {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §7.6 SR7d ii 3 A I] {} column names for the {} proper result columns",
            names.len(),
            proper.len()
        )));
    }
    for (c, name) in proper.iter_mut().zip(names) {
        c.name.clone_from(name);
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §7.6 SR15a] The `<PTF derived table>` `t`, its
/// first column at `start` of the FROM row: an item for each of its range
/// variables, the first of them holding the plan of its rows.
pub(super) fn ptf_reference(b: Bounds<'_>, t: &PtfTable, start: usize, from: &mut From) -> Result<Tree> {
    let bound = bound_ptf(b, (&t.invocation, t.correlation.as_deref(), t.columns.as_deref()), 0).or_at(t.at)?;
    let first_named = bound.ranges.first().is_some_and(|r| r.correlation.is_some() && t.correlation.is_some());
    let mut plan = Some(bound.plan);
    let mut ids = Vec::new();
    let mut offset = start;
    let item = |r: Option<&Range>, columns: Vec<ColumnDesc>, first: usize, source: Source| {
        let table = r.and_then(Range::name).unwrap_or_else(|| QualifiedName { auth: String::new(), ident: String::new() });
        let desc = TableDesc { columns, ..unconstrained(&table) };
        FromItem {
            table,
            at: r.map_or(t.at, |r| r.at),
            correlation: r.and_then(|r| r.correlation.clone()),
            desc,
            offset: first,
            source,
            nullable: false,
            common: Vec::new(),
            drawn: None,
            family: Vec::new(),
        }
    };
    if !first_named {
        // [§7.6 SR7d i] Of RETURNS ONLY PASS THROUGH, no correlation name:
        // an item of no name and no column holds the plan.
        let source = Source::Derived(plan.take().unwrap_or(LogicalPlan::Values { rows: Vec::new(), types: Vec::new() }));
        from.items.push(item(None, Vec::new(), offset, source));
        ids.push(from.items.len().saturating_sub(1));
    }
    for r in &bound.ranges {
        let columns: Vec<ColumnDesc> = r.columns.iter().filter_map(|&c| bound.columns.get(c)).cloned().collect();
        let width = columns.len();
        let source = match plan.take() {
            Some(p) => Source::Derived(p),
            None => Source::Argument,
        };
        from.items.push(item(Some(r), columns, offset, source));
        ids.push(from.items.len().saturating_sub(1));
        offset = offset.saturating_add(width);
    }
    Ok(Tree::Group(ids))
}
