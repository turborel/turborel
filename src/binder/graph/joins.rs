//! \[EXTENSION\] [GS §16 R16.2] Rigid patterns as joins (RULES.md D113): a
//! GRAPH_TABLE whose path patterns have no quantifier is the join of its
//! element variables' tables on their edges' ends, which the ordinary
//! planner orders and reads (`physical/order.rs`), each edge table probed
//! through its adjacency index or another as Expand, both of its ends
//! bound as ExpandInto, or hashed.
//!
//! Each element variable is one operand of the join, a scan of the
//! element table it binds. A variable that may bind several tables, or an
//! edge that may lie either way, gives one join of each choice, and the
//! GRAPH_TABLE is the UNION ALL of them (`branches`). The match row a
//! search makes (`exec/graph_table/values.rs`) is made of the operands'
//! rows by each variable's projection, so the bound COLUMNS, prefilters
//! and postfilter are read over it unchanged (`substituted`).
//!
//! What a join cannot give as a search does stays with the search: a
//! quantifier, a union or a parenthesized pattern; a selector, a COST or a
//! restrictor or match mode over a variable written twice; ONE ROW PER
//! VERTEX or STEP; a path function or a group's set function; OPTIONS of
//! the search's limits; PATH_ALGO; a view or a table read FOR SYSTEM_TIME;
//! a subquery; and a GRAPH_TABLE reading a table before it.

mod rigid;

use std::cell::Cell;

use super::super::Context;
use crate::catalog::ColumnDesc;
use crate::graph::program::{MatchMode, Restrictor};
use crate::graph::view::Way;
use crate::logical::{
    Cond, EDGE_COLUMNS, ElementRead, ElementSource, Expr, GraphMatch, GraphOptions, LogicalPlan, RowsShape, SetOp, VERTEX_COLUMNS, and_all,
    map_cond_exprs, map_expr_columns, or_all,
};
use crate::value::CompOp;
use rigid::{Hop, Rigid, rigids, twice};

/// Of an edge variable's hidden columns, its source's identity's place.
const SOURCE_AT: usize = 2;

/// Of an edge variable's hidden columns, its destination's identity's.
const DESTINATION_AT: usize = 3;

/// The most joins, of choices of tables and ways, a GRAPH_TABLE is the
/// union of; past it, it is searched.
const MOST_BRANCHES: usize = 64;

/// The most whole scans of edge tables of no adjacency index, one of
/// each such edge of each join, a GRAPH_TABLE's joins may read; past it,
/// it is searched, which reads each table once.
const MOST_EDGE_SCANS: usize = 4;

/// One join: the element table each used variable binds, in the order of
/// `Rigid::used`, and of each hop whether it is walked from its source.
#[derive(Clone)]
struct Branch {
    tables: Vec<usize>,
    forward: Vec<bool>,
}

/// \[GS §16 R16.2\] GRAPH_TABLE `g`, its operands `operands`, as the union
/// of joins of its element tables giving its COLUMNS, if it is rigid and
/// a join gives what its search would; none if it is searched. A cyclic
/// one whose edge tables each have an adjacency index is matched by the
/// worst-case optimal join instead, unless NO_WCOJ (RULES.md D114).
pub(super) fn planned(cx: Context<'_>, g: &mut GraphMatch, operands: &[Expr]) -> Option<LogicalPlan> {
    // [GS §16 R16.8] A hint of how to search keeps the pattern searched.
    let h = &g.hints;
    let hinted = h.path_algo.is_some() || h.start.is_some() || !h.reverse.is_empty() || !h.use_adjacency.is_empty();
    let searched = hinted || !g.slots.is_empty() || g.options != GraphOptions::default();
    if searched || !matches!(g.rows, RowsShape::Match) || !operands.iter().all(constant) {
        return None;
    }
    if worst_case(g)? {
        g.wcoj = true;
        return None;
    }
    let g = &*g;
    let mut plans = Vec::new();
    let mut scans = 0_usize;
    for rigid in rigids(g)? {
        // The conditions of a restrictor are of different instances.
        if !rigid.paths.iter().all(|(restrictor, vars)| *restrictor == Restrictor::Walk || !twice(vars)) {
            return None;
        }
        let joins = branches(g, &rigid)?;
        let unindexed = rigid.hops.iter().filter(|hop| !indexed(g, rigid.vars.get(hop.edge))).count();
        scans = scans.checked_add(joins.len().checked_mul(unindexed)?)?;
        (scans <= MOST_EDGE_SCANS).then_some(())?;
        for b in &joins {
            plans.push(joined(cx, g, (&rigid, b), operands)?);
            (plans.len() <= MOST_BRANCHES).then_some(())?;
        }
    }
    let mut plans = plans.into_iter();
    let first = plans.next()?;
    Some(plans.fold(first, |left, right| LogicalPlan::SetOp {
        op: SetOp::Union,
        all: true,
        compared: None,
        left: Box::new(left),
        right: Box::new(right),
    }))
}

