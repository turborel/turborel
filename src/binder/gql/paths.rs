//! \[EXTENSION\] The parts of a GQL path pattern that are no fixed
//! element (RULES.md D-GQL): a path expression's union or alternation of
//! terms, a parenthesized part with its WHERE, a quantified part as a
//! recursive query, and a selector's choice among a path's matches.
//!
//! A quantified part `{m,n}` iterates its body's table: each iteration
//! starts where the one before ended, its variables group variables
//! listing their elements. An iteration is pruned as the restrictor
//! says, of the elements the body fixes against those before: TRAIL of
//! its edges, ACYCLIC of its vertices, SIMPLE of its vertices but a
//! return to the first. Under a selector an unrestricted walk is pruned
//! as SIMPLE, as no shortest walk revisits a vertex but its first.

use super::rewrite::{Scope, column, rewrite};
use super::sources::all_vertices;
use super::terms::{FRAME, Restrict, Seg, Term, segments};
use super::{Graph, Kind, LABEL_BAR, LIST_TYPE, Lowering, Var, quote, string};
use crate::ast::{Cheapest, PathExpression, PathPrimary, Quantifier, Restrictor, Selector};
use crate::error::{Code, Result, SqlError};

impl Lowering<'_> {
    /// The table of a parenthesized or quantified part.
    ///
    /// # Errors
    ///
    /// As `term`.
    pub(super) fn part(&self, graph: &Graph, primary: &PathPrimary, quantifier: Option<Quantifier>, r: Restrict) -> Result<Term> {
        let body = match primary {
            PathPrimary::Node(n) => self.term(graph, &[Seg::Node(Some(n))], r)?,
            PathPrimary::Edge(e) => self.term(graph, &[Seg::Node(None), Seg::Edge(e), Seg::Node(None)], r)?,
            PathPrimary::Parenthesized(p) => {
                let restrictor = p.prefix.as_ref().and_then(|x| x.restrictor).or(r.restrictor);
                let inner = Restrict { restrictor, selected: r.selected };
                let body = self.expression(graph, &p.expression, inner)?;
                match p.where_clause.as_ref().and_then(|_| self.where_range(p.at)) {
                    Some(range) => self.filtered(body, range)?,
                    None => body,
                }
            }
        };
        match quantifier {
            None => Ok(body),
            Some(q) => Ok(self.repeat(graph, body, q, r)),
        }
    }

    /// `body` restricted by the condition of tokens `range`.
    fn filtered(&self, body: Term, range: (usize, usize)) -> Result<Term> {
        let alias = self.alias();
        let scope: Scope<'_> = body.vars.iter().map(|v| (v.name.clone(), (v, alias.clone()))).collect();
        let condition = rewrite(self.text, range, &scope)?;
        let sql = format!("SELECT * FROM ({}) AS {alias} WHERE {condition}", body.sql);
        Ok(Term { sql, ..body })
    }

    /// The table of path expression `e`: its one term's, or its terms'
    /// combined, each variable null where a term has none.
    ///
    /// # Errors
    ///
    /// As `term`.
    pub(super) fn expression(&self, graph: &Graph, e: &PathExpression, r: Restrict) -> Result<Term> {
        let terms: Vec<Term> = e.terms().iter().map(|t| self.term(graph, &segments(&t.factors), r)).collect::<Result<_>>()?;
        let operator = match e {
            PathExpression::Term(_) => return terms.into_iter().next().ok_or_else(|| SqlError::new(Code::SyntaxRule, "empty path")),
            PathExpression::Union(_) => " UNION ",
            PathExpression::Alternation(_) => " UNION ALL ",
        };
        let mut vars: Vec<Var> = Vec::new();
        for v in terms.iter().flat_map(|t| &t.vars) {
            if !vars.iter().any(|x| x.name == v.name) {
                vars.push(v.clone());
            }
        }
        let selects: Vec<String> = terms.iter().map(|t| self.padded(t, &vars)).collect();
        Ok(Term { sql: selects.join(operator), vars, edges: 0, nodes: 0 })
    }

    /// A select of `t`'s `FRAME` and of `vars`' columns, each null that
    /// `t` has not.
    fn padded(&self, t: &Term, vars: &[Var]) -> String {
        let alias = self.alias();
        let mut cols: Vec<String> = FRAME.iter().map(|c| column(&alias, c)).collect();
        for v in vars {
            let has = t.vars.iter().any(|x| x.name == v.name);
            for (c, ty) in v.columns() {
                cols.push(if has {
                    format!("{} AS {}", column(&alias, &c), quote(&c))
                } else {
                    format!("CAST(NULL AS {}) AS {}", ty.unwrap_or_default(), quote(&c))
                });
            }
        }
        format!("SELECT {} FROM ({}) AS {alias}", cols.join(", "), t.sql)
    }

    /// `body` repeated as `q` bounds it, restricted by `r`.
    fn repeat(&self, graph: &Graph, body: Term, q: Quantifier, r: Restrict) -> Term {
        if q.optional() {
            return self.optional(graph, body);
        }
        let walk = matches!(r.restrictor, None | Some(Restrictor::Walk));
        let restrictor = if walk && r.selected { Some(Restrictor::Simple) } else { r.restrictor };
        let groups: Vec<Var> = body.vars.iter().map(|v| Var { name: v.name.clone(), kind: Kind::Group }).collect();
        let name = self.alias();
        let (b, prior) = (self.alias(), self.alias());
        let mut heads: Vec<String> = FRAME.iter().map(|c| quote(c)).collect();
        heads.push(quote("$N"));
        heads.extend(groups.iter().flat_map(Var::columns).map(|(c, _)| quote(&c)));
        let anchor = anchor_columns(&body, &b);
        let step = step_columns(&body, &b, &prior);
        let mut prune = vec![format!("{} < {}", column(&prior, "$N"), q.upper.unwrap_or(u32::MAX))];
        prune.extend(pruned(&body, (&b, &prior), restrictor));
        let mut sql = format!(
            "SELECT {cols} FROM (WITH RECURSIVE {name}({heads}) AS (SELECT {anchor} FROM ({body}) AS {b} UNION ALL SELECT {step} FROM {name} AS {prior} JOIN ({body}) AS {b} ON {} = {} WHERE {}) SELECT * FROM {name}) AS {name} WHERE {} >= {}",
            column(&prior, "$END"),
            column(&b, "$START"),
            prune.join(" AND "),
            column(&name, "$N"),
            q.lower.max(1),
            cols = heads.iter().filter(|h| *h != &quote("$N")).map(|h| format!("{name}.{h}")).collect::<Vec<_>>().join(", "),
            heads = heads.join(", "),
            body = body.sql,
        );
        if q.lower == 0 {
            sql = format!("{sql} UNION ALL {}", self.zero(graph, &groups));
        }
        Term { sql, vars: groups, edges: 0, nodes: 0 }
    }

    /// `body` made optional, `?`: its rows, and each vertex's path itself
    /// with its variables null, conditional singletons [P1 §4.6].
    fn optional(&self, graph: &Graph, body: Term) -> Term {
        let once = self.padded(&body, &body.vars);
        let v = self.alias();
        let id = column(&v, "$ID");
        let listed = format!("CAST({id} || {} AS {LIST_TYPE})", string(LABEL_BAR));
        let values = [id.clone(), id, listed.clone(), listed, format!("CAST('' AS {LIST_TYPE})"), "0".to_owned()];
        let mut cols: Vec<String> = values.iter().zip(FRAME).map(|(value, name)| format!("{value} AS {}", quote(name))).collect();
        for (c, ty) in body.vars.iter().flat_map(Var::columns) {
            cols.push(format!("CAST(NULL AS {}) AS {}", ty.unwrap_or_default(), quote(&c)));
        }
        let none = format!("SELECT {} FROM ({}) AS {v}", cols.join(", "), all_vertices(graph));
        Term { sql: format!("{once} UNION ALL {none}"), vars: body.vars, edges: 0, nodes: 0 }
    }

    /// The rows of a quantified part's zero iterations: each vertex, its
    /// path itself; its group variables empty.
    fn zero(&self, graph: &Graph, groups: &[Var]) -> String {
        let v = self.alias();
        let id = column(&v, "$ID");
        let listed = format!("CAST({id} || {} AS {LIST_TYPE})", string(LABEL_BAR));
        let values = [id.clone(), id, listed.clone(), listed, format!("CAST('' AS {LIST_TYPE})"), "0".to_owned()];
        let mut cols: Vec<String> = values.iter().zip(FRAME).map(|(value, name)| format!("{value} AS {}", quote(name))).collect();
        for g in groups {
            cols.push(format!("CAST('' AS {LIST_TYPE}) AS {}", quote(&g.name)));
            cols.push(format!("0 AS {}", quote(&g.count_column())));
        }
        format!("SELECT {} FROM ({}) AS {v}", cols.join(", "), all_vertices(graph))
    }
}

