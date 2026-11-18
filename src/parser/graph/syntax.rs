//! \[EXTENSION\] The Syntax Rules of a GRAPH_TABLE that need no catalog,
//! checked once it is read: the kind and degree of exposure of each
//! variable [P1 §4.4, §4.6], the joins they may take part in, the
//! termination rules [P1 §5], and where selectors and restrictors may be
//! written. Each refusal is SQLCODE -103 and names the GRF condition of
//! the graph standard it is.
//!
//! A variable's degree of exposure is seen from a place in the pattern:
//! a singleton, bound to one element per match; a conditional singleton,
//! bound in some alternatives of a union, or under `?`, so perhaps null;
//! or a group variable, declared under a quantifier the place is outside
//! of, bound to the list of its iterations' elements. `exposure` gives
//! each variable's, seen from outside a path pattern expression; the
//! references of conditions are checked against it in `references.rs`.

mod references;

use std::collections::BTreeMap;

use crate::ast::{
    GraphPattern, GraphRows, GraphTable, GraphVariable, Literal, PathExpression, PathFactor, PathPrefix, PathPrimary, Quantifier, Selector,
    Sign, ValueExpression, ValueSpecification,
};
use crate::error::{Code, Result, SqlError};

/// [P1 §4.4, §4.6] How a variable is bound, seen from a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Degree {
    /// To one element, or path, per match.
    Singleton,
    /// To one element, or none: declared in some alternatives of a union
    /// only, or under `?`.
    Conditional,
    /// To the list of a quantifier's iterations' elements; `unbounded` if
    /// a quantifier of no upper bound was crossed with no restrictor
    /// other than WALK around it [P1 §5.3].
    Group {
        /// Whether the list may be unbounded.
        unbounded: bool,
    },
}

/// What a variable is declared as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Declared {
    /// A node pattern's variable.
    Vertex,
    /// An edge pattern's variable.
    Edge,
    /// A path or subpath variable.
    Path,
}

/// A variable, seen from a place: what it is declared as, its degree, and
/// where it is first declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exposed {
    /// What it is declared as.
    pub kind: Declared,
    /// Its degree of exposure.
    pub degree: Degree,
    /// Where it is first declared.
    pub at: usize,
}

/// The variables a pattern declares, by name, seen from a place.
pub type Exposure = BTreeMap<String, Exposed>;

/// [Graph standard §22] The conditions the Syntax Rules of a GRAPH_TABLE
/// raise, each with its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Grf {
    /// GRF-0021: an implicit join on a conditional singleton.
    ConditionalJoin,
    /// GRF-0022: a group variable referenced as a singleton, or a
    /// singleton as a group variable.
    GroupAsSingleton,
    /// GRF-0023: an argument of SAME or ALL_DIFFERENT is no unconditional
    /// singleton.
    SameOperand,
    /// GRF-0031: an unbounded quantifier with no selector and no
    /// restrictor other than WALK.
    Unbounded,
    /// GRF-0032: an aggregate of an unbounded group variable in a
    /// prefilter.
    UnboundedAggregate,
    /// GRF-0033: a selector inside a parenthesized path pattern.
    NestedSelector,
    /// GRF-0034: a negative or non-numeric COST.
    Cost,
}

/// Each condition's name, in the order of `Grf`.
const GRF_NAMES: [(Grf, &str); 7] = [
    (Grf::ConditionalJoin, "GRF-0021"),
    (Grf::GroupAsSingleton, "GRF-0022"),
    (Grf::SameOperand, "GRF-0023"),
    (Grf::Unbounded, "GRF-0031"),
    (Grf::UnboundedAggregate, "GRF-0032"),
    (Grf::NestedSelector, "GRF-0033"),
    (Grf::Cost, "GRF-0034"),
];

impl Grf {
    /// Its name, as the graph standard writes it.
    pub(crate) fn name(self) -> &'static str {
        GRF_NAMES.iter().find(|(g, _)| *g == self).map_or("", |(_, n)| n)
    }

    /// The error of it at `at`: SQLCODE -103, its name after `msg`.
    pub(crate) fn at(self, at: usize, msg: &str) -> SqlError {
        SqlError::at(Code::SyntaxRule, at, format!("[EXTENSION] {msg} ({})", self.name()))
    }
}

