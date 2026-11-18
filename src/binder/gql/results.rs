//! \[EXTENSION\] A GQL procedure's result lowered to SQL (RULES.md
//! D110): RETURN projects the binding table, grouping by its items that
//! are no aggregate when one is; the set operators combine statements'
//! tables by column name, OTHERWISE the right's rows if the left has
//! none; NEXT feeds a statement's table to the next [P2 §5.1].

use super::linear::fresh_name;
use super::rewrite::{AGGREGATES, column, identifier_of, word};
use super::{Kind, Lowered, Lowering, Table, Var, quote};
use crate::ast::{GqlComposite, GqlLinear, GqlProcedure, GqlResult, GqlReturn, GqlReturnItem, GqlSetOperator, TableName};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

impl Lowering<'_> {
    /// \[EXTENSION\] The table of procedure `p`, its first statement's
    /// start `start` if given, else I∅.
    ///
    /// # Errors
    ///
    /// As each clause's.
    pub(crate) fn procedure(&self, p: &GqlProcedure, start: Option<Table>) -> Result<Lowered> {
        self.procedure_in(p, start, None, &[])
    }

    /// `procedure`, its statements reading the graph `graph` unless they
    /// USE another, each returning `carry` too: an inline CALL's imported
    /// variables, which it is joined on.
    pub(crate) fn procedure_in(&self, p: &GqlProcedure, start: Option<Table>, graph: Option<&TableName>, carry: &[Var]) -> Result<Lowered> {
        let mut table = start;
        let mut last = None;
        for statement in &p.statements {
            let begin = match (&last, table.take()) {
                (Some(prior), _) => Table::of(self, prior),
                (None, Some(t)) => t,
                (None, None) => Table::unit(),
            };
            last = Some(self.composite(statement, &begin, (graph, carry))?);
        }
        last.ok_or_else(|| SqlError::new(Code::Syntax, "[EXTENSION] expected a GQL statement"))
    }

    /// A composite statement: its linear statements combined.
    fn composite(&self, c: &GqlComposite, start: &Table, (graph, carry): (Option<&TableName>, &[Var])) -> Result<Lowered> {
        let mut left = self.linear(&c.first, start.clone(), (graph, carry))?;
        for (op, right) in &c.rest {
            let right = self.linear(right, start.clone(), (graph, carry))?;
            left = self.combined(left, *op, &right)?;
        }
        Ok(left)
    }

    /// `left` and `right` combined by `op`, by column name.
    fn combined(&self, left: Lowered, op: GqlSetOperator, right: &Lowered) -> Result<Lowered> {
        let names = left.columns();
        let mut theirs = right.columns();
        theirs.sort();
        let mut ours = names.clone();
        ours.sort();
        if ours != theirs {
            return Err(SqlError::new(
                Code::SyntaxRule,
                "[EXTENSION] the statements a set operator combines return columns of the same names",
            ));
        }
        let r = self.alias();
        let cols: Vec<String> = names.iter().map(|c| column(&r, c)).collect();
        let reordered = format!("SELECT {} FROM ({}) AS {r}", cols.join(", "), right.sql);
        let all = |b: bool| if b { " ALL" } else { "" };
        let sql = match op {
            GqlSetOperator::Union(b) => format!("{} UNION{} {reordered}", left.sql, all(b)),
            GqlSetOperator::Except(b) => format!("{} EXCEPT{} {reordered}", left.sql, all(b)),
            GqlSetOperator::Intersect(b) => format!("{} INTERSECT{} {reordered}", left.sql, all(b)),
            GqlSetOperator::Otherwise => {
                let l = self.alias();
                format!("{} UNION ALL {reordered} WHERE NOT EXISTS (SELECT 1 FROM ({}) AS {l})", left.sql, left.sql)
            }
        };
        let o = self.alias();
        Ok(Lowered { sql: format!("SELECT * FROM ({sql}) AS {o}"), vars: left.vars })
    }

    /// A linear statement from `start`.
    fn linear(&self, s: &GqlLinear, start: Table, (graph, carry): (Option<&TableName>, &[Var])) -> Result<Lowered> {
        let graph = s.graph.as_ref().or(graph);
        let table = s.clauses.iter().try_fold(start, |t, c| self.clause(t, c, graph))?;
        self.result(&table, &s.result, carry)
    }

    /// The result of `table`: RETURN's projection, carrying `carry`'s
    /// columns too; FINISH's and nothing's, no row.
    ///
    /// # Errors
    ///
    /// As `rewrite`; SQLCODE -103 if two items have one name.
    pub(crate) fn result(&self, table: &Table, result: &GqlResult, carry: &[Var]) -> Result<Lowered> {
        match result {
            GqlResult::Return(r) => self.return_clause(table, r, carry),
            GqlResult::Finish | GqlResult::None => {
                let a = self.alias();
                Ok(Lowered { sql: format!("SELECT 0 AS \"$UNIT\" FROM ({}) AS {a} WHERE 1 = 0", table.sql), vars: Vec::new() })
            }
        }
    }

    /// RETURN.
    fn return_clause(&self, table: &Table, r: &GqlReturn, carry: &[Var]) -> Result<Lowered> {
        let a = self.alias();
        let mut items: Vec<Item> = Vec::new();
        match &r.items {
            Some(list) => {
                for item in list {
                    items.push(self.item(table, &a, item)?);
                }
            }
            None => items.extend(table.vars.iter().map(|v| Item::Carried(v.clone(), v.clone()))),
        }
        let returned: Vec<String> = items.iter().map(|i| i.var().name.clone()).collect();
        let carried = carry.iter().filter(|v| !returned.contains(&v.name)).map(|v| Item::Carried(v.clone(), v.clone()));
        items.splice(0..0, carried.collect::<Vec<_>>());
        let mut probe = Table { sql: String::new(), vars: Vec::new() };
        for v in items.iter().map(Item::var) {
            fresh_name(&probe, &v.name)?;
            probe.vars.push(v.clone());
        }
        let grouped = items.iter().any(|i| matches!(i, Item::Value(_, _, true))) || !r.group_by.is_empty();
        let (select, keys) = select_list(&items, &a);
        let mut sql = format!("SELECT {}{} FROM ({}) AS {a}", if r.distinct { "DISTINCT " } else { "" }, select.join(", "), table.sql);
        if grouped && !keys.is_empty() {
            sql = format!("{sql} GROUP BY {}", keys.join(", "));
        }
        let named: Vec<String> = items.iter().flat_map(|i| i.var().columns()).map(|(c, _)| c).collect();
        sql.push_str(&self.page_tail(table, &a, &r.page, &named)?);
        Ok(Lowered { sql, vars: items.iter().map(|i| i.var().clone()).collect() })
    }

    /// A RETURN item.
    fn item(&self, table: &Table, a: &str, item: &GqlReturnItem) -> Result<Item> {
        let span = item.value.span;
        let one = span.end.saturating_sub(span.start) == 1;
        if let Some(v) = identifier_of(self.text.tok(span.start)).filter(|_| one).and_then(|n| table.var(n)) {
            let name = item.name.clone().unwrap_or_else(|| v.name.clone());
            return Ok(Item::Carried(v.clone(), v.renamed(&name)));
        }
        let value = self.value(table, a, &item.value)?;
        let group =
            |i: usize| identifier_of(self.text.tok(i)).and_then(|n| table.var(n)).is_some_and(|v| matches!(v.kind, Kind::Group { .. }));
        let quantified = |i: usize| matches!(self.text.tok(i), Tok::KeyWord(kw::DISTINCT | kw::ALL));
        let across = |i: usize| group(i) || (quantified(i) && group(i.saturating_add(1)));
        let aggregate = (span.start..span.end).any(|i| {
            word(self.text.tok(i)).is_some_and(|w| AGGREGATES.contains(&w.as_str()))
                && *self.text.tok(i.saturating_add(1)) == Tok::Delimiter(delim::LEFT_PAREN)
                && !across(i.saturating_add(2))
        });
        let name = item.name.clone().unwrap_or_else(|| self.written_name(span.start, span.end));
        Ok(Item::Value(value, Var::value(&name), aggregate))
    }

    /// The name of an item written without AS: its tokens as written,
    /// each identifier in its normal form.
    fn written_name(&self, start: usize, end: usize) -> String {
        (start..end)
            .map(|i| match self.text.tok(i) {
                Tok::Identifier(n) => n.clone(),
                Tok::KeyWord(k) => (*k).to_owned(),
                Tok::ExactNumeric(_)
                | Tok::ApproximateNumeric(_)
                | Tok::CharacterString(_)
                | Tok::BinaryString(_)
                | Tok::Introducer(_)
                | Tok::LargeObjectLength(_)
                | Tok::Delimiter(_)
                | Tok::End => self.text.written(i),
            })
            .collect()
    }
}

/// A RETURN item.
enum Item {
    /// A variable of the binding table, and it as returned, renamed.
    Carried(Var, Var),
    /// A value: its SQL, its variable, and whether it calls an aggregate.
    Value(String, Var, bool),
}

impl Item {
    /// Its variable as returned.
    fn var(&self) -> &Var {
        match self {
            Item::Carried(_, v) | Item::Value(_, v, _) => v,
        }
    }
}

/// The select list of `items` over the table correlated as `a`, and the
/// grouping keys: each item that calls no aggregate, a variable's every
/// column.
fn select_list(items: &[Item], a: &str) -> (Vec<String>, Vec<String>) {
    let (mut select, mut keys) = (Vec::new(), Vec::new());
    for item in items {
        match item {
            Item::Carried(from, to) => {
                for ((c, _), (n, _)) in from.columns().into_iter().zip(to.columns()) {
                    select.push(format!("{} AS {}", column(a, &c), quote(&n)));
                    keys.push(column(a, &c));
                }
            }
            Item::Value(value, v, aggregate) => {
                select.push(format!("{value} AS {}", quote(&v.name)));
                if !aggregate {
                    keys.push(value.clone());
                }
            }
        }
    }
    (select, keys)
}
