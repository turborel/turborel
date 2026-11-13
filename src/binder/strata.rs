//! [ISO/IEC 9075-2:2023 §7.17 SR3h, SR3j] The rules of a potentially
//! recursive `<with list>` that its syntax decides: the query name
//! dependency graph of its elements and its strata [SR3j ii, v], each
//! stratum's anchors [vii, viii], and what the query of each recursive
//! element may be [ix, x].
//!
//! A query name of the list references its element unless a `<with
//! clause>` within the query has one of that name in scope there [SR3i].
//! A reference to a member of the stratum is allowed only in a `<from
//! clause>` of a `<query specification>` of the element's `<query
//! expression body>`, or of one of a derived table there [ix 5 B]; and not
//! twice in one `<query specification>` [ix 2], beside a set function of
//! its select list or HAVING clause [C], under INTERSECT ALL or EXCEPT ALL
//! [D], in the second operand of EXCEPT [E], or in an operand of an outer
//! join that may be null [F, G]. These keep each element's query monotone
//! in the stratum's tables, so it has a fixpoint [NOTE 317]. A UNION JOIN
//! is a FULL one on a condition never true, and is held to F as one.

use std::collections::BTreeSet;

use super::rule;
use super::set_functions::is_aggregate_call;
use crate::ast::{
    Correspondence, JoinKind, JoinSpecification, QueryExpression, QuerySpecification, RowElement, SearchCondition, SelectItem, SelectList,
    SelectSublist, SetOperator, Subquery, TableReference, ValueExpression, WithElement, WithQuery,
};
use crate::db::State;
use crate::error::{Code, Result, SqlError};

/// [ISO/IEC 9075-2:2023 §7.17 SR3j v] A stratum of a potentially recursive
/// list: its elements, by their places.
pub(super) struct Stratum {
    /// Its members' places, in the order written.
    pub(super) members: Vec<usize>,
    /// Whether its members are recursive [SR3j iv, vi].
    pub(super) recursive: bool,
}

/// [ISO/IEC 9075-2:2023 §7.17 SR3j ii] The query name dependency graph of
/// `w`: of each element, by place, the elements its query references.
pub(super) fn graph(w: &WithQuery) -> Vec<Vec<usize>> {
    let place = |name: &str| w.elements.iter().position(|e| e.name == name);
    w.elements
        .iter()
        .map(|e| {
            let mut on = Vec::new();
            e.query.for_each_named(&mut |t, query| {
                let referenced = (!query && t.table.auth.is_none()).then(|| place(&t.table.ident)).flatten();
                if let Some(j) = referenced.filter(|j| !on.contains(j)) {
                    on.push(j);
                }
            });
            on
        })
        .collect()
}

/// The nodes of `arcs` reached from `from` by one arc or more, through
/// nodes `kept` says are kept alone.
fn reached(arcs: &[Vec<usize>], from: usize, kept: &dyn Fn(usize) -> bool) -> BTreeSet<usize> {
    let next = |n: usize| arcs.get(n).into_iter().flatten().copied().filter(|&m| kept(m));
    let mut seen = BTreeSet::new();
    let mut todo: Vec<usize> = next(from).collect();
    while let Some(n) = todo.pop() {
        if seen.insert(n) {
            todo.extend(next(n));
        }
    }
    seen
}

/// [ISO/IEC 9075-2:2023 §7.17 SR3j v, GR2b] The strata of the graph
/// `arcs`, each after every stratum it depends on, as they are evaluated.
pub(super) fn strata(arcs: &[Vec<usize>]) -> Vec<Stratum> {
    let reach: Vec<BTreeSet<usize>> = (0..arcs.len()).map(|n| reached(arcs, n, &|_| true)).collect();
    let reaches = |a: usize, b: usize| reach.get(a).is_some_and(|r| r.contains(&b));
    let mut found: Vec<Stratum> = Vec::new();
    for n in 0..arcs.len() {
        if found.iter().any(|s| s.members.contains(&n)) {
            continue;
        }
        let members = (0..arcs.len()).filter(|&m| m == n || (reaches(n, m) && reaches(m, n))).collect();
        found.push(Stratum { members, recursive: reaches(n, n) });
    }
    let mut ordered = Vec::new();
    let mut placed = BTreeSet::new();
    while !found.is_empty() {
        let ready = |s: &Stratum| {
            s.members.iter().all(|&m| reach.get(m).into_iter().flatten().all(|r| placed.contains(r) || s.members.contains(r)))
        };
        let stratum = found.remove(found.iter().position(ready).unwrap_or(0));
        placed.extend(stratum.members.iter().copied());
        ordered.push(stratum);
    }
    ordered
}

