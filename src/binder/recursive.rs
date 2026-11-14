//! [ISO/IEC 9075-2:2023 §7.17] A potentially recursive `<with list>`,
//! bound: each element with a `<with column list>` [SR3j i] and with no
//! potentially recursive list in it [SR3h], its strata bound each after
//! those it depends on [GR2b] (`binder/strata.rs`).
//!
//! An element of a stratum that is not recursive is bound as one of a list
//! that is not [GR2c i]. The queries of a recursive stratum are bound with
//! each of its members' names identifying the member's work table [GR2c ii
//! 2], and each reference to a member outside them is a recursive query of
//! the stratum's plans (`LogicalPlan::Recursive`).
//!
//! A member's work table has its query's columns [GR2c ii 1]. An anchor's
//! column that is recursively derived, of which a column of a member is an
//! underlying column of the recursive operand's column, is of the
//! non-recursive operand's type; another, of the type both operands' give
//! it [SR19b ii 2]. Its columns are found again until they settle, each
//! taken to be possibly nullable, which is never wrong. What the recursive
//! operand's columns are drawn from is found as a view's are
//! (`binder/underlying.rs`), each member's work table drawn from itself.
//!
//! A stratum of one member whose recursive operand distributes over its
//! UNION, as an iteration over only the rows the last added would read
//! them, is found so (`Fixpoint::Delta`); another, as the General Rules
//! say (`Fixpoint::Whole`).

use super::case::set_type;
use super::set_operations::{aligned, assembled, bind_query_expression, conform, result_collation};
use super::strata::{self, Rules, Side};
use super::subquery::Outer;
use super::underlying::drawn_from;
use super::with::{Queries, Query, bind_element, element_columns, element_nesting, with_scope};
use super::{Bound, Context, OutColumn, rule};
use crate::ast::{Correspondence, QueryExpression, SetOperation, WithElement, WithQuery};
use crate::catalog::{ColumnDesc, QualifiedName, Underlying};
use crate::error::{At, Code, Result, SqlError};
use crate::logical::{Fixpoint, LogicalPlan, SetOp};

/// A member of a recursive stratum: its element, and, of an anchor, which
/// operand of its UNION is its non-recursive one.
struct Member<'w> {
    element: &'w WithElement,
    anchor: Option<Side>,
    /// The members, by their places in the stratum, its query references.
    on: Vec<usize>,
    /// [RULES.md D70] The places of its columns that are arrays of the
    /// greatest maximum cardinality, of a `<search or cycle clause>`
    /// (`binder/search_cycle.rs`).
    widened: &'w [usize],
}

/// The elements of the potentially recursive list `w`, bound into
/// `queries`, what each column is drawn from found too if `drawn`.
pub(super) fn bind_list(cx: Context<'_>, w: &WithQuery, (queries, drawn): (&Queries<'_>, bool)) -> Result<()> {
    for e in &w.elements {
        if e.columns.is_none() {
            let msg = format!("[ISO/IEC 9075-2:2023 §7.17 SR3j i] query {} of a WITH RECURSIVE list has no <with column list>", e.name);
            return Err(rule(msg).or_at(e.at));
        }
        if strata::has_recursive_list(&e.query) {
            let msg = format!("[ISO/IEC 9075-2:2023 §7.17 SR3h] query {} of a WITH RECURSIVE list contains a WITH RECURSIVE list", e.name);
            return Err(rule(msg).or_at(e.at));
        }
    }
    let arcs = strata::graph(w);
    let expanded = super::search_cycle::expanded(w, &arcs)?;
    let (w, widened) = expanded.as_ref().map_or((w, &[][..]), |x| (&x.list, x.widened.as_slice()));
    for stratum in strata::strata(&arcs) {
        let elements: Vec<&WithElement> = stratum.members.iter().filter_map(|&m| w.elements.get(m)).collect();
        if !stratum.recursive {
            elements.into_iter().try_for_each(|e| bind_element(cx, e, (queries, drawn)))?;
            continue;
        }
        let rules = Rules::new((cx.db, cx.schema), w, &stratum.members);
        let mut members = Vec::new();
        for (e, &n) in elements.into_iter().zip(&stratum.members) {
            rules.check(e)?;
            let on = arcs.get(n).into_iter().flatten().filter_map(|j| stratum.members.iter().position(|m| m == j)).collect();
            let wide = widened.get(n).map_or(&[][..], Vec::as_slice);
            members.push(Member { element: e, anchor: rules.anchor(e)?, on, widened: wide });
        }
        let anchors: Vec<usize> = stratum.members.iter().zip(&members).filter(|(_, m)| m.anchor.is_some()).map(|(&n, _)| n).collect();
        rules.anchored(&arcs, &stratum.members, &anchors)?;
        bind_stratum(cx, &members, (queries, drawn))?;
    }
    Ok(())
}

