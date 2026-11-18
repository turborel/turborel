//! \[EXTENSION\] The clauses of a GQL linear statement lowered to SQL
//! (RULES.md D-GQL), each a map from binding tables to binding tables
//! [P2 §5.1]: MATCH and OPTIONAL MATCH join the table with the pattern's
//! matches on the variables they share, its WHERE their join's
//! condition; FILTER restricts; LET adds a column; FOR an element of a
//! list per row; ORDER BY with OFFSET and LIMIT pages; CALL joins with a
//! procedure's table.
//!
//! A binding table's first column is "$UNIT", so I∅, the one row of no
//! variable, has one.

use super::rewrite::{Scope, column, rewrite};
use super::{Graph, Kind, Lowered, Lowering, Var, misused, quote};
use crate::ast::{GqlCall, GqlClause, GqlCondition, GqlExpression, GqlPage, GqlSpan, GraphPattern, TableName};
use crate::error::{Code, Result, SqlError};

/// A binding table: its SQL and its variables.
#[derive(Debug, Clone)]
pub(crate) struct Table {
    /// A `<query expression>`: "$UNIT", then each variable's columns.
    pub(crate) sql: String,
    /// Its variables, in the order of their columns.
    pub(crate) vars: Vec<Var>,
}

impl Table {
    /// I∅: one row, no variable.
    pub(crate) fn unit() -> Table {
        Table { sql: "SELECT 0 AS \"$UNIT\" FROM (VALUES (0)) AS \"$u\"(\"$Z\")".to_owned(), vars: Vec::new() }
    }

    /// The table of a lowered query's rows.
    pub(crate) fn of(l: &Lowering<'_>, q: &Lowered) -> Table {
        let alias = l.alias();
        let sql = format!("SELECT 0 AS \"$UNIT\", {alias}.* FROM ({}) AS {alias}", q.sql);
        Table { sql, vars: q.vars.clone() }
    }

    /// Its variable `name`, if any.
    pub(crate) fn var(&self, name: &str) -> Option<&Var> {
        self.vars.iter().find(|v| v.name == name)
    }

    /// Its variables seen through the correlation name `q`.
    pub(crate) fn scope(&self, q: &str) -> Scope<'_> {
        self.vars.iter().map(|v| (v.name.clone(), (v, q.to_owned()))).collect()
    }
}