/// The error of a Syntax Rule of no GRF condition, at `at`.
fn rule(at: usize, msg: &str) -> SqlError {
    SqlError::at(Code::SyntaxRule, at, format!("[EXTENSION] {msg}"))
}

/// Checks the Syntax Rules of the GRAPH_TABLE `table` that need no
/// catalog.
///
/// # Errors
///
/// SQLCODE -103 of the first broken, its GRF condition named if it has
/// one.
pub(in crate::parser) fn check(table: &GraphTable) -> Result<()> {
    let pattern = &table.pattern;
    prefixes(pattern)?;
    terminating(pattern)?;
    let mut top = exposure(pattern)?;
    rows_variables(&table.rows, &mut top)?;
    references::check_references(table, &top)
}

/// [P1 §4.3, §4.6] The variables of `pattern`, seen from outside it, as
/// its postfilter and COLUMNS see them: those of each path pattern,
/// joined on their names, and the path variables.
///
/// # Errors
///
/// SQLCODE -103 if a name is declared as two kinds of variable, if two
/// path patterns join on a conditional singleton (GRF-0021) or a group
/// variable (GRF-0022).
pub fn exposure(pattern: &GraphPattern) -> Result<Exposure> {
    let mut top = Exposure::new();
    for path in &pattern.paths {
        let restricted = restricts(path.prefix.as_ref()) || restricts(pattern.keep.as_ref());
        let mut seen = expression_exposure(&path.expression)?;
        if restricted {
            bound(&mut seen);
        }
        if let Some(p) = &path.variable {
            seen = join(seen, declared(p, Declared::Path))?;
        }
        top = join(top, seen)?;
    }
    Ok(top)
}

/// The variables of `expression`, seen from outside it, its own
/// quantifier, if any, not applied.
///
/// # Errors
///
/// As `exposure`.
pub(super) fn expression_exposure(expression: &PathExpression) -> Result<Exposure> {
    let mut each = Vec::new();
    for term in expression.terms() {
        let mut seen = Exposure::new();
        for factor in &term.factors {
            seen = join(seen, factor_exposure(factor)?)?;
        }
        each.push(seen);
    }
    match expression {
        PathExpression::Term(_) => Ok(each.pop().unwrap_or_default()),
        PathExpression::Union(_) | PathExpression::Alternation(_) => alternatives(each),
    }
}

/// The variables of `factor`, seen from outside it: its quantifier
/// applied.
fn factor_exposure(factor: &PathFactor) -> Result<Exposure> {
    let mut seen = match &factor.primary {
        PathPrimary::Node(e) => e.variable.as_ref().map_or_else(Exposure::new, |v| declared(v, Declared::Vertex)),
        PathPrimary::Edge(e) => e.filler.variable.as_ref().map_or_else(Exposure::new, |v| declared(v, Declared::Edge)),
        PathPrimary::Parenthesized(p) => {
            let mut inner = expression_exposure(&p.expression)?;
            if let Some(v) = &p.variable {
                inner = join(inner, declared(v, Declared::Path))?;
            }
            if restricts(p.prefix.as_ref()) {
                bound(&mut inner);
            }
            inner
        }
    };
    if let Some(q) = &factor.quantifier {
        quantified(&mut seen, q);
    }
    Ok(seen)
}

/// The one variable `v`, declared as `kind`, a singleton.
fn declared(v: &GraphVariable, kind: Declared) -> Exposure {
    Exposure::from([(v.name.clone(), Exposed { kind, degree: Degree::Singleton, at: v.at })])
}

/// [P1 §4.2, §4.3] The variables of two parts of a pattern joined on
/// their names: of a concatenation, or of a graph pattern's path
/// patterns. A name shared is an implicit join, of singletons only.
fn join(mut left: Exposure, right: Exposure) -> Result<Exposure> {
    for (name, r) in right {
        let Some(l) = left.get(&name) else {
            left.insert(name, r);
            continue;
        };
        same_kind(&name, *l, r)?;
        let degrees = (l.degree, r.degree);
        if matches!(degrees, (Degree::Conditional, _) | (_, Degree::Conditional)) {
            return Err(
                Grf::ConditionalJoin.at(r.at, &format!("[P1 §4.6] {name} is a conditional singleton, which no implicit join may be on"))
            );
        }
        if matches!(degrees, (Degree::Group { .. }, _) | (_, Degree::Group { .. })) {
            return Err(Grf::GroupAsSingleton.at(r.at, &format!("[P1 §4.4] {name} is a group variable, which no implicit join may be on")));
        }
    }
    Ok(left)
}

