//! \[EXTENSION\] The clauses of a GQL linear statement that read the
//! graph or the binding table (graph standard §9): MATCH and OPTIONAL
//! MATCH, FILTER, LET, FOR, ORDER BY with OFFSET and LIMIT, CALL, and
//! RETURN or FINISH.

use super::super::Parser;
use crate::ast::{
    GqlCall, GqlClause, GqlPage, GqlResult, GqlReturn, GqlReturnItem, GqlSort, GqlSpan, GraphReference, GraphRows, GraphTable, TableName,
};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};

// [ISO/IEC 39075:2024] The words of the clauses.
/// MATCH.
const MATCH: &str = "MATCH";
/// OPTIONAL, of OPTIONAL MATCH and OPTIONAL CALL.
const OPTIONAL: &str = "OPTIONAL";
/// FILTER.
const FILTER: &str = "FILTER";
/// LET.
const LET: &str = "LET";
/// FOR.
const FOR: &str = "FOR";
/// ORDINALITY, of FOR's WITH ORDINALITY.
const ORDINALITY: &str = "ORDINALITY";
/// OFFSET, of FOR's WITH OFFSET and of a page.
const OFFSET: &str = "OFFSET";
/// SKIP, OFFSET's synonym.
const SKIP: &str = "SKIP";
/// LIMIT.
const LIMIT: &str = "LIMIT";
/// ORDER, of ORDER BY.
const ORDER: &str = "ORDER";
/// CALL.
const CALL: &str = "CALL";
/// YIELD, of a named procedure's call.
const YIELD: &str = "YIELD";
/// RETURN.
const RETURN: &str = "RETURN";
/// FINISH.
const FINISH: &str = "FINISH";
/// NULLS, of NULLS FIRST and NULLS LAST.
const NULLS: &str = "NULLS";
/// DESCENDING, DESC's long form.
const DESCENDING: &str = "DESCENDING";
/// ASCENDING, ASC's long form.
const ASCENDING: &str = "ASCENDING";

impl Parser {
    /// The next clause, if one is next.
    pub(super) fn gql_clause(&mut self) -> Result<Option<GqlClause>> {
        let optional = self.at_gql(OPTIONAL);
        if (optional && self.gql_at(1, MATCH)) || self.at_gql(MATCH) {
            return self.gql_match(optional).map(Some);
        }
        if (optional && self.gql_at(1, CALL)) || self.at_gql(CALL) {
            return self.gql_call(optional).map(|c| Some(GqlClause::Call(c)));
        }
        if self.accept_gql(FILTER) || self.accept_gql(kw::WHERE) {
            return self.gql_condition().map(|c| Some(GqlClause::Filter(c)));
        }
        if self.accept_gql(LET) {
            return self.gql_let().map(Some);
        }
        if self.at_gql(FOR) {
            return self.gql_for().map(Some);
        }
        if self.at_page() {
            return self.gql_page().map(|p| Some(GqlClause::Page(p)));
        }
        self.gql_modifying()
    }

    /// \[OPTIONAL\] MATCH `<graph pattern>`, its Syntax Rules checked as a
    /// GRAPH_TABLE's are.
    fn gql_match(&mut self, optional: bool) -> Result<GqlClause> {
        if optional {
            self.advance();
        }
        let at = self.here();
        self.advance();
        let start = self.token_number();
        let pattern = self.graph_pattern()?;
        let span = GqlSpan { start, end: self.token_number() };
        let table = GraphTable {
            graph: GraphReference::Named(TableName { auth: None, ident: String::new(), at }),
            pattern,
            rows: GraphRows::Match,
            columns: Vec::new(),
            correlation: None,
            system_time: None,
            options: crate::ast::GraphLimits::default(),
            at,
        };
        super::super::graph::check(&table)?;
        Ok(GqlClause::Match { optional, pattern: table.pattern, span })
    }

    /// LET `<variable>` = `<value>` \[, ...\], after LET.
    fn gql_let(&mut self) -> Result<GqlClause> {
        let mut bindings = Vec::new();
        loop {
            let name = self.identifier()?;
            self.expect_delim(delim::EQUALS)?;
            bindings.push((name, self.gql_expression()?));
            if !self.accept_delim(delim::COMMA) {
                return Ok(GqlClause::Let(bindings));
            }
        }
    }

    /// FOR `<variable>` IN `<list>` \[WITH ORDINALITY | WITH OFFSET
    /// `<variable>`\].
    fn gql_for(&mut self) -> Result<GqlClause> {
        self.advance();
        let variable = self.identifier()?;
        self.expect_kw(kw::IN)?;
        let list = self.gql_expression()?;
        if !self.accept_kw(kw::WITH) {
            return Ok(GqlClause::For { variable, list, place: None });
        }
        let offset = self.accept_gql(OFFSET);
        if !offset {
            self.expect_gql(ORDINALITY)?;
        }
        let place = Some((self.identifier()?, offset));
        Ok(GqlClause::For { variable, list, place })
    }

    /// Whether ORDER BY, OFFSET, SKIP or LIMIT is next.
    fn at_page(&self) -> bool {
        (self.at_gql(ORDER) && self.gql_at(1, kw::BY)) || self.at_gql(OFFSET) || self.at_gql(SKIP) || self.at_gql(LIMIT)
    }

