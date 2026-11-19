//! \[EXTENSION\] [GS §10.3] How a PGQL or Cypher query's expressions,
//! outside its pattern, read the pattern's variables: as columns of the
//! GRAPH_TABLE the query reads, each a column of what it reads of the
//! match.
//!
//! * `v.p`, of a singleton `v`, is a column of the property;
//! * `v` alone is a column of ELEMENT_ID ( v );
//! * a graph function of a variable, by its GPML name or the alias PGQL
//!   or Cypher has (`ID`, `LENGTH`, `NODES`, `RELATIONSHIPS`), is a
//!   column of it;
//! * an aggregate whose argument names a group variable aggregates the
//!   group of each match, horizontally [P1 §4.4], and is a column of it;
//!   any other aggregates the query's rows, as SQL's does;
//! * Cypher's `collect ( x )` is ARRAY_AGG ( x ).
//!
//! A group variable anywhere else is GRF-0022. A subquery reads none of
//! the variables.

use super::{Bound, Rewrite};
use crate::ast::{
    ColumnSpecification, GraphColumn, GraphFunction, GraphFunctionName, GraphVariable, SetArg, SetFunctionSpecification, TableName,
    ValueExpression,
};
use crate::error::{Code, Result, SqlError};
use crate::functions::{ARRAY_AGG, LISTAGG};
use crate::lexer::{Tok, delim, kw};
use crate::parser::{Degree, Exposed, Parser};

/// Cypher's `collect`, ARRAY_AGG.
const COLLECT: &str = "COLLECT";
/// The aggregates named by identifiers whose argument may name a group.
const NAMED_AGGREGATES: [&str; 4] = [LISTAGG, ARRAY_AGG, COLLECT, "EVERY"];
/// The aggregates named by key words of SQL.
const KEY_WORD_AGGREGATES: [&str; 7] = [kw::COUNT, kw::SUM, kw::AVG, kw::MIN, kw::MAX, kw::ANY, kw::SOME];
/// The graph functions PGQL and Cypher name otherwise.
const ALIASES: [(&str, GraphFunctionName); 4] = [
    ("ID", GraphFunctionName::ElementId),
    ("LENGTH", GraphFunctionName::PathLength),
    ("NODES", GraphFunctionName::Vertices),
    ("RELATIONSHIPS", GraphFunctionName::Edges),
];

impl Rewrite {
    /// What variable `name` is, if a pattern has it: as the first that
    /// has it binds it.
    fn var(&self, name: &str) -> Option<Exposed> {
        self.tables.iter().find_map(|b| b.vars.get(name).copied())
    }

    /// The column of `value`, of the text `key`, of the GRAPH_TABLE that
    /// first binds variable `var`, as `Bound::column` makes it.
    fn column(&mut self, var: &str, key: (String, Option<&str>), value: ValueExpression, at: usize) -> Result<ValueExpression> {
        let found = self.tables.iter_mut().find(|b| b.vars.contains_key(var));
        let bound = found.ok_or_else(|| SqlError::at(Code::Api, at, format!("[DESIGN] no pattern binds {var}")))?;
        Ok(bound.column(key, value, at))
    }
}

impl Bound {
    /// The column of `value`, of the text `key`: one already made of the
    /// same text, else a new one, named `preferred` if no other is, so a
    /// select list's column of it is named so, else by its place.
    pub(super) fn column(&mut self, (key, preferred): (String, Option<&str>), value: ValueExpression, at: usize) -> ValueExpression {
        let found = self.columns.iter().find(|(k, _)| *k == key).map(|(_, c)| c.clone());
        let column = match found {
            Some(GraphColumn::Value { name: Some(n), .. }) => n,
            Some(GraphColumn::Value { name: None, .. } | GraphColumn::All(_)) | None => {
                let named = |c: &GraphColumn| if let GraphColumn::Value { name: Some(n), .. } = c { Some(n.clone()) } else { None };
                let taken = |p: &str| self.columns.iter().any(|(_, c)| named(c).as_deref() == Some(p));
                let free = preferred.filter(|p| !taken(p)).map(str::to_owned);
                let name = free.unwrap_or_else(|| format!("${}", self.columns.len().saturating_add(1)));
                self.columns.push((key, GraphColumn::Value { value, name: Some(name.clone()), at }));
                name
            }
        };
        let qualifier = Some(TableName { auth: None, ident: self.table.clone(), at });
        ValueExpression::Column(ColumnSpecification { qualifier, column, catalog: None, at })
    }
}

