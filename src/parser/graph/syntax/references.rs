//! \[EXTENSION\] [P1 §4.4, §4.7, §5.3] The references of a GRAPH_TABLE's
//! conditions and columns to its variables: each of the kind its place
//! needs, a group variable only in an aggregate and a singleton only
//! outside one (GRF-0022), SAME's and ALL_DIFFERENT's of unconditional
//! singletons (GRF-0023), and in a prefilter no aggregate of a group
//! variable that may be unbounded (GRF-0032).
//!
//! A reference sees the variables of the places around it, the innermost
//! first: its element pattern's own variable, a singleton there; each
//! parenthesized path pattern's, before its quantifier; its path
//! pattern's; and the graph pattern's. A name none declares is a column
//! of the query around the GRAPH_TABLE, which the binder resolves. The
//! walk recurs no deeper than the statement nests, which the parser
//! bounds (RULES.md R-CF-6).

use super::{Declared, Degree, Exposed, Exposure, Grf, bound, declared, expression_exposure, join, restricts, rule};
use crate::ast::{
    CaseExpression, ColumnSpecification, ElementPattern, GraphColumn, GraphPredicate, GraphTable, GraphVariable, ParenthesizedPath,
    PathExpression, PathPrimary, Predicate, SearchCondition, SegmentFilter, ValueExpression, VariableKind, WhenOperand,
};
use crate::error::Result;
use std::cell::RefCell;
use std::rc::Rc;

/// Checks the references of `table`'s conditions and columns, `top` the
/// variables its postfilter and columns see.
pub(super) fn check_references(table: &GraphTable, top: &Exposure) -> Result<()> {
    walked(table, top).map(|_| ())
}

/// The patterns of EXISTS IN PATH in `table`'s conditions and columns,
/// in the order written, its references checked as `check_references`
/// checks them.
pub(super) fn walked(table: &GraphTable, top: &Exposure) -> Result<Vec<ParenthesizedPath>> {
    let found = Found::default();
    let pattern = &table.pattern;
    for path in &pattern.paths {
        let mut seen = expression_exposure(&path.expression)?;
        if restricts(path.prefix.as_ref()) || restricts(pattern.keep.as_ref()) {
            bound(&mut seen);
        }
        let restricted = restricts(path.prefix.as_ref()) || restricts(pattern.keep.as_ref());
        let mut walk = Walk { scopes: vec![top.clone(), seen], prefilter: true, restricted, found: Rc::clone(&found) };
        walk.expression(&path.expression)?;
    }
    let post = Walk { scopes: vec![top.clone()], prefilter: false, restricted: false, found: Rc::clone(&found) };
    if let Some(c) = &pattern.where_clause {
        post.condition(c, false)?;
    }
    for segment in &pattern.segments {
        segment_filter(segment, top, &found)?;
    }
    for column in &table.columns {
        match column {
            GraphColumn::Value { value, .. } => post.value(value, false)?,
            GraphColumn::All(v) => post.operand(v, VariableKind::Element, false)?,
        }
    }
    drop(post);
    Ok(found.take())
}

/// The patterns of EXISTS IN PATH a walk has met.
type Found = Rc<RefCell<Vec<ParenthesizedPath>>>;

/// \[EXTENSION\] The segment filter `s`: of a path variable, its two
/// edge variables new names, its condition of them and of `top`.
fn segment_filter(s: &SegmentFilter, top: &Exposure, found: &Found) -> Result<()> {
    let post = Walk { scopes: vec![top.clone()], prefilter: false, restricted: false, found: Rc::clone(found) };
    post.declared(&s.path, VariableKind::Path)?;
    for v in [&s.first, &s.second] {
        if top.contains_key(&v.name) {
            return Err(rule(v.at, &format!("{} of FOR EACH SEGMENT is a variable the pattern does not declare", v.name)));
        }
    }
    if s.first.name == s.second.name {
        return Err(rule(s.second.at, "the two edges of FOR EACH SEGMENT have two names"));
    }
    let edges = join(declared(&s.first, Declared::Edge), declared(&s.second, Declared::Edge))?;
    let inner = Walk { scopes: vec![top.clone(), edges], prefilter: false, restricted: false, found: Rc::clone(found) };
    inner.condition(&s.condition, false)
}

/// A walk of the references of conditions at one place.
struct Walk {
    /// The variables of the places around it, the outermost first.
    scopes: Vec<Exposure>,
    /// Whether its conditions are prefilters, applied before any
    /// selector.
    prefilter: bool,
    /// Whether a restrictor other than WALK is around, which bounds every
    /// group variable.
    restricted: bool,
    /// The patterns of EXISTS IN PATH met.
    found: Found,
}