/// Which operand of an anchor expression's UNION is its non-recursive
/// operand [ISO/IEC 9075-2:2023 §7.17 SR3j vii 1 B].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Side {
    /// The first.
    Left,
    /// The second.
    Right,
}

/// Where in an element's query a part of it is [ISO/IEC 9075-2:2023 §7.17
/// SR3j ix 5 B].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Of its `<query expression body>`: a reference to the stratum may be
    /// in its `<from clause>`s, a derived table's too.
    Body,
    /// Of a derived table of a `<from clause>` of the body: a reference may
    /// be in its own.
    Derived,
    /// Anywhere else: none may be.
    Elsewhere,
}

/// [ISO/IEC 9075-2:2023 §7.17 SR3j] The rules of the queries of a
/// recursive stratum: its members' names, and the database, whose
/// extension aggregates are set functions too.
pub(super) struct Rules<'w> {
    names: Vec<&'w str>,
    db: &'w State,
}

impl<'w> Rules<'w> {
    /// The rules of the stratum of `w`'s elements at `members`.
    pub(super) fn new(db: &'w State, w: &'w WithQuery, members: &[usize]) -> Rules<'w> {
        let names = members.iter().filter_map(|&m| w.elements.get(m)).map(|e| e.name.as_str()).collect();
        Rules { names, db }
    }

    /// Its members' names, for a message.
    fn listed(&self) -> String {
        self.names.join(", ")
    }

    /// How many references to the stratum `q` has, the query names of
    /// `scope` in scope.
    fn refs(&self, q: &'w QueryExpression, scope: &[&'w str]) -> usize {
        let (mut scope, mut n) = (scope.to_vec(), 0_usize);
        q.named_in(&mut scope, &mut |t, query| {
            if !query && t.table.auth.is_none() && self.names.contains(&t.table.ident.as_str()) {
                n = n.saturating_add(1);
            }
        });
        n
    }

    /// How many references to the stratum the `<table reference>` `r` has,
    /// the query names of `scope` in scope.
    fn reference_refs(&self, r: &'w TableReference, scope: &[&'w str]) -> usize {
        match r {
            TableReference::Table(t) => {
                let named = t.table.auth.is_none() && !scope.contains(&t.table.ident.as_str());
                usize::from(named && self.names.contains(&t.table.ident.as_str()))
            }
            TableReference::Derived(d) => self.refs(&d.query.query, scope),
            TableReference::Unnest(u) => {
                let mut n = 0_usize;
                for v in &u.collections {
                    v.for_each_subquery(&mut |s| n = n.saturating_add(self.refs(&s.query, scope)));
                }
                n
            }
            TableReference::Joined(j) => {
                let mut n = self.reference_refs(&j.left, scope).saturating_add(self.reference_refs(&j.right, scope));
                if let Some(JoinSpecification::On(c)) = &j.on {
                    c.for_each_subquery(&mut |s| n = n.saturating_add(self.refs(&s.query, scope)));
                }
                n
            }
        }
    }

    /// [ISO/IEC 9075-2:2023 §7.17 SR3j ix, x] Checks the query of `e`, a
    /// member of the stratum.
    pub(super) fn check(&self, e: &'w WithElement) -> Result<()> {
        self.query(&e.query, (Place::Body, &mut Vec::new())).map_err(|(clause, what)| {
            rule(format!("[ISO/IEC 9075-2:2023 §7.17 {clause}] query {} references {} {what}", e.name, self.listed())).or_at(e.at)
        })
    }

    /// `check` of `q`, at `place` in the element's query, the query names
    /// of `scope` in scope there.
    fn query(&self, q: &'w QueryExpression, (place, scope): (Place, &mut Vec<&'w str>)) -> Checked {
        if self.refs(q, scope) == 0 {
            return Ok(());
        }
        if place == Place::Elsewhere {
            return Err(("SR3j ix 5 B", "outside a <from clause> of its <query expression body> or of a derived table there"));
        }
        if simply_all(q) {
            return Err(("SR3j ix 5 D", "in a <query expression body> with INTERSECT ALL or EXCEPT ALL"));
        }
        match q {
            QueryExpression::With(w) => self.with(w, (place, scope)),
            QueryExpression::SetOp(s) => {
                if s.op == SetOperator::Except && self.refs(&s.right, scope) > 0 {
                    return Err(("SR3j ix 5 E", "in the second operand of EXCEPT"));
                }
                self.query(&s.left, (place, scope))?;
                self.query(&s.right, (place, scope))
            }
            QueryExpression::Values(rows) => {
                let mut subqueries = Vec::new();
                for v in row_values(rows) {
                    v.for_each_subquery(&mut |s| subqueries.push(s));
                }
                self.elsewhere(subqueries, scope)
            }
            QueryExpression::Spec(spec) => self.spec(spec, (place, scope)),
        }
    }

    /// `query` of a `<with clause>` within the element's query: its
    /// elements' queries anywhere else, its body where it is.
    fn with(&self, w: &'w WithQuery, (place, scope): (Place, &mut Vec<&'w str>)) -> Checked {
        let base = scope.len();
        if w.recursive {
            scope.extend(w.elements.iter().map(|e| e.name.as_str()));
        }
        for e in &w.elements {
            self.query(&e.query, (Place::Elsewhere, scope))?;
            if !w.recursive {
                scope.push(&e.name);
            }
        }
        let body = self.query(&w.body, (place, scope));
        scope.truncate(base);
        body
    }

    /// `query` of the `<query specification>` `spec`.
    fn spec(&self, spec: &'w QuerySpecification, (place, scope): (Place, &mut Vec<&'w str>)) -> Checked {
        let te = &spec.table_expression;
        let refs: usize = te.from.iter().map(|r| self.reference_refs(r, scope)).sum();
        if refs > 1 {
            return Err(("SR3j ix 2, x", "twice in one <query specification>, so it is not linearly recursive"));
        }
        for r in &te.from {
            self.reference(r, (place, scope))?;
        }
        let mut subqueries = Vec::new();
        for c in te.where_clause.iter().chain(&te.having) {
            c.for_each_subquery(&mut |s| subqueries.push(s));
        }
        for v in selected(spec) {
            v.for_each_subquery(&mut |s| subqueries.push(s));
        }
        self.elsewhere(subqueries, scope)?;
        if refs > 0 && self.aggregated(spec) {
            return Err(("SR3j ix 5 C", "in a <query specification> with a set function in its select list or HAVING clause"));
        }
        Ok(())
    }

    /// `query` of each of `subqueries`, none of which may reference the
    /// stratum.
    fn elsewhere(&self, subqueries: Vec<&'w Subquery>, scope: &[&'w str]) -> Checked {
        subqueries.into_iter().try_for_each(|s| self.query(&s.query, (Place::Elsewhere, &mut scope.to_vec())))
    }

    /// Whether the select list or HAVING clause of `spec` contains a set
    /// function [ISO/IEC 9075-2:2023 §7.17 SR3j ix 5 C].
    fn aggregated(&self, spec: &QuerySpecification) -> bool {
        let having = spec.table_expression.having.as_ref();
        selected(spec).into_iter().any(|v| self.has_set_function(v)) || having.is_some_and(|c| self.condition_has_set_function(c))
    }

    /// `query` of the `<table reference>` `r` of a `<from clause>` at
    /// `place`.
    fn reference(&self, r: &'w TableReference, (place, scope): (Place, &mut Vec<&'w str>)) -> Checked {
        match r {
            TableReference::Table(_) => Ok(()),
            TableReference::Derived(d) => {
                let inner = if place == Place::Body { Place::Derived } else { Place::Elsewhere };
                self.query(&d.query.query, (inner, scope))
            }
            TableReference::Unnest(u) => {
                let mut subqueries = Vec::new();
                for v in &u.collections {
                    v.for_each_subquery(&mut |s| subqueries.push(s));
                }
                self.elsewhere(subqueries, scope)
            }
            TableReference::Joined(j) => {
                let (left, right) = (self.reference_refs(&j.left, scope), self.reference_refs(&j.right, scope));
                let refused = match j.kind {
                    JoinKind::Full | JoinKind::Union => left.saturating_add(right) > 0,
                    JoinKind::Left => right > 0,
                    JoinKind::Right => left > 0,
                    JoinKind::Cross | JoinKind::Inner => false,
                };
                if refused {
                    let clause = if j.natural { "SR3j ix 5 G" } else { "SR3j ix 5 F" };
                    return Err((clause, "in an operand of a FULL, LEFT or RIGHT join whose rows may be given nulls"));
                }
                let mut subqueries = Vec::new();
                if let Some(JoinSpecification::On(c)) = &j.on {
                    c.for_each_subquery(&mut |s| subqueries.push(s));
                }
                self.elsewhere(subqueries, scope)?;
                self.reference(&j.left, (place, scope))?;
                self.reference(&j.right, (place, scope))
            }
        }
    }

    /// Whether `v` contains a set function, in a subquery too.
    fn has_set_function(&self, v: &ValueExpression) -> bool {
        let mut found = v.any(&|e| is_aggregate_call(e, self.db));
        v.for_each_subquery(&mut |s| found = found || any_query(&s.query, &|q| self.own_set_function(q)));
        found
    }

    /// Whether `c` contains a set function, in a subquery too.
    fn condition_has_set_function(&self, c: &SearchCondition) -> bool {
        let mut found = false;
        c.for_each_value(&mut |v| found = found || v.any(&|e| is_aggregate_call(e, self.db)));
        c.for_each_subquery(&mut |s| found = found || any_query(&s.query, &|q| self.own_set_function(q)));
        found
    }

    /// Whether a value expression of `q` itself, not of a query nested in
    /// it, is or has a set function.
    fn own_set_function(&self, q: &QueryExpression) -> bool {
        let agg = |v: &ValueExpression| v.any(&|e| is_aggregate_call(e, self.db));
        match q {
            QueryExpression::Spec(spec) => {
                let mut found = selected(spec).into_iter().any(agg);
                for c in conditions(spec) {
                    c.for_each_value(&mut |v| found = found || agg(v));
                }
                found
            }
            QueryExpression::Values(rows) => row_values(rows).into_iter().any(agg),
            QueryExpression::SetOp(_) | QueryExpression::With(_) => false,
        }
    }

    /// [ISO/IEC 9075-2:2023 §7.17 SR3j vii 1] Which operand of the UNION
    /// of `e`'s query is its non-recursive operand, if `e`'s query is an
    /// anchor expression. One whose UNION of CORRESPONDING without BY is
    /// not supported, nor one whose anchor's UNION is within its body.
    pub(super) fn anchor(&self, e: &'w WithElement) -> Result<Option<Side>> {
        let (mut q, mut scope) = (&e.query, Vec::new());
        while let QueryExpression::With(w) = q {
            scope.extend(w.elements.iter().map(|x| x.name.as_str()));
            q = &w.body;
        }
        let QueryExpression::SetOp(s) = q else { return Ok(None) };
        let side = match (s.op, self.refs(&s.left, &scope), self.refs(&s.right, &scope)) {
            (SetOperator::Union, 0, _) => Some(Side::Left),
            (SetOperator::Union, _, 0) => Some(Side::Right),
            (SetOperator::Union | SetOperator::Except | SetOperator::Intersect, _, _) => None,
        };
        if side.is_some() && matches!(s.corresponding, Correspondence::Names) {
            return Err(unsupported(
                "an anchor expression whose UNION is CORRESPONDING without BY [ISO/IEC 9075-2:2023 §7.17 SR3j vii]",
                e,
            ));
        }
        if side.is_none() && self.anchored_within(q, &scope) {
            return Err(unsupported(
                "an anchor expression whose UNION is within its <query expression body> [ISO/IEC 9075-2:2023 §7.17 SR3j vii]",
                e,
            ));
        }
        Ok(side)
    }

    /// Whether an operand of `q`'s set operation, through set operations,
    /// is a UNION one of whose operands does not reference the stratum.
    fn anchored_within(&self, q: &'w QueryExpression, scope: &[&'w str]) -> bool {
        let QueryExpression::SetOp(s) = q else { return false };
        let union = |o: &'w QueryExpression| match o {
            QueryExpression::SetOp(x) => x.op == SetOperator::Union && (self.refs(&x.left, scope) == 0 || self.refs(&x.right, scope) == 0),
            QueryExpression::Spec(_) | QueryExpression::Values(_) | QueryExpression::With(_) => false,
        };
        [&s.left, &s.right].into_iter().any(|o| self.refs(o, scope) > 0 && (union(o) || self.anchored_within(o, scope)))
    }

    /// [ISO/IEC 9075-2:2023 §7.17 SR3j vii, viii] Checks that the stratum
    /// of `members`, of the graph `arcs`, those of them at `anchors` its
    /// anchors, has one, and that every cycle of it has one.
    pub(super) fn anchored(&self, arcs: &[Vec<usize>], members: &[usize], anchors: &[usize]) -> Result<()> {
        if anchors.is_empty() {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §7.17 SR3j vii] the recursive queries {} have no anchor: none is a UNION with an operand that references none of them",
                self.listed()
            )));
        }
        let kept = |n: usize| members.contains(&n) && !anchors.contains(&n);
        if members.iter().any(|&m| kept(m) && reached(arcs, m, &kept).contains(&m)) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §7.17 SR3j viii] a cycle of the recursive queries {} has no anchor",
                self.listed()
            )));
        }
        Ok(())
    }
}

