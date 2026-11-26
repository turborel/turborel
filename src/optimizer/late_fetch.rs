//! \[DESIGN\] The columns a grouping carries of a table joined on its key,
//! fetched after FETCH FIRST (RULES.md D1202). A grouping carries the
//! value of each column its grouping columns determine
//! (`optimizer/dependent_keys.rs`, `binder/invariant.rs`). Where a table
//! below the grouping has a key the grouping columns determine too, every
//! row of a group is of one row of that table, so the group's value of
//! any of the table's columns is that row's. Of a query that keeps a few
//! of its groups, the first rows of an order, those columns are then read
//! of the table again, by its key, for the groups kept alone, and the
//! joins and the grouping below carry the key in their place. Of
//! `SELECT C.KEY, C.NAME, C.ADDRESS, SUM(...) ... GROUP BY C.KEY, C.NAME,
//! C.ADDRESS ORDER BY 4 DESC FETCH FIRST 20 ROWS ONLY`, twenty names and
//! addresses are read.
//!
//! The grouping, its groups and what each set function is given are as
//! they were, so nothing is evaluated of a row or group it was not (RULES.md
//! D15). The table stays joined below, for the rows it leaves out, its
//! scan needing the columns fetched no more. A group
//! has one value of the table's key, not null, as the key's columns are
//! NOT NULL and the row was joined by an inner join, so the join above
//! finds one row for each group kept, the one its rows were of, and
//! neither adds nor loses a row. It may give its rows in another order, so
//! the rows are sorted again above it by the same keys.
//!
//! A column is fetched so only if nothing between the grouping and the
//! FETCH FIRST reads it but to hand it on. A sort key, a HAVING condition
//! or an expression of the select list that reads it needs it of every
//! group, and it stays carried. The count is a constant of FETCH FIRST ... ROWS ONLY.
//!
//! Fetching n rows of a table of r rows by its key reads n log2(r) keys of
//! a tree of them, or the r rows once more, whichever is fewer. Carrying a
//! column reads it once for each row the grouping groups. A column is
//! fetched where the fetch so counted reads fewer rows than the grouping
//! is estimated to group.

use super::dependent_keys::by_own;
use super::{Rule, RuleContext};
use crate::binder::{Known, group_value};
use crate::functions::carried_column;
use crate::logical::{
    AggCall, Cond, Expr, Limit, LogicalPlan, SortKey, and_all, cond_columns, expr_columns, map_cond_columns, map_expr_columns,
};
use crate::physical::estimated_rows;
use crate::storage::cost_of;
use crate::value::{CompOp, DataType};

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const FETCH_DEPENDENTS_LATE: &str = "fetch_dependents_late";

/// The columns a grouping carries of a table joined on its key, fetched
/// after FETCH FIRST, as the module says.
pub struct FetchDependentsLate;

/// What is between a grouping and the sort over it: a select list, or a
/// HAVING condition.
#[derive(Clone, Copy)]
enum Step<'a> {
    List(&'a [Expr]),
    Having(&'a Cond),
}

/// A FETCH FIRST over a grouping: its counts, the keys of the sort
/// between them, if any, the select lists and HAVING conditions between,
/// the uppermost first, and the grouping's parts.
struct Shape<'a> {
    limit: &'a Limit,
    keys: Option<&'a [SortKey]>,
    steps: Vec<Step<'a>>,
    groups: &'a [usize],
    aggs: &'a [AggCall],
    rows: &'a LogicalPlan,
}

/// A table read below the grouping: its scan, the place of its first
/// column among the grouping's input's, and its key's columns with their
/// types.
struct Leaf<'a> {
    scan: &'a LogicalPlan,
    base: usize,
    key: Vec<(usize, DataType)>,
}

/// A column fetched late: the set function that carried it, its table
/// among the leaves, and its place among the table's columns.
#[derive(Clone, Copy)]
struct Late {
    agg: usize,
    leaf: usize,
    column: usize,
}