impl Walk {
    /// The prefilters of `expression`'s element patterns and
    /// parenthesized path patterns.
    fn expression(&mut self, expression: &PathExpression) -> Result<()> {
        for factor in expression.terms().iter().flat_map(|t| &t.factors) {
            match &factor.primary {
                PathPrimary::Node(e) => self.element(e)?,
                PathPrimary::Edge(e) => self.element(&e.filler)?,
                PathPrimary::Parenthesized(p) => self.parenthesized(p)?,
            }
        }
        Ok(())
    }

    /// The prefilters of the parenthesized path pattern `p`, its own
    /// variables seen as from its inside.
    fn parenthesized(&mut self, p: &ParenthesizedPath) -> Result<()> {
        let mut inner = expression_exposure(&p.expression)?;
        if let Some(v) = &p.variable {
            inner = join(inner, declared(v, Declared::Path))?;
        }
        let outer = self.restricted;
        self.restricted = outer || restricts(p.prefix.as_ref());
        if self.restricted {
            bound(&mut inner);
        }
        self.scopes.push(inner);
        let checked = p.where_clause.as_ref().map_or(Ok(()), |c| self.condition(c, false));
        let nested = checked.and_then(|()| self.expression(&p.expression));
        self.scopes.pop();
        self.restricted = outer;
        nested
    }

    /// The prefilter and properties of the element pattern `e`, its own
    /// variable a singleton there.
    fn element(&mut self, e: &ElementPattern) -> Result<()> {
        let own = e.variable.as_ref().map_or_else(Exposure::new, |v| declared(v, Declared::Vertex));
        self.scopes.push(own.into_iter().map(|(n, x)| (n.clone(), self.own(&n).unwrap_or(x))).collect());
        let checked = e.where_clause.as_ref().map_or(Ok(()), |c| self.condition(c, false));
        let all = checked.and_then(|()| e.properties.iter().try_for_each(|p| self.value(&p.value, false)));
        self.scopes.pop();
        all
    }

    /// The variable `name` as a singleton of the kind the places around
    /// declare it as, if one declares it.
    fn own(&self, name: &str) -> Option<Exposed> {
        self.lookup(name).map(|x| Exposed { degree: Degree::Singleton, ..x })
    }

    /// The variable `name` as the innermost place declaring it sees it.
    fn lookup(&self, name: &str) -> Option<Exposed> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    /// The references of `c`, in an aggregate's argument if `aggregated`.
    fn condition(&self, c: &SearchCondition, aggregated: bool) -> Result<()> {
        match c {
            SearchCondition::Or(a, b) | SearchCondition::And(a, b) => {
                self.condition(a, aggregated)?;
                self.condition(b, aggregated)
            }
            SearchCondition::Not(a) | SearchCondition::Is(a, ..) => self.condition(a, aggregated),
            SearchCondition::Predicate(p, _) => {
                if let Predicate::Graph(g) = &**p {
                    return self.predicate(g, aggregated);
                }
                let mut result = Ok(());
                p.for_each_value(&mut |v| {
                    if result.is_ok() {
                        result = self.value(v, aggregated);
                    }
                });
                result
            }
            SearchCondition::Predicand(v, _) => self.value(v, aggregated),
        }
    }

    /// The references of `v`, in an aggregate's argument if `aggregated`.
    fn value(&self, v: &ValueExpression, aggregated: bool) -> Result<()> {
        if let ValueExpression::Column(c) = v {
            return self.column(c, aggregated);
        }
        if let ValueExpression::Graph(f, _) = v {
            return self.operand(&f.operand, f.name.operand(), aggregated);
        }
        if let ValueExpression::Boolean(c) = v {
            return self.condition(c, aggregated);
        }
        if let ValueExpression::Case(case) = v {
            return self.case(case, aggregated);
        }
        let inner = aggregated || matches!(v, ValueExpression::SetFunction(_));
        if let ValueExpression::SetFunction(sf) = v
            && let Some(filter) = &sf.filter
        {
            self.condition(filter, inner)?;
        }
        let mut result = Ok(());
        v.for_each_child(&mut |child| {
            if result.is_ok() {
                result = self.value(child, inner);
            }
        });
        result
    }

    /// The references of the CASE `case`, its conditions' among them.
    fn case(&self, case: &CaseExpression, aggregated: bool) -> Result<()> {
        let values = |list: &[&ValueExpression]| list.iter().try_for_each(|x| self.value(x, aggregated));
        match case {
            CaseExpression::NullIf(a, b) => values(&[a, b]),
            CaseExpression::Coalesce(list) => list.iter().try_for_each(|x| self.value(x, aggregated)),
            CaseExpression::Simple { operand, whens, otherwise } => {
                operand.iter().try_for_each(|x| self.value(x, aggregated))?;
                for (list, result) in whens {
                    for when in list {
                        match when {
                            WhenOperand::Value(row, _) => row.iter().try_for_each(|x| self.value(x, aggregated))?,
                            WhenOperand::Condition(c) => self.condition(c, aggregated)?,
                        }
                    }
                    result.iter().try_for_each(|x| self.value(x, aggregated))?;
                }
                otherwise.iter().try_for_each(|x| self.value(x, aggregated))
            }
            CaseExpression::Searched { whens, otherwise } => {
                for (c, result) in whens {
                    self.condition(c, aggregated)?;
                    result.iter().try_for_each(|x| self.value(x, aggregated))?;
                }
                otherwise.iter().try_for_each(|x| self.value(x, aggregated))
            }
        }
    }