/// The recursive stratum of `members`, bound into `queries`.
fn bind_stratum(cx: Context<'_>, members: &[Member<'_>], (queries, drawn): (&Queries<'_>, bool)) -> Result<()> {
    let columns = typed(cx, members, queries)?;
    let descs = members.iter().zip(&columns).map(|(m, c)| element_columns(c, m.element)).collect::<Result<Vec<_>>>()?;
    let fixpoint = planned(cx, members, (&columns, &descs), queries)?;
    let underlying = if drawn { Some(drawn_members(cx, members, &descs, queries)?) } else { None };
    for (k, (m, desc)) in members.iter().zip(descs).enumerate() {
        let names = desc.iter().map(|c| c.name.clone()).collect();
        let plan = LogicalPlan::Recursive { member: k, fixpoint: fixpoint.clone(), names };
        let underlying = underlying.as_ref().and_then(|u: &Vec<Underlying>| u.get(k).cloned());
        queries.add(Query { name: m.element.name.clone(), columns: desc, plan, underlying });
    }
    Ok(())
}

/// The work tables of the stratum of `members`, each of the columns of
/// `descs`, in a scope within `queries`, each drawn from what `drawn` says.
fn work_tables<'q>(
    queries: &'q Queries<'q>,
    members: &[Member<'_>],
    descs: &[Vec<ColumnDesc>],
    drawn: &dyn Fn(usize) -> Option<Underlying>,
) -> Queries<'q> {
    let scope = Queries::within(queries);
    for (k, (m, columns)) in members.iter().zip(descs).enumerate() {
        let names = columns.iter().map(|c| c.name.clone()).collect();
        let plan = LogicalPlan::WorkTable { member: k, names };
        scope.add(Query { name: m.element.name.clone(), columns: columns.clone(), plan, underlying: drawn(k) });
    }
    scope
}

/// What a work table of the columns `columns` is drawn from to find which
/// columns are recursively derived: itself, as a table of the query name
/// `name` and of no schema, which no other is.
fn itself(name: &str, columns: &[ColumnDesc]) -> Underlying {
    let table = QualifiedName { auth: String::new(), ident: name.to_owned() };
    Underlying { leaves: vec![table.clone()], columns: columns.iter().map(|c| vec![(table.clone(), c.name.clone())]).collect() }
}

/// `then` of the `<query expression body>` of `q`, bound with `cx`, a
/// subquery of `site`'s query if given, the query names of the `<with
/// clause>`s before it in scope.
fn in_body<'a, T>(
    cx: Context<'a>,
    q: &QueryExpression,
    site: Option<&'a Outer<'a>>,
    then: &mut dyn FnMut(Context<'_>, &QueryExpression) -> Result<T>,
) -> Result<T> {
    match q {
        QueryExpression::With(w) => with_scope(cx, w, (site, false), |inner| in_body(inner, &w.body, site, then)),
        QueryExpression::Spec(_) | QueryExpression::Values(_) | QueryExpression::SetOp(_) => then(cx, q),
    }
}