impl Lowering<'_> {
    /// `table` after the reading clause `clause`, in the graph `graph`
    /// finds.
    ///
    /// # Errors
    ///
    /// As `bind_match` and `rewrite`; SQLCODE -103 of a variable bound
    /// twice; -900 of a data-modifying statement, which only EXECUTE GQL
    /// and a GQL session run.
    pub(crate) fn clause(&self, table: Table, clause: &GqlClause, graph: Option<&TableName>) -> Result<Table> {
        match clause {
            GqlClause::Match { optional, pattern, span } => {
                let g = (self.graphs)(graph)?;
                self.match_clause(table, &g, (pattern, *span), *optional)
            }
            GqlClause::Filter(c) => self.filter(table, c),
            GqlClause::Let(items) => items.iter().try_fold(table, |t, (name, e)| self.let_item(t, name, e)),
            GqlClause::For { variable, list, place } => self.for_clause(&table, variable, list, place.as_ref()),
            GqlClause::Page(page) => self.page(table, page),
            GqlClause::Call(call) => self.call(table, call, graph),
            GqlClause::Insert(_) | GqlClause::Set(_) | GqlClause::Remove(_) | GqlClause::Delete { .. } => Err(SqlError::new(
                Code::Unsupported,
                "[EXTENSION] a GQL data-modifying statement runs through EXECUTE GQL or a GQL session, not in a query of GQL_TABLE",
            )),
        }
    }

    /// MATCH or, if `optional`, OPTIONAL MATCH: `table` joined with the
    /// pattern's GRAPH_TABLE, read laterally, on the elements both bind; a
    /// row of no match kept, null extended, if `optional`.
    fn match_clause(&self, table: Table, graph: &Graph, (pattern, span): (&GraphPattern, GqlSpan), optional: bool) -> Result<Table> {
        let (a, b) = (self.alias(), self.alias());
        let m = self.bind_match(graph, (pattern, span), &table.scope(&a))?;
        let mut on = vec!["1 = 1".to_owned()];
        let mut fresh = Vec::new();
        for v in &m.vars {
            match table.var(&v.name) {
                Some(x) => {
                    if !matches!(x.kind, Kind::Element { .. }) || !matches!(v.kind, Kind::Element { .. }) {
                        return Err(misused(&v.name, "an element variable of one element"));
                    }
                    on.push(format!("{} = {}", column(&a, &v.name), column(&b, &v.name)));
                }
                None => fresh.push(v.clone()),
            }
        }
        let cols: Vec<String> = fresh.iter().flat_map(Var::columns).map(|(c, _)| column(&b, &c)).collect();
        let join = if optional { "LEFT JOIN" } else { "JOIN" };
        let extra = if cols.is_empty() { String::new() } else { format!(", {}", cols.join(", ")) };
        let sql = format!("SELECT {a}.*{extra} FROM ({}) AS {a} {join} {} AS {b} ON {}", table.sql, m.table, on.join(" AND "));
        let mut vars = table.vars;
        vars.extend(fresh);
        Ok(Table { sql, vars })
    }

    /// FILTER, or WHERE as a statement.
    fn filter(&self, table: Table, c: &GqlCondition) -> Result<Table> {
        let a = self.alias();
        let condition = rewrite(self.text, (c.span.start, c.span.end), &table.scope(&a))?;
        Ok(Table { sql: format!("SELECT * FROM ({}) AS {a} WHERE {condition}", table.sql), vars: table.vars })
    }

    /// The value of `e`, written over the table correlated as `a`.
    pub(crate) fn value(&self, table: &Table, a: &str, e: &GqlExpression) -> Result<String> {
        rewrite(self.text, (e.span.start, e.span.end), &table.scope(a))
    }

    /// The value of the tokens `range`, written over the table correlated
    /// as `a`.
    pub(crate) fn range_value(&self, table: &Table, a: &str, range: (usize, usize)) -> Result<String> {
        rewrite(self.text, range, &table.scope(a))
    }

    /// LET `name` = `e`.
    fn let_item(&self, table: Table, name: &str, e: &GqlExpression) -> Result<Table> {
        fresh_name(&table, name)?;
        let a = self.alias();
        let value = self.value(&table, &a, e)?;
        let sql = format!("SELECT {a}.*, ({value}) AS {} FROM ({}) AS {a}", quote(name), table.sql);
        let mut vars = table.vars;
        vars.push(Var::value(name));
        Ok(Table { sql, vars })
    }

    /// FOR `name` IN `list`, with the place of each element if `place`
    /// names a variable, counted from 0 if it is OFFSET's.
    fn for_clause(&self, table: &Table, name: &str, list: &GqlExpression, place: Option<&(String, bool)>) -> Result<Table> {
        fresh_name(table, name)?;
        let (a, u) = (self.alias(), self.alias());
        let value = self.value(table, &a, list)?;
        let mut vars = table.vars.clone();
        vars.push(Var::value(name));
        let Some((at, offset)) = place else {
            let sql = format!("SELECT {a}.*, {} FROM ({}) AS {a}, UNNEST({value}) AS {u}({})", column(&u, name), table.sql, quote(name));
            return Ok(Table { sql, vars });
        };
        fresh_name(table, at)?;
        let counted = if *offset { format!("{} - 1", column(&u, "$I")) } else { column(&u, "$I") };
        let sql = format!(
            "SELECT {a}.*, {}, {counted} AS {} FROM ({}) AS {a}, UNNEST({value}) WITH ORDINALITY AS {u}({}, \"$I\")",
            column(&u, name),
            quote(at),
            table.sql,
            quote(name)
        );
        vars.push(Var::value(at));
        Ok(Table { sql, vars })
    }

    /// ORDER BY, OFFSET and LIMIT as a statement.
    fn page(&self, table: Table, page: &GqlPage) -> Result<Table> {
        let a = self.alias();
        let tail = self.page_tail(&table, &a, page, &[])?;
        Ok(Table { sql: format!("SELECT * FROM ({}) AS {a}{tail}", table.sql), vars: table.vars })
    }

    /// The ORDER BY, OFFSET and FETCH of `page` over `table` correlated
    /// as `a`; a sort key that is one of `named` sorts by that column.
    pub(crate) fn page_tail(&self, table: &Table, a: &str, page: &GqlPage, named: &[String]) -> Result<String> {
        let mut tail = String::new();
        if !page.order.is_empty() {
            let mut keys = Vec::new();
            for s in &page.order {
                let one = s.value.span.end.saturating_sub(s.value.span.start) == 1;
                let name = super::rewrite::identifier_of(self.text.tok(s.value.span.start)).cloned();
                let key = match name.filter(|n| one && named.contains(n)) {
                    Some(n) => quote(&n),
                    None => self.value(table, a, &s.value)?,
                };
                let nulls = s.nulls_first.map_or("", |f| if f { " NULLS FIRST" } else { " NULLS LAST" });
                keys.push(format!("{key}{}{nulls}", if s.descending { " DESC" } else { " ASC" }));
            }
            tail = format!(" ORDER BY {}", keys.join(", "));
        }
        if let Some(o) = &page.offset {
            tail = format!("{tail} OFFSET {} ROWS", self.value(table, a, o)?);
        }
        if let Some(l) = &page.limit {
            tail = format!("{tail} FETCH FIRST {} ROWS ONLY", self.value(table, a, l)?);
        }
        Ok(tail)
    }

    /// CALL: an inline procedure's table joined with `table`, on the
    /// variables its scope clause imports or, without one, on those it
    /// returns that `table` has [P2 §5.1]; a named procedure's, a table
    /// function's rows of each.
    fn call(&self, table: Table, call: &GqlCall, graph: Option<&TableName>) -> Result<Table> {
        match call {
            GqlCall::Inline { optional, scope, body } => {
                let imported: Vec<Var> = match scope {
                    Some(names) => {
                        names.iter().map(|n| table.var(n).cloned().ok_or_else(|| misused(n, "in scope"))).collect::<Result<_>>()?
                    }
                    None => Vec::new(),
                };
                let start = if scope.is_some() { Some(self.imported(&table, &imported)) } else { None };
                let inner = self.procedure_in(body, start, graph, &imported)?;
                Ok(self.joined(table, &inner, *optional))
            }
            GqlCall::Named { optional, name, arguments, yields } => self.named_call(&table, (name, arguments, yields), *optional),
        }
    }

    /// The distinct rows of `table`'s variables `vars`: an inline
    /// procedure's start, its scope.
    pub(crate) fn imported(&self, table: &Table, vars: &[Var]) -> Table {
        let a = self.alias();
        let mut cols = vec![column(&a, "$UNIT")];
        cols.extend(vars.iter().flat_map(Var::columns).map(|(c, _)| column(&a, &c)));
        Table { sql: format!("SELECT DISTINCT {} FROM ({}) AS {a}", cols.join(", "), table.sql), vars: vars.to_vec() }
    }

    /// `table` joined with `inner`'s rows on the variables both have.
    pub(crate) fn joined(&self, table: Table, inner: &Lowered, optional: bool) -> Table {
        let (a, c) = (self.alias(), self.alias());
        let mut on = vec!["1 = 1".to_owned()];
        let mut fresh = Vec::new();
        for v in &inner.vars {
            match table.var(&v.name) {
                Some(_) => on.push(format!("{} = {}", column(&a, &v.name), column(&c, &v.name))),
                None => fresh.push(v.clone()),
            }
        }
        let cols: Vec<String> = fresh.iter().flat_map(Var::columns).map(|(n, _)| column(&c, &n)).collect();
        let extra = if cols.is_empty() { String::new() } else { format!(", {}", cols.join(", ")) };
        let join = if optional { "LEFT JOIN" } else { "JOIN" };
        let sql = format!("SELECT {a}.*{extra} FROM ({}) AS {a} {join} ({}) AS {c} ON {}", table.sql, inner.sql, on.join(" AND "));
        let mut vars = table.vars;
        vars.extend(fresh);
        Table { sql, vars }
    }

    /// CALL of the procedure `name`: the rows of the table function of
    /// that name, of the arguments as each row of `table` gives them.
    fn named_call(
        &self,
        table: &Table,
        (name, arguments, yields): (&TableName, &[GqlExpression], &[(String, String)]),
        optional: bool,
    ) -> Result<Table> {
        let (a, f) = (self.alias(), self.alias());
        let args: Vec<String> = arguments.iter().map(|e| self.value(table, &a, e)).collect::<Result<_>>()?;
        let routine = name.auth.as_ref().map_or_else(|| quote(&name.ident), |s| format!("{}.{}", quote(s), quote(&name.ident)));
        let mut vars = table.vars.clone();
        let mut cols = Vec::new();
        for (c, v) in yields {
            fresh_name(table, v)?;
            cols.push(format!("{} AS {}", column(&f, c), quote(v)));
            vars.push(Var::value(v));
        }
        let extra = if cols.is_empty() { String::new() } else { format!(", {}", cols.join(", ")) };
        let join = if optional { "LEFT JOIN" } else { "JOIN" };
        let sql = format!("SELECT {a}.*{extra} FROM ({}) AS {a} {join} TABLE({routine}({})) AS {f} ON 1 = 1", table.sql, args.join(", "));
        Ok(Table { sql, vars })
    }
}

/// That `table` has no variable `name`.
///
/// # Errors
///
/// SQLCODE -103 if it has.
pub(crate) fn fresh_name(table: &Table, name: &str) -> Result<()> {
    if table.var(name).is_some() {
        return Err(SqlError::new(Code::SyntaxRule, format!("[EXTENSION] variable {name} is already bound")));
    }
    Ok(())
}