/// The anchor's columns: the body's first iteration.
fn anchor_columns(body: &Term, b: &str) -> String {
    let mut cols: Vec<String> = FRAME.iter().map(|c| column(b, c)).collect();
    cols.push("1".to_owned());
    for v in &body.vars {
        let (list, count) = match v.kind {
            Kind::Group => (column(b, &v.name), column(b, &v.count_column())),
            Kind::Element { .. } | Kind::Path | Kind::Value => (format!("{} || {}", column(b, &v.name), string(LABEL_BAR)), "1".to_owned()),
        };
        cols.push(format!("CAST({list} AS {LIST_TYPE})"));
        cols.push(count);
    }
    cols.join(", ")
}

/// The recursive step's columns: the iterations before, `prior`, and
/// the body's next, `b`.
fn step_columns(body: &Term, b: &str, prior: &str) -> String {
    let rest = |c: &str| format!("SUBSTRING({} FROM CHAR_LENGTH({}) + 2)", column(b, c), column(b, "$START"));
    let list = |s: String| format!("CAST({s} AS {LIST_TYPE})");
    let mut cols = vec![
        column(prior, "$START"),
        column(b, "$END"),
        list(format!("{} || {}", column(prior, "$SEQ"), rest("$SEQ"))),
        list(format!("{} || {}", column(prior, "$NODES"), rest("$NODES"))),
        list(format!("{} || {}", column(prior, "$EDGES"), column(b, "$EDGES"))),
        format!("{} + {}", column(prior, "$LEN"), column(b, "$LEN")),
        format!("{} + 1", column(prior, "$N")),
    ];
    for v in &body.vars {
        let (list_next, count) = match v.kind {
            Kind::Group => (column(b, &v.name), column(b, &v.count_column())),
            Kind::Element { .. } | Kind::Path | Kind::Value => (format!("{} || {}", column(b, &v.name), string(LABEL_BAR)), "1".to_owned()),
        };
        cols.push(list(format!("{} || {list_next}", column(prior, &v.name))));
        cols.push(format!("{} + {count}", column(prior, &v.count_column())));
    }
    cols.join(", ")
}