/// [ISO/IEC 9075-2:2023 §7.17 SR19b ii 2] The columns of the work table
/// of each of `members`, found again until they settle: first an anchor's
/// of its non-recursive operand, and each other's of its query over those
/// found before it.
fn typed(cx: Context<'_>, members: &[Member<'_>], queries: &Queries<'_>) -> Result<Vec<Vec<OutColumn>>> {
    let mut guess: Vec<Option<Vec<OutColumn>>> = Vec::new();
    for m in members {
        guess.push(m.anchor.map(|side| first_columns(cx, (m, side), queries)).transpose()?);
    }
    while let Some(k) = (0..members.len()).find(|&k| guess.get(k).is_some_and(Option::is_none) && ready(members, k, &guess)) {
        let found = columns_over(cx, (members, k), &guess, queries)?;
        if let Some(g) = guess.get_mut(k) {
            *g = Some(found);
        }
    }
    let mut settled: Vec<Vec<OutColumn>> = guess.into_iter().map(Option::unwrap_or_default).collect();
    for _ in 0..members.len().saturating_add(2) {
        let over: Vec<Option<Vec<OutColumn>>> = settled.iter().cloned().map(Some).collect();
        let next = (0..members.len()).map(|k| columns_over(cx, (members, k), &over, queries)).collect::<Result<Vec<_>>>()?;
        if next.iter().zip(&settled).all(|(a, b)| same_types(a, b)) {
            return Ok(next);
        }
        settled = next;
    }
    let names: Vec<&str> = members.iter().map(|m| m.element.name.as_str()).collect();
    Err(SqlError::new(Code::Unsupported, format!("recursive queries {} whose types do not settle are not supported", names.join(", "))))
}

/// Whether every member that member `k` of `members` references has
/// columns `guess` gives.
fn ready(members: &[Member<'_>], k: usize, guess: &[Option<Vec<OutColumn>>]) -> bool {
    members.get(k).is_some_and(|m| m.on.iter().all(|&j| guess.get(j).is_some_and(Option::is_some)))
}

/// Whether `a` and `b` are columns of the same types and collations.
fn same_types(a: &[OutColumn], b: &[OutColumn]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.data_type == y.data_type && x.collation == y.collation)
}

/// The columns of the non-recursive operand of the anchor `m`, its
/// operand at `side`, possibly nullable.
fn first_columns(cx: Context<'_>, (m, side): (&Member<'_>, Side), queries: &Queries<'_>) -> Result<Vec<OutColumn>> {
    let inner = Context { queries: Some(queries), draws: false, ..cx };
    let outer = queries.site();
    in_body(inner, &m.element.query, outer, &mut |within, body| {
        let s = union_of(body)?;
        let first = bind_query_expression(within, operand(s, side), &element_nesting(outer))?;
        let columns = match &s.corresponding {
            Correspondence::By(names) => picked(&first.columns, &positions(&first.columns, names)),
            Correspondence::Position | Correspondence::Names => first.columns,
        };
        let columns = columns.into_iter().map(|c| OutColumn { not_null: false, origin: None, ..c }).collect();
        Ok(super::search_cycle::widened(columns, m.widened))
    })
    .or_at(m.element.at)
}