    /// \[ORDER BY `<sort key>` \[, ...\]\] \[{OFFSET | SKIP} `<value>`\]
    /// \[LIMIT `<value>`\].
    fn gql_page(&mut self) -> Result<GqlPage> {
        let mut page = GqlPage::default();
        if self.at_gql(ORDER) && self.gql_at(1, kw::BY) {
            self.advance();
            self.advance();
            page.order.push(self.gql_sort()?);
            while self.accept_delim(delim::COMMA) {
                page.order.push(self.gql_sort()?);
            }
        }
        if self.accept_gql(OFFSET) || self.accept_gql(SKIP) {
            page.offset = Some(self.gql_expression()?);
        }
        if self.accept_gql(LIMIT) {
            page.limit = Some(self.gql_expression()?);
        }
        Ok(page)
    }

    /// `<value>` \[ASC | DESC\] \[NULLS {FIRST | LAST}\].
    fn gql_sort(&mut self) -> Result<GqlSort> {
        let value = self.gql_expression()?;
        let descending = self.accept_gql(kw::DESC) || self.accept_gql(DESCENDING);
        if !descending && !self.accept_gql(kw::ASC) {
            self.accept_gql(ASCENDING);
        }
        let nulls_first = if self.accept_gql(NULLS) {
            let first = self.accept_gql(kw::FIRST);
            if !first {
                self.expect_gql(kw::LAST)?;
            }
            Some(first)
        } else {
            None
        };
        Ok(GqlSort { value, descending, nulls_first })
    }

    /// \[OPTIONAL\] CALL, of an inline procedure or a named one.
    fn gql_call(&mut self, optional: bool) -> Result<GqlCall> {
        if optional {
            self.advance();
        }
        self.advance();
        let by_name = matches!(self.peek(), Tok::Identifier(_)) && !self.is_delim(delim::LEFT_PAREN);
        if by_name {
            return self.gql_named_call(optional);
        }
        let scope = if self.accept_delim(delim::LEFT_PAREN) {
            let mut names = Vec::new();
            while !self.accept_delim(delim::RIGHT_PAREN) {
                names.push(self.identifier()?);
                if !self.is_delim(delim::RIGHT_PAREN) {
                    self.expect_delim(delim::COMMA)?;
                }
            }
            Some(names)
        } else {
            None
        };
        self.expect_delim(delim::LEFT_BRACE)?;
        let body = self.nested(Parser::gql_procedure)?;
        self.expect_delim(delim::RIGHT_BRACE)?;
        Ok(GqlCall::Inline { optional, scope, body: Box::new(body) })
    }

    /// `<procedure name>` ( `<value>` \[, ...\] ) \[YIELD `<column>` \[AS
    /// `<variable>`\] \[, ...\]\].
    fn gql_named_call(&mut self, optional: bool) -> Result<GqlCall> {
        let name = self.table_name()?;
        self.expect_delim(delim::LEFT_PAREN)?;
        let mut arguments = Vec::new();
        while !self.accept_delim(delim::RIGHT_PAREN) {
            arguments.push(self.gql_expression()?);
            if !self.is_delim(delim::RIGHT_PAREN) {
                self.expect_delim(delim::COMMA)?;
            }
        }
        let mut yields = Vec::new();
        if self.accept_gql(YIELD) {
            loop {
                let column = self.identifier()?;
                let variable = if self.accept_kw(kw::AS) { self.identifier()? } else { column.clone() };
                yields.push((column, variable));
                if !self.accept_delim(delim::COMMA) {
                    break;
                }
            }
        }
        Ok(GqlCall::Named { optional, name, arguments, yields })
    }

    /// RETURN, FINISH, or nothing.
    pub(super) fn gql_result(&mut self) -> Result<GqlResult> {
        if self.accept_gql(FINISH) {
            return Ok(GqlResult::Finish);
        }
        if !self.accept_gql(RETURN) {
            return Ok(GqlResult::None);
        }
        let distinct = self.accept_kw(kw::DISTINCT);
        if !distinct {
            self.accept_kw(kw::ALL);
        }
        let items = if self.accept_delim(delim::ASTERISK) { None } else { Some(self.gql_return_items()?) };
        let mut group_by = Vec::new();
        if self.is_kw(kw::GROUP) && self.next_is_kw(kw::BY) {
            self.advance();
            self.advance();
            loop {
                let at = self.here();
                group_by.push((self.identifier()?, at));
                if !self.accept_delim(delim::COMMA) {
                    break;
                }
            }
        }
        let page = self.gql_page()?;
        Ok(GqlResult::Return(Box::new(GqlReturn { distinct, items, group_by, page })))
    }

    /// `<return item>` \[, ...\].
    fn gql_return_items(&mut self) -> Result<Vec<GqlReturnItem>> {
        let mut items = Vec::new();
        loop {
            let value = self.gql_expression()?;
            let name = if self.accept_kw(kw::AS) { Some(self.identifier()?) } else { None };
            items.push(GqlReturnItem { value, name });
            if !self.accept_delim(delim::COMMA) {
                return Ok(items);
            }
        }
    }
}