/// The conditions on an iteration `b` after those before, `prior`, the
/// restrictor `r` puts on the elements the body fixes.
fn pruned(body: &Term, (b, prior): (&str, &str), r: Option<Restrictor>) -> Vec<String> {
    let absent = |id: String, list: &str| {
        let bar = string(LABEL_BAR);
        format!("POSITION({bar} || {id} || {bar} IN {bar} || {}) = 0", column(prior, list))
    };
    let vertices = (2..=body.nodes).map(|k| column(b, &format!("$V{k}")));
    match r {
        Some(Restrictor::Trail) => (1..=body.edges).map(|k| absent(column(b, &format!("$E{k}")), "$EDGES")).collect(),
        Some(Restrictor::Acyclic) => vertices.map(|v| absent(v, "$NODES")).collect(),
        Some(Restrictor::Simple) => {
            let mut out: Vec<String> =
                vertices.map(|v| format!("({} OR {v} = {})", absent(v.clone(), "$NODES"), column(prior, "$START"))).collect();
            out.push(format!("{} <> {}", column(prior, "$END"), column(prior, "$START")));
            out
        }
        Some(Restrictor::Walk) | None => Vec::new(),
    }
}

/// \[EXTENSION\] The selector `s` of a path pattern's table `t`: of the
/// matches of each pair of first and last vertices, those it keeps.
///
/// # Errors
///
/// SQLCODE -900 of CHEAPEST, whose costs GRAPH_TABLE's path search sums.
pub(super) fn select(lowering: &Lowering<'_>, term: Term, selector: &Selector) -> Result<Term> {
    let (rank, keep) = match selector {
        Selector::Any | Selector::AnyShortest => ("ROW_NUMBER", 1),
        Selector::AnyK(k) | Selector::ShortestK(k) => ("ROW_NUMBER", *k),
        Selector::AllShortest => ("RANK", 1),
        Selector::ShortestGroups(k) => ("DENSE_RANK", *k),
        Selector::Cheapest { which: Cheapest::Any | Cheapest::All | Cheapest::K(_), .. } => {
            return Err(SqlError::new(
                Code::Unsupported,
                "[EXTENSION] CHEAPEST is chosen by GRAPH_TABLE's path search, which sums the costs; a GQL query's lowering ranks by length (RULES.md D-GQL)",
            ));
        }
    };
    let (q, k) = (lowering.alias(), lowering.alias());
    let cols: Vec<String> = term.columns().iter().map(|c| column(&k, c)).collect();
    let sql = format!(
        "SELECT {} FROM (SELECT {q}.*, {rank}() OVER (PARTITION BY {}, {} ORDER BY {}) AS \"$RANK\" FROM ({}) AS {q}) AS {k} WHERE {k}.\"$RANK\" <= {keep}",
        cols.join(", "),
        column(&q, "$START"),
        column(&q, "$END"),
        column(&q, "$LEN"),
        term.sql
    );
    Ok(Term { sql, ..term })
}