/// The rule a part of a recursive element's query breaks, and how.
type Checked = std::result::Result<(), (&'static str, &'static str)>;

/// Code::Unsupported of `what`, of the element `e`.
fn unsupported(what: &str, e: &WithElement) -> SqlError {
    SqlError::new(Code::Unsupported, format!("{what} is not supported, as query {} is", e.name)).or_at(e.at)
}

/// [ISO/IEC 9075-2:2023 §7.17 SR3j ix 5 D] Whether the `<query expression
/// body>` of `q` simply contains INTERSECT ALL or EXCEPT ALL.
fn simply_all(q: &QueryExpression) -> bool {
    match q {
        QueryExpression::SetOp(s) => (s.all && s.op != SetOperator::Union) || simply_all(&s.left) || simply_all(&s.right),
        QueryExpression::With(w) => simply_all(&w.body),
        QueryExpression::Spec(_) | QueryExpression::Values(_) => false,
    }
}

/// [ISO/IEC 9075-2:2023 §7.17 SR3h] Whether `q` contains a potentially
/// recursive `<with list>`.
pub(super) fn has_recursive_list(q: &QueryExpression) -> bool {
    any_query(q, &|x| matches!(x, QueryExpression::With(w) if w.recursive))
}

/// Whether `pred` holds of `q` or of a query expression it contains: an
/// operand, an element's query or body, a derived table's, a subquery's.
fn any_query(q: &QueryExpression, pred: &dyn Fn(&QueryExpression) -> bool) -> bool {
    if pred(q) {
        return true;
    }
    let mut nested: Vec<&QueryExpression> = Vec::new();
    match q {
        QueryExpression::SetOp(s) => nested.extend([&s.left, &s.right]),
        QueryExpression::With(w) => nested.extend(w.elements.iter().map(|e| &e.query).chain([&w.body])),
        QueryExpression::Values(rows) => row_values(rows).into_iter().for_each(|v| v.for_each_subquery(&mut |s| nested.push(&s.query))),
        QueryExpression::Spec(spec) => {
            for v in selected(spec) {
                v.for_each_subquery(&mut |s| nested.push(&s.query));
            }
            for c in conditions(spec) {
                c.for_each_subquery(&mut |s| nested.push(&s.query));
            }
            let mut refs: Vec<&TableReference> = spec.table_expression.from.iter().collect();
            while let Some(r) = refs.pop() {
                match r {
                    TableReference::Table(_) => {}
                    TableReference::Derived(d) => nested.push(&d.query.query),
                    TableReference::Unnest(u) => {
                        for v in &u.collections {
                            v.for_each_subquery(&mut |s| nested.push(&s.query));
                        }
                    }
                    TableReference::Joined(j) => refs.extend([&j.left, &j.right]),
                }
            }
        }
    }
    nested.into_iter().any(|n| any_query(n, pred))
}