/// What is built of a plan's columns as the select lists are rebuilt:
/// each column's new place, or the set function whose column it hands on
/// if that is fetched late, and where each leaf's key columns are.
struct Places {
    at: Vec<Result<usize, usize>>,
    keys: Vec<Vec<usize>>,
}

impl Rule for FetchDependentsLate {
    fn name(&self) -> &'static str {
        FETCH_DEPENDENTS_LATE
    }
    fn apply(&self, plan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let shape = shape_of(plan)?;
        let kept = cost_of(shape.limit.most()?);
        let (leaves, mut late) = candidates(&shape, kept, ctx);
        // Each turn fetches one column fewer, the one something reads.
        for _ in 0..shape.aggs.len() {
            if late.is_empty() {
                return None;
            }
            match rebuilt(&shape, &leaves, &late) {
                Ok(fetched) => return Some(fetched),
                Err(read) => late.retain(|l| l.agg != read),
            }
        }
        None
    }
}

/// The shape of `plan`, if it is a FETCH FIRST over a grouping of no
/// group made before any row, a sort and select lists alone between them.
fn shape_of(plan: &LogicalPlan) -> Option<Shape<'_>> {
    let LogicalPlan::Limit { limit, input } = plan else { return None };
    let (keys, mut at) =
        if let LogicalPlan::Sort { keys, input: sorted } = &**input { (Some(keys.as_slice()), &**sorted) } else { (None, &**input) };
    let mut steps = Vec::new();
    loop {
        match at {
            LogicalPlan::Project { exprs, input: below } => {
                steps.push(Step::List(exprs));
                at = below;
            }
            LogicalPlan::Filter { cond, input: below } => {
                steps.push(Step::Having(cond));
                at = below;
            }
            LogicalPlan::Scan { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Product { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::OuterJoin { .. }
            | LogicalPlan::SemiJoin { .. }
            | LogicalPlan::MarkJoin(_)
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::Sort { .. }
            | LogicalPlan::Limit { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::WorkTable { .. }
            | LogicalPlan::Transition { .. }
            | LogicalPlan::Unnest(..)
            | LogicalPlan::Window(..)
            | LogicalPlan::Recognize(..)
            | LogicalPlan::Polymorphic(..) => break,
        }
    }
    let LogicalPlan::Aggregate { groups, collations, aggs, seeds, input: rows } = at else { return None };
    let simple = seeds.is_empty() && collations.is_empty();
    simple.then_some(Shape { limit, keys, steps, groups, aggs, rows })
}

/// What is passed on the way from a plan down to one of its scans: a
/// filter's condition, with the place of its input's first column, or a
/// product, of which the other operand and whether the way is its left.
enum Passed<'a> {
    Filter(&'a Cond, usize),
    Product(&'a LogicalPlan, bool),
}

/// The scan of a table that gives column `c` of `plan`, through filters
/// and products alone, which keep each row of the table whole beside the
/// others'; the place of the table's first column; and what is passed on
/// the way to it.
fn way_to(plan: &LogicalPlan, c: usize) -> Option<(&LogicalPlan, usize, Vec<Passed<'_>>)> {
    let (mut at, mut base, mut passed) = (plan, 0_usize, Vec::new());
    loop {
        match at {
            LogicalPlan::Filter { cond, input } => {
                passed.push(Passed::Filter(cond, base));
                at = input;
            }
            LogicalPlan::Product { left, right } => {
                let width = left.width();
                let of_left = c.checked_sub(base)? < width;
                passed.push(Passed::Product(if of_left { right } else { left }, of_left));
                at = if of_left { left } else { right };
                base = if of_left { base } else { base.saturating_add(width) };
            }
            LogicalPlan::Scan { located: false, .. } => return Some((at, base, passed)),
            LogicalPlan::Scan { located: true, .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Project { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::OuterJoin { .. }
            | LogicalPlan::SemiJoin { .. }
            | LogicalPlan::MarkJoin(_)
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::System { .. }
            | LogicalPlan::Values { .. }
            | LogicalPlan::Sort { .. }
            | LogicalPlan::Limit { .. }
            | LogicalPlan::Recursive { .. }
            | LogicalPlan::WorkTable { .. }
            | LogicalPlan::Transition { .. }
            | LogicalPlan::Unnest(..)
            | LogicalPlan::Window(..)
            | LogicalPlan::Recognize(..)
            | LogicalPlan::Polymorphic(..) => return None,
        }
    }
}

/// `plan`, the rows below a grouping, with the scan of the table whose
/// first column is at `base` needing none of its columns `late` that
/// neither the grouping, which reads the columns `read`, nor a filter on
/// the way to the scan reads, so the scan is priced by what is still read
/// of it.
fn narrowed(plan: &LogicalPlan, base: usize, late: &[usize], read: &[usize]) -> Option<LogicalPlan> {
    let (scan, first, passed) = way_to(plan, base)?;
    let LogicalPlan::Scan { table, names, needed, located } = scan else { return None };
    let mut read = read.to_vec();
    for step in &passed {
        if let Passed::Filter(cond, from) = step {
            let mut of_input = Vec::new();
            cond_columns(cond, &mut of_input);
            read.extend(of_input.iter().map(|c| c.saturating_add(*from)));
        }
    }
    let still = |c: &usize| !late.contains(c) || read.contains(&first.saturating_add(*c));
    let needed = needed.iter().copied().filter(still).collect();
    let mut made = LogicalPlan::Scan { table: table.clone(), names: names.clone(), needed, located: *located };
    for step in passed.into_iter().rev() {
        made = match step {
            Passed::Filter(cond, _) => LogicalPlan::Filter { cond: cond.clone(), input: Box::new(made) },
            Passed::Product(right, true) => LogicalPlan::Product { left: Box::new(made), right: Box::new(right.clone()) },
            Passed::Product(left, false) => LogicalPlan::Product { left: Box::new(left.clone()), right: Box::new(made) },
        };
    }
    Some(made)
}

/// The key of the table `scan` reads that a join may fetch its rows by:
/// the columns of a PRIMARY KEY or UNIQUE constraint, not deferrable and
/// of no period, each NOT NULL and of no collation, with their types.
fn key_of(scan: &LogicalPlan, ctx: &RuleContext<'_>) -> Option<Vec<(usize, DataType)>> {
    let LogicalPlan::Scan { table, .. } = scan else { return None };
    let desc = ctx.db.catalog.table(table)?;
    let plain = |c: &usize| desc.columns.get(*c).filter(|d| d.not_null && d.collation.is_none()).map(|d| (*c, d.data_type.clone()));
    desc.constraints.iter().filter(|c| !c.attributes.deferrable).find_map(|c| match &c.kind {
        crate::catalog::ConstraintKind::Unique { columns, period: None, .. } => columns.iter().map(plain).collect::<Option<Vec<_>>>(),
        crate::catalog::ConstraintKind::Unique { period: Some(_), .. }
        | crate::catalog::ConstraintKind::NotNull(_)
        | crate::catalog::ConstraintKind::ForeignKey(_)
        | crate::catalog::ConstraintKind::Check { .. }
        | crate::catalog::ConstraintKind::Connection(_) => None,
    })
}

/// The tables below the grouping of `shape` whose columns it carries and
/// may fetch for `kept` rows, and each such column: one of a table that
/// has a key the grouping columns determine strictly, the column not of
/// the key, where the fetch reads fewer rows than the grouping groups.
fn candidates<'a>(shape: &Shape<'a>, kept: f64, ctx: &RuleContext<'_>) -> (Vec<Leaf<'a>>, Vec<Late>) {
    let known = Known::strict(ctx.db, shape.rows);
    let own: Vec<usize> =
        (0..shape.groups.len()).filter(|&i| by_own(&known, (shape.groups, &[]), i)).filter_map(|i| shape.groups.get(i).copied()).collect();
    let grouped = estimated_rows(ctx.db, ctx.seen, shape.rows);
    let (mut leaves, mut late): (Vec<Leaf<'a>>, Vec<Late>) = (Vec::new(), Vec::new());
    for (agg, c) in shape.aggs.iter().enumerate().filter_map(|(j, a)| Some((j, carried_column(a)?))) {
        let Some((scan, base, _)) = way_to(shape.rows, c) else { continue };
        let Some(key) = key_of(scan, ctx) else { continue };
        let column = c.saturating_sub(base);
        let rows = estimated_rows(ctx.db, ctx.seen, scan);
        let determined = key.iter().all(|(k, _)| known.determines(&own, base.saturating_add(*k)));
        let read = (kept * rows.max(1.0_f64).log2()).min(rows);
        if !determined || key.iter().any(|(k, _)| *k == column) || read >= grouped {
            continue;
        }
        let leaf = leaves.iter().position(|l| l.base == base).unwrap_or_else(|| {
            leaves.push(Leaf { scan, base, key });
            leaves.len().saturating_sub(1)
        });
        late.push(Late { agg, leaf, column });
    }
    (leaves, late)
}

/// The grouping of `shape` without the columns `late` fetches, carrying
/// each key column of the `leaves` it fetches of that it neither groups by
/// nor carries yet, and where its columns then are.
fn regrouped(shape: &Shape<'_>, leaves: &[Leaf<'_>], late: &[Late]) -> (LogicalPlan, Places) {
    let names = shape.rows.names();
    let mut at: Vec<Result<usize, usize>> = (0..shape.groups.len()).map(Ok).collect();
    let mut aggs = Vec::new();
    for (j, a) in shape.aggs.iter().enumerate() {
        if late.iter().any(|l| l.agg == j) {
            at.push(Err(j));
        } else {
            at.push(Ok(shape.groups.len().saturating_add(aggs.len())));
            aggs.push(a.clone());
        }
    }
    let mut keys = Vec::new();
    for (n, leaf) in leaves.iter().enumerate() {
        let used = late.iter().any(|l| l.leaf == n);
        let mut places = Vec::new();
        for (c, t) in leaf.key.iter().filter(|_| used).map(|(k, t)| (leaf.base.saturating_add(*k), t)) {
            let grouped = shape.groups.iter().position(|&g| g == c);
            let carried = || aggs.iter().position(|a| carried_column(a) == Some(c)).map(|p| shape.groups.len().saturating_add(p));
            places.push(grouped.or_else(carried).unwrap_or_else(|| {
                aggs.push(group_value(c, t, names.get(c).cloned().unwrap_or_default()));
                shape.groups.len().saturating_add(aggs.len()).saturating_sub(1)
            }));
        }
        keys.push(places);
    }
    let mut read = shape.groups.to_vec();
    aggs.iter().flat_map(AggCall::exprs).for_each(|e| expr_columns(e, &mut read));
    let mut rows = shape.rows.clone();
    for (n, leaf) in leaves.iter().enumerate() {
        let fetched: Vec<usize> = late.iter().filter(|l| l.leaf == n).map(|l| l.column).collect();
        rows = narrowed(&rows, leaf.base, &fetched, &read).unwrap_or(rows);
    }
    let (groups, input) = (shape.groups.to_vec(), Box::new(rows));
    (LogicalPlan::Aggregate { groups, collations: Vec::new(), aggs, seeds: Vec::new(), input }, Places { at, keys })
}

/// The select list `exprs` over `plan`, whose columns are at `places`,
/// without the columns it only hands on of those fetched late, and with
/// the leaves' key columns handed on after its own.
///
/// # Errors
///
/// The set function whose column an expression reads, if one reads a
/// column fetched late.
fn relisted(exprs: &[Expr], plan: LogicalPlan, places: &Places) -> Result<(LogicalPlan, Places), usize> {
    let late = |c: &usize| places.at.get(*c).and_then(|p| p.err());
    let (mut list, mut at) = (Vec::new(), Vec::new());
    for e in exprs {
        if let Expr::Column(c) = e
            && let Some(agg) = late(c)
        {
            at.push(Err(agg));
            continue;
        }
        let mut read = Vec::new();
        expr_columns(e, &mut read);
        if let Some(agg) = read.iter().find_map(late) {
            return Err(agg);
        }
        at.push(Ok(list.len()));
        list.push(map_expr_columns(e.clone(), &|c| places.at.get(c).copied().and_then(Result::ok).unwrap_or(c)));
    }
    let mut keys = Vec::new();
    for leaf in &places.keys {
        let from = list.len();
        list.extend(leaf.iter().map(|&p| Expr::Column(p)));
        keys.push((from..list.len()).collect());
    }
    Ok((LogicalPlan::Project { exprs: list, input: Box::new(plan) }, Places { at, keys }))
}

/// The HAVING condition `cond` over `plan`, whose columns are at
/// `places`.
///
/// # Errors
///
/// The set function whose column the condition reads, if it reads a
/// column fetched late.
fn refiltered(cond: &Cond, plan: LogicalPlan, places: &Places) -> Result<LogicalPlan, usize> {
    let mut read = Vec::new();
    cond_columns(cond, &mut read);
    if let Some(agg) = read.iter().find_map(|c| places.at.get(*c).and_then(|p| p.err())) {
        return Err(agg);
    }
    let cond = map_cond_columns(cond.clone(), &|c| places.at.get(c).copied().and_then(Result::ok).unwrap_or(c));
    Ok(LogicalPlan::Filter { cond, input: Box::new(plan) })
}

/// The plan of `shape` with the columns `late` fetched of the `leaves`
/// after its FETCH FIRST, as the module says.
///
/// # Errors
///
/// The set function whose column a select list's expression, a HAVING
/// condition or a sort key reads, which cannot then be fetched late.
fn rebuilt(shape: &Shape<'_>, leaves: &[Leaf<'_>], late: &[Late]) -> Result<LogicalPlan, usize> {
    let (mut plan, mut places) = regrouped(shape, leaves, late);
    for step in shape.steps.iter().rev() {
        match step {
            Step::List(exprs) => (plan, places) = relisted(exprs, plan, &places)?,
            Step::Having(cond) => plan = refiltered(cond, plan, &places)?,
        }
    }
    if let Some(keys) = shape.keys {
        let moved = |k: &SortKey| places.at.get(k.column).copied().unwrap_or(Ok(k.column)).map(|column| SortKey { column, ..*k });
        let keys = keys.iter().map(moved).collect::<Result<Vec<_>, usize>>()?;
        plan = LogicalPlan::Sort { keys, input: Box::new(plan) };
    }
    plan = LogicalPlan::Limit { limit: shape.limit.clone(), input: Box::new(plan) };
    let mut firsts = Vec::new();
    for ((leaf, keys), n) in leaves.iter().zip(&places.keys).zip(0_usize..) {
        firsts.push(plan.width());
        if late.iter().any(|l| l.leaf == n) {
            plan = fetched_of(plan, leaf, keys, late.iter().filter(|l| l.leaf == n).map(|l| l.column));
        }
    }
    let fetched = |agg: usize| late.iter().find(|l| l.agg == agg).and_then(|l| Some(firsts.get(l.leaf)?.saturating_add(l.column)));
    let column = |p: &Result<usize, usize>| p.map_or_else(|agg| fetched(agg).unwrap_or(agg), |c| c);
    let exprs = places.at.iter().map(|p| Expr::Column(column(p))).collect();
    let listed = LogicalPlan::Project { exprs, input: Box::new(plan) };
    Ok(match shape.keys {
        Some(keys) => LogicalPlan::Sort { keys: keys.to_vec(), input: Box::new(listed) },
        None => listed,
    })
}

/// `plan` joined to the table of `leaf` on its key, whose columns are at
/// `keys` of `plan`, the table read for its key and the columns `read`.
fn fetched_of(plan: LogicalPlan, leaf: &Leaf<'_>, keys: &[usize], read: impl Iterator<Item = usize>) -> LogicalPlan {
    let width = plan.width();
    let equal =
        |(&at, (k, _)): (&usize, &(usize, DataType))| Cond::Compare(Expr::Column(at), CompOp::Eq, Expr::Column(width.saturating_add(*k)));
    let Some(cond) = and_all(keys.iter().zip(&leaf.key).map(equal).collect()) else { return plan };
    let mut scan = leaf.scan.clone();
    if let LogicalPlan::Scan { needed, .. } = &mut scan {
        *needed = leaf.key.iter().map(|(k, _)| *k).chain(read).collect();
        needed.sort_unstable();
        needed.dedup();
    }
    LogicalPlan::Filter { cond, input: Box::new(LogicalPlan::Product { left: Box::new(plan), right: Box::new(scan) }) }
}

// Unit tests: what each scan of the table fetched of still needs, which
// no result shows (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    /// The columns each scan of the table `name` in `plan` needs, in the
    /// order the scans are met, the uppermost operand's first.
    fn needs(plan: &LogicalPlan, name: &str, out: &mut Vec<Vec<usize>>) {
        if let LogicalPlan::Scan { table, needed, .. } = plan
            && table.ident == name
        {
            out.push(needed.clone());
        }
        plan.inputs().for_each(|input| needs(input, name, out));
    }

    /// Verifies: [DESIGN] RULES.md D1202: of a column fetched after FETCH
    /// FIRST, of tables of 20 and 200 rows, the table's scan below the
    /// grouping needs its key alone, and the scan above the key and the
    /// column; a column a filter below still reads is needed by both.
    #[test]
    fn the_scan_below_needs_the_columns_fetched_no_more() {
        let db = Database::new();
        db.schema(
            "CREATE SCHEMA AUTHORIZATION O
               CREATE TABLE P (K INTEGER NOT NULL PRIMARY KEY, N VARCHAR(10), A INTEGER)
               CREATE TABLE C (K INTEGER NOT NULL PRIMARY KEY, P INTEGER)",
        )
        .unwrap();
        let s = db.session("O");
        let rows = |n: i32, of: &dyn Fn(i32) -> String| (0_i32..n).map(of).collect::<Vec<_>>().join(", ");
        s.run(&format!("INSERT INTO P VALUES {}", rows(20_i32, &|k| format!("({k}, 'n{k}', {k})")))).unwrap();
        s.run(&format!("INSERT INTO C VALUES {}", rows(200_i32, &|k| format!("({k}, {})", k % 20_i32)))).unwrap();
        s.commit().unwrap();
        db.maintain().unwrap();
        let grouped = "FROM P, C WHERE C.P = P.K GROUP BY P.K, P.N, P.A ORDER BY 4 DESC FETCH FIRST 1 ROW ONLY";
        for (filter, below, above) in [("", vec![0], vec![0, 1, 2]), (" AND P.N <> 'x'", vec![0, 1], vec![0, 1, 2])] {
            let sql = format!("SELECT P.K, P.N, P.A, COUNT(*) {}", grouped.replace(" GROUP", &format!("{filter} GROUP")));
            let (st, bound) = s.bound_plan(&sql).unwrap();
            let (plan, trace) = st.optimizer.optimize(bound, &RuleContext { db: &st, seen: &crate::physical::Seen::default() });
            assert!(trace.contains(&FETCH_DEPENDENTS_LATE), "{sql}: {trace:?}");
            let mut found = Vec::new();
            needs(&plan, "P", &mut found);
            assert_eq!(found, vec![below, above], "{sql}");
        }
    }
}
