//! \[EXTENSION\] [GS §10.1] The conditions of a MATCH query's WHERE
//! clause that test one table of the MATCH alone, also tested as the
//! element pattern of its variable is bound [GS §6.4]: so a search
//! starts at the rows they keep, not at every element. A condition is
//! taken when it reads columns of that table that are properties of the
//! same name and value, and literals; it stays in the WHERE clause too,
//! so the rows are the same either way.

use std::collections::BTreeMap;

use crate::ast::{PathExpression, PathFactor, PathPattern, PathPrimary, PathTerm, SearchCondition, ValueExpression, ValueSpecification};
use crate::catalog::{GraphDesc, QualifiedName, identifier};

/// Whether `v` is a value of no subquery, parameter or variable, each
/// column it reads one of `alias` that `property` says is a property.
fn plain(v: &ValueExpression, alias: &str, property: &dyn Fn(&str) -> bool) -> bool {
    !v.any(&|x| {
        if let ValueExpression::Column(c) = x {
            return c.qualifier.as_ref().is_none_or(|q| q.ident != alias || q.auth.is_some()) || !property(&c.column);
        }
        let operator = matches!(x, ValueExpression::Dyadic(..) | ValueExpression::Monadic(..) | ValueExpression::Nested(_));
        !operator && !matches!(x, ValueExpression::Value(ValueSpecification::Literal(_)))
    })
}

/// The conditions of `conjuncts` each variable of `tables` takes, by
/// variable, in graph `g` whose label of each is `labels`'.
pub(super) fn prefilters(
    conjuncts: &[SearchCondition],
    tables: &BTreeMap<String, QualifiedName>,
    (g, labels): (&GraphDesc, &BTreeMap<String, String>),
) -> BTreeMap<String, SearchCondition> {
    let mut out: BTreeMap<String, SearchCondition> = BTreeMap::new();
    for (alias, table) in tables {
        let Some(label) = labels.get(alias) else { continue };
        let property = |column: &str| {
            let quoted = format!("\"{}\"", column.replace('"', "\"\""));
            g.of_table(table)
                .iter()
                .filter_map(|e| e.label(label))
                .flat_map(|l| &l.properties)
                .any(|p| p.name == column && (p.expression == identifier(column) || p.expression == quoted))
        };
        for c in conjuncts {
            let mut fits = true;
            c.for_each_subquery(&mut |_| fits = false);
            c.for_each_value(&mut |v| fits = fits && plain(v, alias, &property));
            if !fits || !reads_any(c) {
                continue;
            }
            let taken = match out.remove(alias) {
                Some(before) => SearchCondition::And(Box::new(before), Box::new(c.clone())),
                None => c.clone(),
            };
            out.insert(alias.clone(), taken);
        }
    }
    out
}

/// Whether `c` reads a column.
fn reads_any(c: &SearchCondition) -> bool {
    let mut found = false;
    c.for_each_value(&mut |v| found = found || v.any(&|x| matches!(x, ValueExpression::Column(_))));
    found
}

/// Gives the first element pattern of each variable `filters` names its
/// condition, with any written.
pub(super) fn place(paths: &mut [PathPattern], mut filters: BTreeMap<String, SearchCondition>) {
    for p in paths {
        place_terms(&mut p.expression, &mut filters);
    }
}

/// `place`, of each term of `e`.
fn place_terms(e: &mut PathExpression, filters: &mut BTreeMap<String, SearchCondition>) {
    let terms: Vec<&mut PathTerm> = match e {
        PathExpression::Term(t) => vec![t],
        PathExpression::Union(ts) | PathExpression::Alternation(ts) => ts.iter_mut().collect(),
    };
    for t in terms {
        place_factors(&mut t.factors, filters);
    }
}

/// `place`, of `factors`.
fn place_factors(factors: &mut [PathFactor], filters: &mut BTreeMap<String, SearchCondition>) {
    for f in factors {
        let element = match &mut f.primary {
            PathPrimary::Node(e) => e,
            PathPrimary::Edge(e) => &mut e.filler,
            PathPrimary::Parenthesized(p) => {
                place_terms(&mut p.expression, filters);
                continue;
            }
        };
        let Some(c) = element.variable.as_ref().and_then(|v| filters.remove(&v.name)) else { continue };
        element.where_clause = Some(match element.where_clause.take() {
            Some(w) => SearchCondition::And(Box::new(w), Box::new(c)),
            None => c,
        });
    }
}
