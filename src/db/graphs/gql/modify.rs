//! \[EXTENSION\] A GQL procedure that modifies data (graph standard
//! §12.3, RULES.md D110), run clause by clause [P2 §5.1].
//!
//! Its reading clauses are lowered as a query's are. At each
//! data-modifying statement the binding table so far is read whole, with
//! the values the statement writes as columns of it; each row's writes
//! are made (`gql/writes.rs`, `gql/changes.rs`); and the table goes on
//! from its rows, as parameters of a table value constructor, an
//! INSERT's new elements' columns added and a SET's properties as set.
//! The whole procedure runs under a savepoint: all or none.

use super::graphs::SAVEPOINT;
use super::writes::Typed;
use crate::ast::{GqlCall, GqlClause, GqlLinear, GqlProcedure, GqlResult, GqlSetItem, PathPrimary, TableName};
use crate::binder::gql::{Kind, Lowered, Lowering, Table, Var, quote};
use crate::db::{Completion, Direct, Session};
use crate::error::{Code, Result, SqlError};
use crate::lexer::lex;
use crate::params::Params;
use crate::value::{DataType, Value};

/// A binding table read whole: its columns' names, SQL types and value
/// types, its rows, and its variables.
pub(super) struct Binding {
    /// Its columns' names, "$UNIT" first.
    pub(super) names: Vec<String>,
    /// Their types as written.
    pub(super) texts: Vec<String>,
    /// Their types.
    pub(super) types: Vec<DataType>,
    /// Its rows.
    pub(super) rows: Vec<Vec<Value>>,
    /// Its variables.
    pub(super) vars: Vec<Var>,
}

impl Binding {
    /// The place of column `name`.
    pub(super) fn at(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }

    /// The identity text the column `name` of `row` holds, if not null.
    pub(super) fn id<'r>(&self, row: &'r [Value], name: &str) -> Option<&'r str> {
        if let Some(Value::Char(s)) = self.at(name).and_then(|i| row.get(i)) {
            return Some(s.as_ref());
        }
        None
    }

    /// Whether variable `name` is an edge's.
    pub(super) fn is_edge(&self, name: &str) -> bool {
        self.vars.iter().any(|v| v.name == name && matches!(v.kind, Kind::Element { edge: true, .. }))
    }
}

/// The lowering a procedure is read by, and the parameters its tables
/// read.
type Run<'a, 'l> = (&'a Lowering<'l>, &'a mut Params);

/// Where a procedure begins: its first statement's table, if not I∅;
/// the graph its statements read unless they USE another; and the
/// variables each returns too, an inline CALL's imported ones.
struct Scoped<'a> {
    /// The first statement's table.
    start: Option<Table>,
    /// The graph.
    graph: Option<&'a TableName>,
    /// The variables returned too.
    carry: &'a [Var],
}

impl Session {
    /// \[EXTENSION\] Runs the data-modifying procedure `p`, read from
    /// `source`, of the graph `graph` unless it USEs another: the rows of
    /// its RETURN, if any.
    ///
    /// # Errors
    ///
    /// As its clauses', each undoing all it wrote.
    pub(super) fn gql_modify(&self, graph: Option<&TableName>, source: &str, p: &GqlProcedure) -> Result<Direct> {
        let st = self.gql_state()?;
        let graphs = |name: Option<&TableName>| self.gql_graph(&st, name.or(graph));
        let chars: Vec<char> = source.chars().collect();
        let toks = lex(source)?;
        let l = Lowering::new(&chars, &toks, &graphs);
        let mut params = Params::new();
        self.run(&format!("SAVEPOINT {SAVEPOINT}"))?;
        let ran = self.modify_procedure((&l, &mut params), p, Scoped { start: None, graph: None, carry: &[] });
        let lowered = match ran {
            Ok(lowered) => lowered,
            Err(e) => {
                self.run(&format!("ROLLBACK TO SAVEPOINT {SAVEPOINT}"))?;
                self.run(&format!("RELEASE SAVEPOINT {SAVEPOINT}"))?;
                return Err(e);
            }
        };
        self.run(&format!("RELEASE SAVEPOINT {SAVEPOINT}"))?;
        if lowered.vars.is_empty() {
            return Ok(Direct::Done(Completion::Success));
        }
        let commands = (crate::db::area::commands::SELECT_ROWS, crate::db::area::commands::UNRECOGNIZED);
        self.open_with(commands, |call| self.open_in(call, &lowered.sql, &params)).map(Direct::Rows)
    }