    /// A column reference `c`: of a variable's property, `x`.`p`, or of
    /// the element itself, `x`, if `x` is a variable.
    fn column(&self, c: &ColumnSpecification, aggregated: bool) -> Result<()> {
        if c.catalog.is_some() {
            return Ok(());
        }
        let (name, property) = match &c.qualifier {
            Some(q) => {
                if q.auth.is_some() {
                    return Ok(());
                }
                (q.ident.as_str(), true)
            }
            None => (c.column.as_str(), false),
        };
        let Some(e) = self.lookup(name) else { return Ok(()) };
        if property && e.kind == Declared::Path {
            return Err(rule(c.at, &format!("{name} is a path variable, which has no properties")));
        }
        self.degree(name, e.degree, aggregated, c.at)
    }

    /// The graph predicate `g`'s operands.
    fn predicate(&self, g: &GraphPredicate, aggregated: bool) -> Result<()> {
        for (v, kind) in g.operands() {
            let e = self.declared(v, kind)?;
            if g.needs_unconditional() && e.degree != Degree::Singleton {
                let msg = format!("[P1 §4.7] {} is no unconditional singleton here, as SAME and ALL_DIFFERENT need", v.name);
                return Err(Grf::SameOperand.at(v.at, &msg));
            }
            self.degree(&v.name, e.degree, aggregated, v.at)?;
        }
        if let GraphPredicate::InPath { pattern, .. } = g {
            self.found.borrow_mut().push((**pattern).clone());
            let found = Rc::clone(&self.found);
            let mut inner = Walk { scopes: self.scopes.clone(), prefilter: self.prefilter, restricted: self.restricted, found };
            inner.parenthesized(pattern)?;
        }
        Ok(())
    }

    /// The operand `v` of a graph function or of `<variable>`.*, which is
    /// of the kind `kind`.
    fn operand(&self, v: &GraphVariable, kind: VariableKind, aggregated: bool) -> Result<()> {
        let e = self.declared(v, kind)?;
        self.degree(&v.name, e.degree, aggregated, v.at)
    }

    /// The variable `v` as seen here, which is declared and of the kind
    /// `kind`.
    fn declared(&self, v: &GraphVariable, kind: VariableKind) -> Result<Exposed> {
        let Some(e) = self.lookup(&v.name) else {
            return Err(rule(v.at, &format!("{} is not a variable of the graph pattern", v.name)));
        };
        let fits = match kind {
            VariableKind::Element => e.kind != Declared::Path,
            VariableKind::Vertex => e.kind == Declared::Vertex,
            VariableKind::Edge => e.kind == Declared::Edge,
            VariableKind::Path => e.kind == Declared::Path,
            VariableKind::ElementOrPath => true,
        };
        if !fits {
            let what = match kind {
                VariableKind::Element => "an element variable",
                VariableKind::Vertex => "a vertex variable",
                VariableKind::Edge => "an edge variable",
                VariableKind::Path => "a path variable",
                VariableKind::ElementOrPath => "a variable",
            };
            return Err(rule(v.at, &format!("{} is not {what}, which is needed here", v.name)));
        }
        Ok(e)
    }

    /// [P1 §4.4, §5.3] A reference to `name`, of degree `degree` here, in
    /// an aggregate's argument if `aggregated`.
    fn degree(&self, name: &str, degree: Degree, aggregated: bool, at: usize) -> Result<()> {
        let group = matches!(degree, Degree::Group { .. });
        if group && !aggregated {
            return Err(
                Grf::GroupAsSingleton.at(at, &format!("[P1 §4.4] {name} is a group variable here, referenced only in an aggregate"))
            );
        }
        if aggregated && !group {
            return Err(Grf::GroupAsSingleton.at(at, &format!("[P1 §4.4] {name} is a singleton here, which no aggregate takes")));
        }
        if self.prefilter && aggregated && degree == (Degree::Group { unbounded: true }) {
            let msg = format!("[P1 §5.3] a prefilter takes no aggregate of {name}, a group variable that may be unbounded here");
            return Err(Grf::UnboundedAggregate.at(at, &msg));
        }
        Ok(())
    }
}