/// Of rigid `g`, whether it is matched by the worst-case optimal join:
/// its edge patterns close a cycle of its node patterns, each edge table
/// they may bind has an adjacency index, and no NO_WCOJ hint is written.
fn worst_case(g: &GraphMatch) -> Option<bool> {
    let rigids = rigids(g)?;
    let [r] = rigids.as_slice() else { return Some(false) };
    let mut parent: Vec<usize> = (0..r.vars.len()).collect();
    let root = |of: &[usize], mut v: usize| {
        while let Some(&p) = of.get(v).filter(|&&p| p != v) {
            v = p;
        }
        v
    };
    let mut cyclic = false;
    for hop in &r.hops {
        let (a, b) = (root(&parent, hop.before), root(&parent, hop.after));
        cyclic |= a == b;
        if let Some(slot) = parent.get_mut(a) {
            *slot = b;
        }
    }
    Some(cyclic && !r.unrolled && !g.hints.no_wcoj && r.hops.iter().all(|h| indexed(g, r.vars.get(h.edge))))
}

/// Of `g`, whether each edge table edge variable `var` may bind has an
/// adjacency index.
fn indexed(g: &GraphMatch, var: Option<&usize>) -> bool {
    let tables = var.and_then(|&v| g.vars.get(v)).map(|v| v.tables.as_slice()).unwrap_or_default();
    tables.iter().all(|(t, _)| g.tables.get(*t).is_some_and(|s| s.adjacency.is_some()))
}

/// Whether an operand is the same for every row: a constant or a dynamic
/// parameter, not a column of a table before the GRAPH_TABLE.
fn constant(e: &Expr) -> bool {
    matches!(e, Expr::Const(_) | Expr::Dynamic(_))
}

/// Each join of `r`: a choice of a table for each variable that every
/// element pattern of it allows by its labels, and of a way for each edge
/// pattern its orientation and its ends allow; none if there are more
/// than `MOST_BRANCHES`. Each edge pattern is walked, in order, once its
/// variables have tables, so the choices it rules out go first.
fn branches(g: &GraphMatch, r: &Rigid<'_>) -> Option<Vec<Branch>> {
    let mut chosen = vec![Branch { tables: Vec::new(), forward: Vec::new() }];
    let mut hops = r.hops.iter().peekable();
    for (i, &var) in r.vars.iter().enumerate() {
        let shape = g.vars.get(var)?;
        let fits = |t: usize| {
            let labels = g.tables.get(t).map(|s| s.labels.as_slice()).unwrap_or_default();
            r.tests.iter().filter(|(of, _)| *of == i).all(|(_, x)| x.labels.as_ref().is_none_or(|l| l.holds(labels)))
        };
        let tables: Vec<usize> = shape.tables.iter().map(|(t, _)| *t).filter(|&t| fits(t)).collect();
        let stored = tables.iter().all(|&t| g.tables.get(t).is_some_and(|s| matches!(s.read, ElementRead::Stored)));
        if !stored {
            return None;
        }
        chosen = chosen.into_iter().flat_map(|b| tables.iter().map(move |&t| b.with_table(t))).collect();
        while let Some(hop) = hops.next_if(|h| h.before.max(h.edge).max(h.after) <= i) {
            chosen = chosen.into_iter().flat_map(|b| [true, false].into_iter().filter_map(move |f| b.walked(g, (hop, f)))).collect();
        }
        (chosen.len() <= MOST_BRANCHES).then_some(())?;
    }
    hops.next().is_none().then_some(chosen)
}

impl Branch {
    /// It, `table` chosen for the next variable.
    fn with_table(&self, table: usize) -> Branch {
        let mut b = self.clone();
        b.tables.push(table);
        b
    }

    /// The table it chose for instance `i`.
    fn table(&self, i: usize) -> Option<usize> {
        self.tables.get(i).copied()
    }