    /// The statements of `p`, each from the table the one before gave,
    /// the first from `scoped`'s start, or I∅.
    fn modify_procedure(&self, (l, params): Run<'_, '_>, p: &GqlProcedure, scoped: Scoped<'_>) -> Result<Lowered> {
        let mut last: Option<Lowered> = None;
        let mut start = scoped.start;
        for c in &p.statements {
            if !c.rest.is_empty() {
                return Err(SqlError::new(
                    Code::SyntaxRule,
                    "[EXTENSION] a procedure that modifies data combines no statements by a set operator",
                ));
            }
            let begin = match (&last, start.take()) {
                (Some(prior), _) => Table::of(l, prior),
                (None, Some(t)) => t,
                (None, None) => Table::unit(),
            };
            let graph = c.first.graph.as_ref().or(scoped.graph);
            let table = self.modify_linear((l, &mut *params), &c.first, begin, graph)?;
            last = Some(l.result(&table, &c.first.result, scoped.carry)?);
        }
        last.ok_or_else(|| SqlError::new(Code::Syntax, "[EXTENSION] expected a GQL statement"))
    }

    /// Linear statement `s` from `start`.
    fn modify_linear(&self, (l, params): Run<'_, '_>, s: &GqlLinear, start: Table, graph: Option<&TableName>) -> Result<Table> {
        let mut table = start;
        for clause in &s.clauses {
            table = match clause {
                GqlClause::Insert(_) | GqlClause::Set(_) | GqlClause::Remove(_) | GqlClause::Delete { .. } => {
                    self.modify_clause((l, &mut *params), &table, clause, graph)?
                }
                GqlClause::Call(GqlCall::Inline { optional, scope, body }) => {
                    if super::modifies(body) {
                        self.modifying_call((l, &mut *params), table, (*optional, scope.as_deref(), body), graph)?
                    } else {
                        l.clause(table, clause, graph)?
                    }
                }
                GqlClause::Match { .. }
                | GqlClause::Filter(_)
                | GqlClause::Let(_)
                | GqlClause::For { .. }
                | GqlClause::Page(_)
                | GqlClause::Call(GqlCall::Named { .. }) => l.clause(table, clause, graph)?,
            };
        }
        Ok(table)
    }

    /// An inline CALL whose procedure modifies data: run for each row of
    /// `table`, duplicates each their own, seeing the variables its scope
    /// imports; each row joined with the rows its run returns, or kept as
    /// it is if the procedure returns none [P2 §5.1].
    fn modifying_call(
        &self,
        (l, params): Run<'_, '_>,
        table: Table,
        (optional, scope, body): (bool, Option<&[String]>, &GqlProcedure),
        graph: Option<&TableName>,
    ) -> Result<Table> {
        let mut imported = Vec::new();
        for n in scope.unwrap_or_default() {
            imported.push(
                table
                    .var(n)
                    .cloned()
                    .ok_or_else(|| SqlError::new(Code::SyntaxRule, format!("[EXTENSION] variable {n} is not in scope")))?,
            );
        }
        let numbered = self.numbered((l, &mut *params), &table)?;
        imported.push(Var::value(ROW));
        let start = Some(l.imported(&numbered, &imported));
        let inner = self.modify_procedure((l, params), body, Scoped { start, graph, carry: &imported })?;
        let unit = matches!(body.statements.last().map(|c| &c.first.result), Some(GqlResult::None | GqlResult::Finish));
        if unit {
            return Ok(table);
        }
        Ok(without(l, l.joined(numbered, &inner, optional), ROW))
    }

    /// `table` read whole, each row numbered in "$ROW", as a table of
    /// parameters.
    fn numbered(&self, (l, params): Run<'_, '_>, table: &Table) -> Result<Table> {
        let a = l.alias();
        let sql = format!("SELECT {a}.*, ROW_NUMBER() OVER () AS {} FROM ({}) AS {a}", quote(ROW), table.sql);
        let (types, rows) = self.read_all(&sql, params)?;
        let mut vars = table.vars.clone();
        vars.push(Var::value(ROW));
        let mut b = binding(&vars, &types);
        b.rows = rows;
        Ok(materialized(l, &b, params))
    }

    /// `table` after the data-modifying statement `clause`.
    fn modify_clause(&self, (l, params): Run<'_, '_>, table: &Table, clause: &GqlClause, name: Option<&TableName>) -> Result<Table> {
        let graph = (l.graphs)(name)?;
        let ranges = needed(l, clause)?;
        let a = l.alias();
        let mut cols = vec![format!("{a}.*")];
        for (i, r) in ranges.iter().enumerate() {
            cols.push(format!("({}) AS {}", l.range_value(table, &a, *r)?, quote(&format!("$V{i}"))));
        }
        let (types, rows) = self.read_all(&format!("SELECT {} FROM ({}) AS {a}", cols.join(", "), table.sql), params)?;
        let width = types.len().saturating_sub(ranges.len());
        let mut b = binding(&table.vars, types.get(..width).unwrap_or_default());
        let mut inserts = Vec::new();
        if let GqlClause::Insert(paths) = clause {
            inserts = self.insert_plan(&graph, paths, &mut b, l)?;
        }
        for row in rows {
            let (base, values) = row.split_at(width.min(row.len()));
            let values: Vec<Typed> = values.iter().cloned().zip(types.iter().skip(width).cloned()).collect();
            let mut base = base.to_vec();
            match clause {
                GqlClause::Insert(_) => self.insert_row_elements(&graph, &inserts, (&mut b, &mut base), &values)?,
                GqlClause::Set(items) => self.set_items(&graph, items, (&b, &mut base), &values)?,
                GqlClause::Remove(items) => self.remove_items(&graph, items, (&b, &mut base))?,
                GqlClause::Delete { detach, items } => self.delete_items(&graph, (*detach, items), &b, &base)?,
                GqlClause::Match { .. }
                | GqlClause::Filter(_)
                | GqlClause::Let(_)
                | GqlClause::For { .. }
                | GqlClause::Page(_)
                | GqlClause::Call(_) => {}
            }
            b.rows.push(base);
        }
        Ok(materialized(l, &b, params))
    }
}

