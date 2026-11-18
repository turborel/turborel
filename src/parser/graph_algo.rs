//! \[EXTENSION\] The functions of GRAPH_ALGO, the graph standard's §18
//! (RULES.md D-GRAPH-ALGO), as a table reference and as INTO PROPERTY:
//!
//! ```text
//! <graph algorithm table> ::= GRAPH_ALGO . <name> ( <arguments> )
//!     [ [ AS ] <correlation name> [ ( <derived column list> ) ] ]
//! <into property statement> ::= GRAPH_ALGO . <name> ( <arguments> )
//!     INTO PROPERTY <column name> OF <table name> [ ( <key column name> ) ]
//! ```
//!
//! The invocation is a `<PTF derived table>`'s, written without TABLE ( ),
//! its arguments positional and then named, as `PAGERANK(TABLE (e),
//! damping => 0.85)`. INTO PROPERTY is read as the MERGE it is
//! (`ast/merge.rs`).

use super::Parser;
use super::graph_ddl::PROPERTY;
use crate::ast::{IntoProperty, PtfTable, SqlStatement, TableReference};
use crate::error::Result;
use crate::graph::algo::{Algorithm, Output, SCHEMA};
use crate::lexer::{Tok, delim, kw};

/// The tokens of `GRAPH_ALGO . <name>` before its parenthesis.
const NAME_TOKENS: usize = 3;

/// Whether `tokens`, a statement's, begin an `<into property statement>`:
/// GRAPH_ALGO, and INTO PROPERTY after the parentheses of the
/// invocation, which a query of it lacks.
pub(crate) fn begins_property_statement<'t>(tokens: impl IntoIterator<Item = &'t Tok>) -> bool {
    let tokens: Vec<&Tok> = tokens.into_iter().collect();
    if !matches!(tokens.first(), Some(Tok::Identifier(schema)) if schema == SCHEMA) {
        return false;
    }
    let mut depth = 0_usize;
    for (k, t) in tokens.iter().enumerate() {
        if **t == Tok::Delimiter(delim::LEFT_PAREN) {
            depth = depth.saturating_add(1);
        } else if **t == Tok::Delimiter(delim::RIGHT_PAREN) {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                let rest = tokens.get(k.saturating_add(1)..).unwrap_or_default();
                return matches!((rest.first(), rest.get(1)), (Some(Tok::KeyWord(kw::INTO)), Some(Tok::Identifier(w))) if w == PROPERTY);
            }
        }
    }
    false
}

impl Parser {
    /// Whether a function of GRAPH_ALGO is invoked here: GRAPH_ALGO, a
    /// period, a name and a left parenthesis.
    pub(in crate::parser) fn at_graph_algorithm(&self) -> bool {
        let Tok::Identifier(schema) = self.peek() else { return false };
        schema == SCHEMA
            && matches!(self.peek_at(1), Tok::Delimiter(delim::PERIOD))
            && matches!(self.peek_at(2), Tok::Identifier(_))
            && matches!(self.peek_at(NAME_TOKENS), Tok::Delimiter(delim::LEFT_PAREN))
    }

    /// `<graph algorithm table>`, which begins here.
    ///
    /// # Errors
    ///
    /// The text is not one.
    pub(in crate::parser) fn graph_algorithm_table(&mut self) -> Result<TableReference> {
        let at = self.here();
        let invocation = self.ptf_invocation()?;
        let correlation = self.correlation()?;
        let columns = if correlation.is_some() { self.derived_columns()? } else { None };
        Ok(TableReference::Polymorphic(Box::new(PtfTable { invocation, correlation, columns, at })))
    }

    /// `<into property statement>`, which begins here, read as its MERGE.
    ///
    /// # Errors
    ///
    /// The text is not one; GRAPH_ALGO has no function of its name, or one
    /// whose result is of pairs, not of vertices.
    pub(in crate::parser) fn property_statement(&mut self) -> Result<SqlStatement> {
        let start = self.here();
        let invocation = self.ptf_invocation()?;
        let text = self.text_from(start);
        self.expect_kw(kw::INTO)?;
        self.expect_word(PROPERTY)?;
        let column = self.identifier()?;
        self.expect_kw(kw::OF)?;
        let table_start = self.here();
        self.table_name()?;
        let table = self.text_from(table_start);
        let key = if self.accept_delim(delim::LEFT_PAREN) {
            let k = self.identifier()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            Some(k)
        } else {
            None
        };
        let Some(algorithm) = Algorithm::named(&invocation.name) else {
            return Err(self.rule(format!("[EXTENSION] there is no graph algorithm {SCHEMA}.{}", invocation.name)));
        };
        let [_, (value, output)] = algorithm.result() else {
            return Err(self.rule(format!("[EXTENSION] {SCHEMA}.{} gives pairs of vertices, no property of one", algorithm.name())));
        };
        let value_type = match output {
            Output::Real => Some("DOUBLE PRECISION"),
            Output::Count => Some("BIGINT"),
            Output::Key => None,
        };
        let property = IntoProperty { invocation: text, table, column, key, value: (*value).to_owned(), value_type };
        let merge = property.merge(property.key.as_deref().unwrap_or(&property.column));
        let SqlStatement::Merge(mut m) = Parser::new(&merge)?.sql_statement()? else {
            return Err(self.rule("[DESIGN] INTO PROPERTY is read as a MERGE"));
        };
        m.property = Some(Box::new(property));
        Ok(SqlStatement::Merge(m))
    }
}