/// Whether `e` is bound to one element, or none, per match.
fn single(e: Exposed) -> bool {
    match e.degree {
        Degree::Singleton | Degree::Conditional => true,
        Degree::Group { .. } => false,
    }
}

/// GRF-0022 of group variable `name` read where a singleton is.
fn group_read(name: &str, at: usize) -> SqlError {
    SqlError::at(Code::SyntaxRule, at, format!("[GS §10.3] group variable {name} is read outside an aggregate (GRF-0022)"))
}

impl Parser {
    /// [GS §10] A primary of the compatibility syntax, if one begins
    /// here: a WITHIN GROUP (GRAPH PATH) aggregate; or, in a PGQL or
    /// Cypher query, what reads its pattern's variables.
    ///
    /// # Errors
    ///
    /// SQLCODE -102 if it does not conform to its Format; -103 (GRF-0022)
    /// of a group variable read outside an aggregate.
    pub(in crate::parser) fn compat_primary(&mut self) -> Result<Option<ValueExpression>> {
        if let Some(path) = self.graph_path_aggregate()? {
            return Ok(Some(path));
        }
        if self.compat.last().is_none_or(|f| f.rewrite.is_none()) {
            return Ok(None);
        }
        if self.is_word(COLLECT) && *self.peek_at(1) == Tok::Delimiter(delim::LEFT_PAREN) {
            return self.collect().map(Some);
        }
        if let Some(f) = self.aliased_function() {
            return self.variable_function(f).map(Some);
        }
        if let Some(group) = self.horizontal_of() {
            return self.horizontal(&group).map(Some);
        }
        if let Some(listagg) = self.pgql_listagg()? {
            return Ok(Some(listagg));
        }
        if self.graph > 0 {
            return Ok(None);
        }
        self.variable_read()
    }

    /// A grouping column of a PGQL query that reads a variable of its
    /// pattern, if one begins here: the column of the match it reads.
    ///
    /// # Errors
    ///
    /// As `compat_primary`.
    pub(in crate::parser) fn compat_column(&mut self) -> Result<Option<ColumnSpecification>> {
        if self.graph > 0 || self.compat.last().is_none_or(|f| f.rewrite.is_none()) {
            return Ok(None);
        }
        if let Some(ValueExpression::Column(c)) = self.variable_read()? { Ok(Some(c)) } else { Ok(None) }
    }