/// The column a modifying CALL numbers its incoming rows in.
const ROW: &str = "$ROW";

/// A binding table of `vars`, its columns of types `types`, and no row
/// yet.
fn binding(vars: &[Var], types: &[DataType]) -> Binding {
    let mut names = vec!["$UNIT".to_owned()];
    let mut texts = vec!["INTEGER".to_owned()];
    for (c, t) in vars.iter().flat_map(Var::columns) {
        names.push(c);
        texts.push(t.unwrap_or_default());
    }
    let texts = texts.into_iter().zip(types).map(|(t, d)| if t.is_empty() { d.to_string() } else { t }).collect();
    Binding { names, texts, types: types.to_vec(), rows: Vec::new(), vars: vars.to_vec() }
}

/// `table` without its variable `name`.
fn without(l: &Lowering<'_>, table: Table, name: &str) -> Table {
    let a = l.alias();
    let vars: Vec<Var> = table.vars.into_iter().filter(|v| v.name != name).collect();
    let mut cols = vec![format!("{a}.{}", quote("$UNIT"))];
    cols.extend(vars.iter().flat_map(Var::columns).map(|(c, _)| format!("{a}.{}", quote(&c))));
    Table { sql: format!("SELECT {} FROM ({}) AS {a}", cols.join(", "), table.sql), vars }
}

/// The tokens of each value `clause` writes, in order.
fn needed(l: &Lowering<'_>, clause: &GqlClause) -> Result<Vec<(usize, usize)>> {
    let mut out = Vec::new();
    match clause {
        GqlClause::Insert(paths) => {
            for p in paths.iter().flat_map(|p| p.expression.terms()).flat_map(|t| &t.factors) {
                let element = match &p.primary {
                    PathPrimary::Node(n) => n,
                    PathPrimary::Edge(e) => &e.filler,
                    PathPrimary::Parenthesized(_) => continue,
                };
                for spec in &element.properties {
                    let name =
                        l.text.token_at(spec.at).ok_or_else(|| SqlError::at(Code::Syntax, spec.at, "[EXTENSION] a property's value"))?;
                    let start = name.saturating_add(2);
                    out.push((start, l.value_end(start)));
                }
            }
        }
        GqlClause::Set(items) => {
            for item in items {
                match item {
                    GqlSetItem::Property { value, .. } => out.push((value.span.start, value.span.end)),
                    GqlSetItem::All { properties, .. } => out.extend(properties.iter().map(|(_, v)| (v.span.start, v.span.end))),
                    GqlSetItem::Label { .. } => {}
                }
            }
        }
        GqlClause::Remove(_)
        | GqlClause::Delete { .. }
        | GqlClause::Match { .. }
        | GqlClause::Filter(_)
        | GqlClause::Let(_)
        | GqlClause::For { .. }
        | GqlClause::Page(_)
        | GqlClause::Call(_) => {}
    }
    Ok(out)
}

/// The binding table `b` as a table value constructor, each value a
/// parameter of `params`.
fn materialized(l: &Lowering<'_>, b: &Binding, params: &mut Params) -> Table {
    let names: Vec<String> = b.names.iter().map(|n| quote(n)).collect();
    if b.rows.is_empty() {
        let cols: Vec<String> = b.texts.iter().zip(&names).map(|(t, n)| format!("CAST(NULL AS {t}) AS {n}")).collect();
        return Table { sql: format!("SELECT {} FROM (VALUES (0)) AS \"$z\"(\"$Z\") WHERE 1 = 0", cols.join(", ")), vars: b.vars.clone() };
    }
    let mut rows = Vec::new();
    for row in &b.rows {
        let mut values = Vec::new();
        for (k, v) in row.iter().enumerate() {
            let text = b.texts.get(k).map_or("VARCHAR(1000)", String::as_str);
            if v.is_null() {
                values.push(format!("CAST(NULL AS {text})"));
                continue;
            }
            let name = l.fresh("GQL_M");
            let t = b.types.get(k).cloned().unwrap_or(DataType::varying(1000));
            *params = std::mem::take(params).declare(&name, t, v.clone());
            values.push(format!("CAST({name} AS {text})"));
        }
        rows.push(format!("({})", values.join(", ")));
    }
    let alias = l.alias();
    Table { sql: format!("SELECT * FROM (VALUES {}) AS {alias}({})", rows.join(", "), names.join(", ")), vars: b.vars.clone() }
}