    /// It, `hop` walked from the edge's source if `forward`, if the edge
    /// table's ends are the node patterns' tables and the orientation
    /// allows that way.
    fn walked(&self, g: &GraphMatch, (hop, forward): (&Hop, bool)) -> Option<Branch> {
        let edge = g.tables.get(self.table(hop.edge)?)?;
        let ends = edge.ends.as_ref()?;
        let way = match (ends.directed, forward) {
            (true, true) => Way::Right,
            (true, false) => Way::Left,
            (false, true | false) => Way::Undirected,
        };
        let allowed = hop.orientation.allows(way);
        let (source, destination) = if forward { (hop.before, hop.after) } else { (hop.after, hop.before) };
        let fits = ends.source.table == self.table(source)? && ends.destination.table == self.table(destination)?;
        (allowed && fits).then(|| {
            let mut b = self.clone();
            b.forward.push(forward);
            b
        })
    }
}

/// The join of branch `b` of `r`: the scans of its tables, filtered by
/// its edges' ends, its prefilters, its restrictors and match mode, and
/// the postfilter, and projected to COLUMNS.
fn joined(cx: Context<'_>, g: &GraphMatch, (r, b): (&Rigid<'_>, &Branch), operands: &[Expr]) -> Option<LogicalPlan> {
    let (mut starts, mut conds) = (Vec::new(), Vec::new());
    let mut plan: Option<LogicalPlan> = None;
    let mut width = 0_usize;
    for &t in &b.tables {
        let source = g.tables.get(t)?;
        let desc = cx.db.catalog.table(&source.table)?;
        let names: Vec<String> = desc.columns.iter().map(|c| c.name.clone()).collect();
        let needed = (0..names.len()).collect();
        let scan = LogicalPlan::Scan { table: source.table.clone(), names, needed, located: false };
        conds.extend(null_ends(source, &desc.columns, width)?);
        starts.push(width);
        width = width.checked_add(source.width)?;
        plan = Some(match plan {
            Some(left) => LogicalPlan::Product { left: Box::new(left), right: Box::new(scan) },
            None => scan,
        });
    }
    let row = Row { g, r, b, starts: &starts, operands, pin: None };
    conds.extend(ends(&row)?);
    conds.extend(distinct(&row)?);
    for p in &g.prefilters {
        // [GS §16 R16.4] An unrolled element pattern's, of each iteration.
        let each = p.own.map(|own| r.instances_of(own)).unwrap_or_default();
        let unrolled = |var: usize| r.instances_of(var).len() > 1;
        if p.reads.iter().any(|&(var, _)| Some(var) != p.own && unrolled(var)) {
            return None;
        }
        match p.own.filter(|_| each.len() > 1) {
            Some(own) => {
                for &i in &each {
                    conds.push(Row { pin: Some((own, i)), ..row }.cond(&p.cond)?);
                }
            }
            None => conds.push(row.cond(&p.cond)?),
        }
    }
    if let Some(c) = &g.postfilter {
        conds.push(row.cond(c)?);
    }
    let mut plan = plan?;
    if let Some(cond) = and_all(conds) {
        plan = LogicalPlan::Filter { cond, input: Box::new(plan) };
    }
    let exprs = g.columns.iter().map(|e| row.expr(e)).collect::<Option<Vec<_>>>()?;
    Some(LogicalPlan::Project { exprs, input: Box::new(plan) })
}

/// Of element table `source` of `columns`, its row at `start` of the
/// join, IS NOT NULL of each edge end's column that may be null: implied by
/// the ends' equalities, it lets an adjacency index, which holds no entry
/// of a null end, be probed (`physical/indexed/expand.rs`).
fn null_ends(source: &ElementSource, columns: &[ColumnDesc], start: usize) -> Option<Vec<Cond>> {
    let Some(shape) = &source.ends else { return Some(Vec::new()) };
    let ends = shape.source.columns.iter().chain(&shape.destination.columns);
    let nullable = ends.filter(|&&c| columns.get(c).is_some_and(|d| !d.not_null));
    nullable.map(|&c| Some(Cond::IsNull(Expr::Column(start.checked_add(c)?), true))).collect()
}

/// The match row of a branch, of the join's operands: where each
/// variable's table's row begins, and the GRAPH_TABLE's operands.
struct Row<'a> {
    g: &'a GraphMatch,
    r: &'a Rigid<'a>,
    b: &'a Branch,
    starts: &'a [usize],
    operands: &'a [Expr],
    /// A variable read of one of its instances, not its first.
    pin: Option<(usize, usize)>,
}