/// [P1 §4.5, §4.6] The variables of the alternatives of a union or
/// alternation: a singleton of every one stays one; a singleton of some
/// is conditional; a group variable of any stays one.
fn alternatives(each: Vec<Exposure>) -> Result<Exposure> {
    let count = each.len();
    let mut merged: BTreeMap<String, (Exposed, usize)> = BTreeMap::new();
    for seen in each {
        for (name, e) in seen {
            let Some((m, n)) = merged.get_mut(&name) else {
                merged.insert(name, (e, 1));
                continue;
            };
            same_kind(&name, *m, e)?;
            *n = n.saturating_add(1);
            m.degree = match (m.degree, e.degree) {
                (Degree::Group { unbounded: a }, Degree::Group { unbounded: b }) => Degree::Group { unbounded: a || b },
                (Degree::Group { unbounded }, _) | (_, Degree::Group { unbounded }) => Degree::Group { unbounded },
                (Degree::Conditional, _) | (_, Degree::Conditional) => Degree::Conditional,
                (Degree::Singleton, Degree::Singleton) => Degree::Singleton,
            };
        }
    }
    Ok(merged
        .into_iter()
        .map(|(name, (mut e, n))| {
            if n < count && e.degree == Degree::Singleton {
                e.degree = Degree::Conditional;
            }
            (name, e)
        })
        .collect())
}

/// Refuses `name` declared as two kinds of variable, `a` and `b`.
fn same_kind(name: &str, a: Exposed, b: Exposed) -> Result<()> {
    if a.kind == b.kind {
        return Ok(());
    }
    let kind = |k: Declared| match k {
        Declared::Vertex => "a vertex variable",
        Declared::Edge => "an edge variable",
        Declared::Path => "a path variable",
    };
    Err(rule(b.at, &format!("[P1 §4.2] {name} is declared as {} and as {}", kind(a.kind), kind(b.kind))))
}

/// [P1 §4.4, §4.6] The variables `seen` under the quantifier `q`: `?`
/// makes singletons conditional; any other makes every variable a group
/// variable, unbounded if `q` is.
fn quantified(seen: &mut Exposure, q: &Quantifier) {
    for e in seen.values_mut() {
        e.degree = match (q.optional(), e.degree) {
            (true, Degree::Singleton | Degree::Conditional) => Degree::Conditional,
            (true, group @ Degree::Group { .. }) => group,
            (false, Degree::Group { unbounded }) => Degree::Group { unbounded: unbounded || q.unbounded() },
            (false, Degree::Singleton | Degree::Conditional) => Degree::Group { unbounded: q.unbounded() },
        };
    }
}

/// [P1 §5.3] The variables `seen` inside a restrictor other than WALK:
/// none of its group variables is unbounded.
fn bound(seen: &mut Exposure) {
    for e in seen.values_mut() {
        if let Degree::Group { unbounded } = &mut e.degree {
            *unbounded = false;
        }
    }
}

/// Whether `prefix` has a restrictor other than WALK.
fn restricts(prefix: Option<&PathPrefix>) -> bool {
    prefix.and_then(|p| p.restrictor).is_some_and(crate::ast::Restrictor::restricts)
}

/// Whether `prefix` has a selector.
fn selects(prefix: Option<&PathPrefix>) -> bool {
    prefix.is_some_and(|p| p.selector.is_some())
}

/// [P1 §5.1] Where selectors and restrictors are written: a selector at
/// the head of a path pattern alone (GRF-0033); KEEP's selector or
/// restrictor where no path pattern has its own; and COST no negative or
/// non-numeric literal (GRF-0034).
fn prefixes(pattern: &GraphPattern) -> Result<()> {
    if let Some(keep) = &pattern.keep {
        let path_selects = pattern.paths.iter().any(|p| selects(p.prefix.as_ref()));
        let path_restricts = pattern.paths.iter().any(|p| p.prefix.as_ref().is_some_and(|x| x.restrictor.is_some()));
        if (keep.selector.is_some() && path_selects) || (keep.restrictor.is_some() && path_restricts) {
            return Err(rule(keep.at, "[P1 §7.2] KEEP gives a selector or restrictor to path patterns that have none of their own"));
        }
    }
    let mut todo: Vec<&PathExpression> = Vec::new();
    for prefix in pattern.paths.iter().filter_map(|p| p.prefix.as_ref()).chain(&pattern.keep) {
        cost(prefix)?;
    }
    todo.extend(pattern.paths.iter().map(|p| &p.expression));
    while let Some(expression) = todo.pop() {
        for factor in expression.terms().iter().flat_map(|t| &t.factors) {
            let PathPrimary::Parenthesized(p) = &factor.primary else { continue };
            if let Some(prefix) = p.prefix.as_ref().filter(|x| x.selector.is_some()) {
                let msg = "[P1 §5.1] a selector is written at the head of a path pattern, not inside a parenthesized path pattern";
                return Err(Grf::NestedSelector.at(prefix.at, msg));
            }
            todo.push(&p.expression);
        }
    }
    Ok(())
}