/// The columns of member `k` of `members` with each member's work table of
/// the columns `guess` gives it, those it has not left out of scope.
fn columns_over(
    cx: Context<'_>,
    (members, k): (&[Member<'_>], usize),
    guess: &[Option<Vec<OutColumn>>],
    queries: &Queries<'_>,
) -> Result<Vec<OutColumn>> {
    let Some(m) = members.get(k) else { return Err(SqlError::new(Code::Api, "[DESIGN] a recursive stratum has no such member")) };
    let mut descs = Vec::new();
    let mut known = Vec::new();
    for (j, (member, columns)) in members.iter().zip(guess).enumerate() {
        if let Some(columns) = columns {
            descs.push(element_columns(columns, member.element)?);
            known.push(j);
        }
    }
    let known_members: Vec<&Member<'_>> = known.iter().filter_map(|&j| members.get(j)).collect();
    let scope = known_work_tables(queries, (&known_members, &known), &descs);
    let inner = Context { queries: Some(&scope), draws: false, ..cx };
    let outer = queries.site();
    let names: Vec<&str> = members.iter().map(|member| member.element.name.as_str()).collect();
    in_body(inner, &m.element.query, outer, &mut |within, body| {
        if let Some(side) = m.anchor {
            let columns = anchor_columns(within, (union_of(body)?, side), (outer, &names))?;
            return Ok(super::search_cycle::widened(columns, m.widened));
        }
        let bound = bind_query_expression(within, body, &element_nesting(outer))?;
        Ok(bound.columns.into_iter().map(|c| OutColumn { not_null: false, origin: None, ..c }).collect())
    })
    .or_at(m.element.at)
}

/// The work tables of `known`, the members at their places `places`, each
/// of the columns of `descs`, in a scope within `queries`, each drawn from
/// itself (`itself`).
fn known_work_tables<'q>(queries: &'q Queries<'q>, (known, places): (&[&Member<'_>], &[usize]), descs: &[Vec<ColumnDesc>]) -> Queries<'q> {
    let scope = Queries::within(queries);
    for ((m, &k), columns) in known.iter().zip(places).zip(descs) {
        let names = columns.iter().map(|c| c.name.clone()).collect();
        let plan = LogicalPlan::WorkTable { member: k, names };
        let underlying = Some(itself(&m.element.name, columns));
        scope.add(Query { name: m.element.name.clone(), columns: columns.clone(), plan, underlying });
    }
    scope
}