impl Row<'_> {
    /// Where the row of instance `i`'s table begins in the join.
    fn start(&self, i: usize) -> Option<usize> {
        self.starts.get(i).copied()
    }

    /// Column `k` of the match row over the join: a variable's, of its
    /// instance; else an operand.
    fn column(&self, k: usize) -> Option<Expr> {
        let Some((var, shape)) = self.g.vars.iter().enumerate().find(|(_, v)| k >= v.offset && k < v.offset.saturating_add(v.width)) else {
            return self.operands.get(k.checked_sub(self.g.width)?).cloned();
        };
        let pinned = self.pin.and_then(|(v, i)| (v == var).then_some(i));
        let i = pinned.or_else(|| self.r.of_var(var))?;
        self.instance_column(i, var, k.saturating_sub(shape.offset))
    }

    /// Column `at` of variable `var`'s, of instance `i`: by the projection
    /// of the table it binds, an edge's ends' identities those of its
    /// hop's node patterns, as the search fills them
    /// (`exec/graph_table/shared.rs`).
    fn instance_column(&self, i: usize, var: usize, at: usize) -> Option<Expr> {
        let shape = self.g.vars.get(var)?;
        let hidden = shape.width.saturating_sub(EDGE_COLUMNS);
        let source = at == hidden.saturating_add(SOURCE_AT);
        if shape.edge && (source || at == hidden.saturating_add(DESTINATION_AT)) {
            let (hop, forward) = self.r.hops.iter().zip(&self.b.forward).find(|(h, _)| h.edge == i)?;
            return self.identity(if source == *forward { hop.before } else { hop.after });
        }
        let (table, start) = (self.b.table(i)?, self.start(i)?);
        let e = shape.projection(table)?.get(at)?.clone();
        Some(map_expr_columns(e, &|c| c.saturating_add(start)))
    }

    /// The identity of instance `i`, of its variable's hidden columns.
    fn identity(&self, i: usize) -> Option<Expr> {
        let var = *self.r.vars.get(i)?;
        let shape = self.g.vars.get(var)?;
        let hidden = if shape.edge { EDGE_COLUMNS } else { VERTEX_COLUMNS };
        self.instance_column(i, var, shape.width.checked_sub(hidden)?)
    }

    /// `e`, over the match row, over the join; none if it reads a subquery,
    /// whose outer references the join does not give.
    fn expr(&self, e: &Expr) -> Option<Expr> {
        let failed = Cell::new(false);
        let out = substituted(e.clone(), &|k| {
            let made = self.column(k);
            failed.set(failed.get() || made.is_none());
            made.unwrap_or(Expr::Column(k))
        });
        (!failed.get() && !reads_subquery(&out)).then_some(out)
    }

    /// `c`, over the match row, over the join.
    fn cond(&self, c: &Cond) -> Option<Cond> {
        if !c.subqueries().is_empty() {
            return None;
        }
        let failed = Cell::new(false);
        let out = map_cond_exprs(c.clone(), &|e| {
            let made = self.expr(&e);
            failed.set(failed.get() || made.is_none());
            made.unwrap_or(e)
        });
        (!failed.get()).then_some(out)
    }
}

/// `e`, each column read by `f`.
fn substituted(e: Expr, f: &dyn Fn(usize) -> Expr) -> Expr {
    match e {
        Expr::Column(k) => f(k),
        other @ (Expr::Const(_)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::NextValue(_)
        | Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Call(..)
        | Expr::Case(..)
        | Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::SortKey(..)
        | Expr::Subquery(..)
        | Expr::Truth(_)
        | Expr::Invoked(_)) => other.map_children(&mut |child| substituted(child, f)),
    }
}

/// Whether `e` reads a subquery.
fn reads_subquery(e: &Expr) -> bool {
    let mut found = matches!(e, Expr::Subquery(..));
    e.for_each_child(&mut |child| found = found || reads_subquery(child));
    found
}