/// [Graph standard §7.2] GRF-0034 of a CHEAPEST selector's COST that is
/// a literal, negative or not a number; others the binder checks by
/// their type.
fn cost(prefix: &PathPrefix) -> Result<()> {
    let Some(Selector::Cheapest { cost: Some(cost), .. }) = &prefix.selector else { return Ok(()) };
    let refused = match literal(cost) {
        None => false,
        Some((sign, Literal::Exact(d))) => (sign == Sign::Minus && d.units > 0) || (sign == Sign::Plus && d.units < 0),
        Some((sign, Literal::Approximate(f))) => (if sign == Sign::Minus { -f } else { *f }) < 0.0_f64,
        Some((_, Literal::Character(_) | Literal::Binary(_) | Literal::Datetime(_) | Literal::Interval(_) | Literal::Boolean(_))) => true,
    };
    if refused {
        return Err(Grf::Cost.at(prefix.at, "a COST is a number not less than 0 for each edge"));
    }
    Ok(())
}

/// The literal `v` is, perhaps with a sign, if it is one.
fn literal(v: &ValueExpression) -> Option<(Sign, &Literal)> {
    if let ValueExpression::Value(ValueSpecification::Literal(l)) = v {
        return Some((Sign::Plus, l));
    }
    if let ValueExpression::Monadic(sign, inner) = v
        && let ValueExpression::Value(ValueSpecification::Literal(l)) = &**inner
    {
        return Some((*sign, l));
    }
    None
}

/// [P1 §5] GRF-0031 of an unbounded quantifier in the scope of no
/// selector and no restrictor other than WALK: of its path pattern, of
/// KEEP, or of a parenthesized path pattern around it.
fn terminating(pattern: &GraphPattern) -> Result<()> {
    let kept = selects(pattern.keep.as_ref()) || restricts(pattern.keep.as_ref());
    let mut todo: Vec<(&PathExpression, bool)> = Vec::new();
    for path in &pattern.paths {
        let covered = kept || selects(path.prefix.as_ref()) || restricts(path.prefix.as_ref());
        todo.push((&path.expression, covered));
    }
    while let Some((expression, covered)) = todo.pop() {
        for factor in expression.terms().iter().flat_map(|t| &t.factors) {
            if let Some(q) = factor.quantifier.filter(|q| q.unbounded() && !covered) {
                let msg = "[P1 §5] an unbounded quantifier is in the scope of a selector or of a restrictor other than WALK";
                return Err(Grf::Unbounded.at(q.at, msg));
            }
            if let PathPrimary::Parenthesized(p) = &factor.primary {
                todo.push((&p.expression, covered || restricts(p.prefix.as_ref())));
            }
        }
    }
    Ok(())
}

/// The variables ONE ROW PER VERTEX or STEP declares, added to `top`:
/// each a name the pattern does not declare, and no two alike.
fn rows_variables(rows: &GraphRows, top: &mut Exposure) -> Result<()> {
    let kinds = match rows {
        GraphRows::Match => Vec::new(),
        GraphRows::Vertex(v) => vec![(v, Declared::Vertex)],
        GraphRows::Step(a, e, b) => vec![(a, Declared::Vertex), (e, Declared::Edge), (b, Declared::Vertex)],
    };
    for (v, kind) in kinds {
        if top.contains_key(&v.name) {
            return Err(rule(v.at, &format!("{} of ONE ROW PER is a variable the pattern does not declare", v.name)));
        }
        top.insert(v.name.clone(), Exposed { kind, degree: Degree::Singleton, at: v.at });
    }
    Ok(())
}