/// [ISO/IEC 9075-2:2023 §7.17 SR19b ii 2] The columns of the anchor
/// expression whose UNION is `s`, its non-recursive operand at `side`,
/// bound with `cx`, a subquery of `site`'s query if given, the members of
/// its stratum named `names`: a recursively derived column of the
/// non-recursive operand's type, another of both's.
fn anchor_columns(
    cx: Context<'_>,
    (s, side): (&SetOperation, Side),
    (outer, names): (Option<&Outer<'_>>, &[&str]),
) -> Result<Vec<OutColumn>> {
    let nesting = element_nesting(outer);
    let (left, right) = (bind_query_expression(cx, &s.left, &nesting)?, bind_query_expression(cx, &s.right, &nesting)?);
    let recursive = match side {
        Side::Left => (&s.right, &right),
        Side::Right => (&s.left, &left),
    };
    let derived = recursively_derived(cx, recursive, (s, outer), names)?;
    let (left, right) = aligned(&cx, s, (left, right))?;
    let first = match side {
        Side::Left => &left.columns,
        Side::Right => &right.columns,
    };
    let mut columns = Vec::new();
    for (i, (a, b)) in left.columns.iter().zip(&right.columns).enumerate() {
        if let (true, Some(f)) = (derived.get(i).copied().unwrap_or(true), first.get(i)) {
            columns.push(OutColumn { not_null: false, origin: None, ..f.clone() });
            continue;
        }
        let data_type = set_type(&[a.data_type.clone(), b.data_type.clone()], "§7.17 SR19b ii 2 B")?;
        columns.push(OutColumn { name: None, collation: result_collation(a, b), data_type, not_null: false, origin: None });
    }
    Ok(columns)
}

/// [ISO/IEC 9075-2:2023 §7.17 SR19b ii 2 A] Of each column of the anchor
/// expression whose UNION is `s`, whether it is recursively derived: of
/// its recursive operand `query`, bound as `bound`, whether a work table
/// of `names` is drawn from by its column of the result's.
fn recursively_derived(
    cx: Context<'_>,
    (query, bound): (&QueryExpression, &Bound),
    (s, site): (&SetOperation, Option<&Outer<'_>>),
    names: &[&str],
) -> Result<Vec<bool>> {
    let drawn = drawn_from(cx, query, site)?;
    let of_member = |from: &Vec<(QualifiedName, String)>| from.iter().any(|(t, _)| t.auth.is_empty() && names.contains(&t.ident.as_str()));
    let derived: Vec<bool> = drawn.columns.iter().map(of_member).collect();
    Ok(match &s.corresponding {
        Correspondence::By(list) => picked(&derived, &positions(&bound.columns, list)),
        Correspondence::Position | Correspondence::Names => derived,
    })
}

/// The places among `columns` of the columns named `names`.
fn positions(columns: &[OutColumn], names: &[String]) -> Vec<usize> {
    names.iter().filter_map(|n| columns.iter().position(|c| c.name.as_ref() == Some(n))).collect()
}

/// The items of `items` at `places`.
fn picked<T: Clone>(items: &[T], places: &[usize]) -> Vec<T> {
    places.iter().filter_map(|&p| items.get(p)).cloned().collect()
}

/// The operand of the UNION `s` at `side`.
fn operand(s: &SetOperation, side: Side) -> &QueryExpression {
    match side {
        Side::Left => &s.left,
        Side::Right => &s.right,
    }
}

/// The UNION an anchor expression's body is.
fn union_of(body: &QueryExpression) -> Result<&SetOperation> {
    match body {
        QueryExpression::SetOp(s) => Ok(s),
        QueryExpression::Spec(_) | QueryExpression::Values(_) | QueryExpression::With(_) => {
            Err(SqlError::new(Code::Api, "[DESIGN] an anchor expression is not a UNION"))
        }
    }
}

/// [ISO/IEC 9075-2:2023 §7.17 GR2c ii] The plans of the members of the
/// stratum of `members`, of the columns `columns` and their work tables'
/// `descs`, and how their fixpoint is found.
fn planned(
    cx: Context<'_>,
    members: &[Member<'_>],
    (columns, descs): (&[Vec<OutColumn>], &[Vec<ColumnDesc>]),
    queries: &Queries<'_>,
) -> Result<Fixpoint<LogicalPlan>> {
    let scope = work_tables(queries, members, descs, &|_| None);
    let inner = Context { queries: Some(&scope), draws: false, ..cx };
    let outer = queries.site();
    let mut plans = Vec::new();
    for (m, cols) in members.iter().zip(columns) {
        let plan = in_body(inner, &m.element.query, outer, &mut |within, body| {
            let nesting = element_nesting(outer);
            if m.anchor.is_none() {
                return Ok(conform(bind_query_expression(within, body, &nesting)?, cols));
            }
            let s = union_of(body)?;
            let operands = (bind_query_expression(within, &s.left, &nesting)?, bind_query_expression(within, &s.right, &nesting)?);
            Ok(assembled(&within, s, aligned(&within, s, operands)?, cols.clone()).plan)
        })
        .or_at(m.element.at)?;
        plans.push(plan);
    }
    Ok(iteration(plans, members))
}

/// How the fixpoint of the members' `plans` is found: of one anchor whose
/// UNION's recursive operand distributes over it, an iteration over the
/// rows each adds; else as the General Rules say.
fn iteration(mut plans: Vec<LogicalPlan>, members: &[Member<'_>]) -> Fixpoint<LogicalPlan> {
    let (Some(side), true) = (members.first().and_then(|m| m.anchor), plans.len() == 1) else { return Fixpoint::Whole(plans) };
    let Some(LogicalPlan::SetOp { op: SetOp::Union, all, compared: None, left, right }) = plans.pop() else {
        return Fixpoint::Whole(plans);
    };
    let (anchor, step) = match side {
        Side::Left => (left, right),
        Side::Right => (right, left),
    };
    if distributes(&step, !all) {
        return Fixpoint::Delta { anchor, step, distinct: !all };
    }
    let (first, second) = match side {
        Side::Left => (anchor, step),
        Side::Right => (step, anchor),
    };
    Fixpoint::Whole(vec![LogicalPlan::SetOp { op: SetOp::Union, all, compared: None, left: first, right: second }])
}

/// Whether `plan`, a recursive operand, distributes over the UNION ALL of
/// the rows of its stratum's work table, or over UNION if `distinct`: each
/// node over the work table reads it through one input, and keeps each row
/// of that input as often as it would of the input alone, or, `distinct`,
/// keeps it or not as it would.
fn distributes(plan: &LogicalPlan, distinct: bool) -> bool {
    if !reads_work(plan) {
        return true;
    }
    match plan {
        LogicalPlan::Filter { input, .. } | LogicalPlan::Project { input, .. } | LogicalPlan::Sort { input, .. } => {
            distributes(input, distinct)
        }
        // Its rows are found of each row of its input alone.
        LogicalPlan::Unnest(u) => distributes(&u.input, distinct),
        // A window function's value of a row is of every row of its
        // partition.
        LogicalPlan::Window(_) => false,
        LogicalPlan::Product { left, right } | LogicalPlan::OuterJoin { left, right, .. } => one_side(left, right, distinct),
        LogicalPlan::SetOp { op: SetOp::Union, all, left, right, .. } => {
            (*all || distinct) && distributes(left, distinct) && distributes(right, distinct)
        }
        LogicalPlan::SetOp { op: SetOp::Except | SetOp::Intersect, left, right, .. } => distinct && one_side(left, right, distinct),
        LogicalPlan::Distinct { input } => distinct && distributes(input, distinct),
        // A group made before any row is no group of one row.
        LogicalPlan::Aggregate { aggs, seeds, input, .. } => {
            distinct && aggs.is_empty() && seeds.is_empty() && distributes(input, distinct)
        }
        LogicalPlan::WorkTable { .. }
        | LogicalPlan::Scan { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Recursive { .. } => true,
    }
}

/// `distributes` of the operands `left` and `right` of a node: only one of
/// them may read the work table.
fn one_side(left: &LogicalPlan, right: &LogicalPlan, distinct: bool) -> bool {
    match (reads_work(left), reads_work(right)) {
        (true, true) => false,
        (true, false) => distributes(left, distinct),
        (false, true) => distributes(right, distinct),
        (false, false) => true,
    }
}

/// Whether `plan` reads a work table of its stratum: not one of a
/// recursive query in it, of a stratum of its own.
fn reads_work(plan: &LogicalPlan) -> bool {
    match plan {
        LogicalPlan::WorkTable { .. } => true,
        LogicalPlan::Recursive { .. } => false,
        LogicalPlan::Scan { .. }
        | LogicalPlan::Filter { .. }
        | LogicalPlan::Product { .. }
        | LogicalPlan::Aggregate { .. }
        | LogicalPlan::Project { .. }
        | LogicalPlan::Distinct { .. }
        | LogicalPlan::OuterJoin { .. }
        | LogicalPlan::SetOp { .. }
        | LogicalPlan::System { .. }
        | LogicalPlan::Values { .. }
        | LogicalPlan::Unnest(..)
        | LogicalPlan::Window(..)
        | LogicalPlan::Sort { .. } => plan.inputs().any(reads_work),
    }
}

/// [ISO/IEC 9075-2:2023 §7.17 SR23] What the columns of each of `members`
/// are drawn from, their work tables of the columns `descs`: found again,
/// each work table drawn from what its member's columns were found to be,
/// until they settle, as they do once each has reached every other.
fn drawn_members(cx: Context<'_>, members: &[Member<'_>], descs: &[Vec<ColumnDesc>], queries: &Queries<'_>) -> Result<Vec<Underlying>> {
    let mut found: Vec<Underlying> = descs.iter().map(|d| Underlying { leaves: Vec::new(), columns: vec![Vec::new(); d.len()] }).collect();
    for _ in 0..members.len().saturating_add(2) {
        let scope = work_tables(queries, members, descs, &|k| found.get(k).cloned());
        let inner = Context { queries: Some(&scope), draws: false, ..cx };
        let next = members.iter().map(|m| drawn_from(inner, &m.element.query, queries.site())).collect::<Result<Vec<_>>>()?;
        if next == found {
            return Ok(found);
        }
        found = next;
    }
    Err(SqlError::new(Code::Api, "[DESIGN] what a recursive query's columns are drawn from did not settle"))
}