/// \[GS §4.1 R4.2\] The conditions joining each edge to its ends: its
/// source's columns equal to those its source vertex's key references,
/// and its destination's; an undirected edge walked from its destination
/// is no self-loop, which its source gives once.
fn ends(row: &Row<'_>) -> Option<Vec<Cond>> {
    let mut conds = Vec::new();
    // The edges' columns equal to each vertex column, which equal each other.
    let mut met: Vec<((usize, usize), Vec<usize>)> = Vec::new();
    for (hop, &forward) in row.r.hops.iter().zip(&row.b.forward) {
        let edge = row.g.tables.get(row.b.table(hop.edge)?)?;
        let shape = edge.ends.as_ref()?;
        let e = row.start(hop.edge)?;
        let (source, destination) = if forward { (hop.before, hop.after) } else { (hop.after, hop.before) };
        for (end, var) in [(&shape.source, source), (&shape.destination, destination)] {
            let v = row.start(var)?;
            for (&c, &k) in end.columns.iter().zip(&end.references) {
                let (at, of) = (e.checked_add(c)?, v.checked_add(k)?);
                conds.push(Cond::Compare(Expr::Column(at), CompOp::Eq, Expr::Column(of)));
                match met.iter_mut().find(|(vertex, _)| *vertex == (var, k)) {
                    Some((_, columns)) => columns.push(at),
                    None => met.push(((var, k), vec![at])),
                }
            }
        }
        // A self-loop walked both ways is one edge: the way back skips it.
        let both_ways = !shape.directed || hop.orientation.allows(Way::Right);
        if both_ways && !forward && shape.source.table == shape.destination.table {
            let pairs = shape.source.columns.iter().zip(&shape.destination.columns);
            let differ =
                pairs.map(|(&s, &d)| Some(Cond::Compare(Expr::Column(e.checked_add(s)?), CompOp::Ne, Expr::Column(e.checked_add(d)?))));
            conds.push(or_all(differ.collect::<Option<Vec<_>>>()?)?);
        }
    }
    // [GS §16 R16.2] Implied, so an edge may be joined to the next by Expand.
    for (_, columns) in &met {
        let first = *columns.first()?;
        conds.extend(
            columns.iter().skip(1).filter(|&&c| c != first).map(|&c| Cond::Compare(Expr::Column(first), CompOp::Eq, Expr::Column(c))),
        );
    }
    Some(conds)
}

/// \[GS §7.1, §7.3\] The conditions that the variables a restrictor or the
/// match mode keeps apart bind different elements: of each pair of them
/// binding one table, their identities differ.
fn distinct(row: &Row<'_>) -> Option<Vec<Cond>> {
    let g = row.g;
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let all: Vec<usize> = (0..row.r.vars.len()).collect();
    let kind = |instances: &[usize], edge: bool| -> Vec<usize> {
        let of = |i: usize| row.r.vars.get(i).and_then(|&v| g.vars.get(v));
        instances.iter().copied().filter(|&i| of(i).is_some_and(|s| s.edge == edge)).collect()
    };
    match g.mode {
        MatchMode::RepeatableElements => {}
        MatchMode::DifferentEdges => pairs.extend(all_pairs(&kind(&all, true))),
        MatchMode::DifferentVertices => pairs.extend(all_pairs(&kind(&all, false))),
    }
    for (restrictor, vars) in &row.r.paths {
        match restrictor {
            Restrictor::Walk => {}
            Restrictor::Trail => pairs.extend(all_pairs(&kind(vars, true))),
            Restrictor::Acyclic => pairs.extend(all_pairs(&kind(vars, false))),
            Restrictor::Simple => {
                // The last vertex may be the first.
                let vertices = kind(vars, false);
                let ends = (vertices.first().copied(), vertices.last().copied());
                pairs.extend(all_pairs(&vertices).into_iter().filter(|&(a, b)| (Some(a), Some(b)) != ends));
            }
        }
    }
    let mut conds = Vec::new();
    for (a, b) in pairs.into_iter().filter(|&(a, b)| row.b.table(a) == row.b.table(b)) {
        // Of one table, two elements differ if a column of its key does:
        // comparisons of columns, which the join order can place early.
        let key = &g.tables.get(row.b.table(a)?)?.key;
        let (at_a, at_b) = (row.start(a)?, row.start(b)?);
        let differ =
            key.iter().map(|&k| Some(Cond::Compare(Expr::Column(at_a.checked_add(k)?), CompOp::Ne, Expr::Column(at_b.checked_add(k)?))));
        conds.push(or_all(differ.collect::<Option<Vec<_>>>()?)?);
    }
    Some(conds)
}

/// Each pair of `vars`, the earlier first.
fn all_pairs(vars: &[usize]) -> Vec<(usize, usize)> {
    vars.iter().enumerate().flat_map(|(i, &a)| vars.iter().skip(i.saturating_add(1)).map(move |&b| (a, b))).collect()
}