    /// The rewrite of the frame at hand. Of none, as `compat_primary`
    /// sees to, an error.
    pub(super) fn rewrite(&mut self) -> Result<&mut Rewrite> {
        let found = self.compat.last_mut().and_then(|f| f.rewrite.as_mut());
        found.ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] a PGQL or Cypher query is read in a frame of its own"))
    }

    /// The graph function named here, by its name or an alias, if one of
    /// a variable of the pattern begins here.
    fn aliased_function(&self) -> Option<GraphFunctionName> {
        let Tok::Identifier(word) = self.peek() else { return None };
        let f = GraphFunctionName::named(word).or_else(|| ALIASES.iter().find(|(a, _)| a == word).map(|(_, f)| *f))?;
        let operand = matches!(self.peek_at(2), Tok::Identifier(v) if self.compat.last().and_then(|r| r.rewrite.as_ref()).is_some_and(|r| r.var(v).is_some()));
        let call = *self.peek_at(1) == Tok::Delimiter(delim::LEFT_PAREN) && *self.peek_at(3) == Tok::Delimiter(delim::RIGHT_PAREN);
        (operand && call).then_some(f)
    }

    /// Graph function `f` of the variable here: itself inside the pattern
    /// or a horizontal aggregate, else a column of it.
    fn variable_function(&mut self, f: GraphFunctionName) -> Result<ValueExpression> {
        let at = self.here();
        self.advance();
        self.advance();
        let operand = GraphVariable { at: self.here(), name: self.identifier()?, anonymous: false };
        self.advance();
        let (key, var) = (format!("{}({})", f.word(), operand.name), operand.name.clone());
        let value = ValueExpression::Graph(Box::new(GraphFunction { name: f, operand, property: None }), at);
        if self.graph > 0 {
            return Ok(value);
        }
        self.rewrite()?.column(&var, (key, None), value, at)
    }

    /// Cypher's `collect ( x )`: ARRAY_AGG ( x ), its argument read as an
    /// aggregate's is.
    fn collect(&mut self) -> Result<ValueExpression> {
        let horizontal = self.horizontal_of();
        let at = self.here();
        let key = self.text_to(self.past_closing(1).unwrap_or(1));
        self.graph = self.graph.saturating_add(usize::from(horizontal.is_some()));
        let read = self.collected(at);
        self.graph = self.graph.saturating_sub(usize::from(horizontal.is_some()));
        let value = read?;
        match horizontal {
            Some(group) => self.rewrite()?.column(&group, (key, None), value, at),
            None => Ok(value),
        }
    }

    /// `collect ( x )`, read.
    fn collected(&mut self, at: usize) -> Result<ValueExpression> {
        self.advance();
        self.expect_delim(delim::LEFT_PAREN)?;
        let arg = SetArg::All(Box::new(self.value_expression()?));
        self.expect_delim(delim::RIGHT_PAREN)?;
        let f = SetFunctionSpecification {
            name: ARRAY_AGG,
            arg,
            order_by: Vec::new(),
            listagg: None,
            json: None,
            filter: None,
            semantics: None,
            at,
        };
        Ok(ValueExpression::SetFunction(f))
    }

    /// The text of the tokens from here up to the one at offset `past`,
    /// not included.
    fn text_to(&self, past: usize) -> String {
        let last = self.toks.get(self.pos.saturating_add(past).saturating_sub(1)).map_or(self.here(), |t| t.end);
        self.src.get(self.here()..last).unwrap_or_default().iter().collect()
    }

    /// Outside a pattern, the group variable of the patterns named first
    /// in the argument of an aggregate beginning here, if one does.
    fn horizontal_of(&self) -> Option<String> {
        if self.graph > 0 {
            return None;
        }
        let named = match self.peek() {
            Tok::Identifier(w) => NAMED_AGGREGATES.contains(&w.as_str()),
            Tok::KeyWord(k) => KEY_WORD_AGGREGATES.contains(k),
            Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::LargeObjectLength(_)
            | Tok::Delimiter(_)
            | Tok::End => false,
        };
        let rewrite = self.compat.last().and_then(|f| f.rewrite.as_ref())?;
        let past = if named && *self.peek_at(1) == Tok::Delimiter(delim::LEFT_PAREN) { self.past_closing(1).unwrap_or(0) } else { 0 };
        let group = |n: usize| {
            if let Tok::Identifier(v) = self.peek_at(n) { rewrite.var(v).filter(|e| !single(*e)).map(|_| v.clone()) } else { None }
        };
        (2..past).find_map(group)
    }

    /// The aggregate here, of each match's group of variable `group`: a
    /// column of it.
    fn horizontal(&mut self, group: &str) -> Result<ValueExpression> {
        let at = self.here();
        let key = self.text_to(self.past_closing(1).unwrap_or(1));
        self.graph = self.graph.saturating_add(1);
        let read = self.primary();
        self.graph = self.graph.saturating_sub(1);
        let value = read?;
        self.rewrite()?.column(group, (key, None), value, at)
    }

    /// A read of a variable here, `v.p` or `v` alone, if one begins here:
    /// a column of it.
    fn variable_read(&mut self) -> Result<Option<ValueExpression>> {
        let Tok::Identifier(name) = self.peek() else { return Ok(None) };
        let name = name.clone();
        let Some(exposed) = self.rewrite()?.var(&name) else { return Ok(None) };
        let at = self.here();
        let property = *self.peek_at(1) == Tok::Delimiter(delim::PERIOD) && matches!(self.peek_at(2), Tok::Identifier(_));
        if *self.peek_at(1) == Tok::Delimiter(delim::LEFT_PAREN) || (property && *self.peek_at(3) == Tok::Delimiter(delim::LEFT_PAREN)) {
            return Ok(None);
        }
        if !single(exposed) {
            return Err(group_read(&name, at));
        }
        self.advance();
        if !property {
            let operand = GraphVariable { name: name.clone(), at, anonymous: false };
            let f = GraphFunction { name: GraphFunctionName::ElementId, operand, property: None };
            let value = ValueExpression::Graph(Box::new(f), at);
            return self.rewrite()?.column(&name, (format!("ELEMENT_ID({name})"), Some(&name)), value, at).map(Some);
        }
        self.advance();
        let column = self.identifier()?;
        let qualifier = Some(TableName { auth: None, ident: name.clone(), at });
        let value = ValueExpression::Column(ColumnSpecification { qualifier, column: column.clone(), catalog: None, at });
        self.rewrite()?.column(&name, (format!("{name}.{column}"), Some(&column)), value, at).map(Some)
    }
}