/// The value expressions of the select list of `spec`.
fn selected(spec: &QuerySpecification) -> Vec<&ValueExpression> {
    match &spec.select_list {
        SelectList::Star => Vec::new(),
        SelectList::Values(items) => items
            .iter()
            .filter_map(|i| match i {
                SelectSublist::Derived(SelectItem { value, .. }) | SelectSublist::Fields { value, .. } => Some(value),
                SelectSublist::All(_) => None,
            })
            .collect(),
    }
}

/// The search conditions of `spec`'s own: its WHERE and HAVING clauses',
/// and its join conditions'.
fn conditions(spec: &QuerySpecification) -> Vec<&SearchCondition> {
    let te = &spec.table_expression;
    let mut out: Vec<&SearchCondition> = te.where_clause.iter().chain(&te.having).collect();
    let mut refs: Vec<&TableReference> = te.from.iter().collect();
    while let Some(r) = refs.pop() {
        if let TableReference::Joined(j) = r {
            if let Some(JoinSpecification::On(c)) = &j.on {
                out.push(c);
            }
            refs.extend([&j.left, &j.right]);
        }
    }
    out
}

/// The value expressions of the rows of a `<table value constructor>`.
fn row_values(rows: &[crate::ast::ValuesRow]) -> Vec<&ValueExpression> {
    rows.iter()
        .flat_map(|r| &r.elements)
        .filter_map(|e| match e {
            RowElement::Value(v, _) => Some(v),
            RowElement::Null(_) | RowElement::Default(_) => None,
        })
        .collect()
}
