//! The query grammar: `<cursor specification>` with ORDER BY [X3.135-1986
//! §8.3], `<query expression>` [ISO/IEC 9075:1992 §7.10], `<query
//! specification>` [X3.135-1986 §5.25] and `<table expression>`
//! [X3.135-1986 §5.19-§5.23]. Split from parser.rs (RULES.md R-SIZE-2).

use super::Parser;
use crate::ast::{
    Correspondence, CursorSpecification, DerivedTable, JoinKind, JoinSpecification, JoinedTable, NamedTable, QueryExpression,
    QuerySpecification, RowElement, SelectItem, SelectList, SelectSublist, SetOperation, SetOperator, SortColumn, SortSpecification,
    TableExpression, TableName, TableReference, ValuesRow,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

impl Parser {
    /// [X3.135-1986 §8.3; ISO/IEC 9075:1992 §13.1] `<cursor specification>`
    /// ::= `<query expression>` \[`<order by clause>`\] \[`<updatability
    /// clause>`\]
    ///
    /// # Errors
    ///
    /// A syntax error, or valid SQL this implementation does not process.
    pub fn cursor_specification(&mut self) -> Result<CursorSpecification> {
        let query = self.query_expression()?;
        // <order by clause> ::= ORDER BY <sort specification> [{, <sort specification>}...]
        let mut order_by = Vec::new();
        if self.accept_kw(kw::ORDER) {
            self.expect_kw(kw::BY)?;
            order_by.push(self.sort_specification()?);
            while self.accept_delim(delim::COMMA) {
                order_by.push(self.sort_specification()?);
            }
        }
        let updatability = self.updatability_clause()?;
        self.end()?;
        Ok(CursorSpecification { query, order_by, updatability })
    }

    /// [X3.135-1986 §8.3] `<sort specification>` ::= {`<unsigned integer>` | `<column specification>`} \[ASC | DESC\]
    pub(super) fn sort_specification(&mut self) -> Result<SortSpecification> {
        let key = if matches!(self.peek(), Tok::ExactNumeric(_)) {
            SortColumn::Position(self.unsigned_integer()?)
        } else {
            SortColumn::Column(self.column_specification()?)
        };
        // [ISO/IEC 9075:1992 §13.1] [<collate clause>]: refused.
        self.collate_clause()?;
        let descending = if self.accept_kw(kw::DESC) {
            true
        } else {
            self.accept_kw(kw::ASC);
            false
        };
        Ok(SortSpecification { key, descending })
    }

    /// [X3.135-1986 §8.3; ISO/IEC 9075:1992 §7.10] `<query expression>`
    /// ::= `<query term>` | `<query expression>` {UNION | EXCEPT} \[ALL\]
    /// \[`<corresponding spec>`\] `<query term>`
    pub(super) fn query_expression(&mut self) -> Result<QueryExpression> {
        self.nested(|p| {
            let mut left = p.query_term()?;
            loop {
                let at = p.here();
                let op = if p.accept_kw(kw::UNION) {
                    SetOperator::Union
                } else if p.accept_kw(kw::EXCEPT) {
                    SetOperator::Except
                } else {
                    return Ok(left);
                };
                p.deepen()?;
                let (all, corresponding) = p.set_quantifiers()?;
                let right = p.query_term()?;
                left = QueryExpression::SetOp(Box::new(SetOperation { op, all, corresponding, left, right, at }));
            }
        })
    }

    /// [ISO/IEC 9075:1992 §7.10] `<query term>` ::= `<query primary>` |
    /// `<query term>` INTERSECT \[ALL\] \[`<corresponding spec>`\] `<query
    /// primary>`
    fn query_term(&mut self) -> Result<QueryExpression> {
        let mut left = self.query_primary()?;
        loop {
            let at = self.here();
            if !self.accept_kw(kw::INTERSECT) {
                return Ok(left);
            }
            self.deepen()?;
            let (all, corresponding) = self.set_quantifiers()?;
            let right = self.query_primary()?;
            left = QueryExpression::SetOp(Box::new(SetOperation { op: SetOperator::Intersect, all, corresponding, left, right, at }));
        }
    }

    /// \[ALL\] \[`<corresponding spec>`\] after a set operator, where
    /// `<corresponding spec>` ::= CORRESPONDING \[BY (`<corresponding column
    /// list>`)\] [ISO/IEC 9075:1992 §7.10].
    fn set_quantifiers(&mut self) -> Result<(bool, Correspondence)> {
        let all = self.accept_kw(kw::ALL);
        if !self.accept_kw(kw::CORRESPONDING) {
            return Ok((all, Correspondence::Position));
        }
        if !self.accept_kw(kw::BY) {
            return Ok((all, Correspondence::Names));
        }
        Ok((all, Correspondence::By(self.name_list()?)))
    }

    /// [ISO/IEC 9075:1992 §7.10] `<query primary>`: (`<query
    /// expression>`), a `<query specification>`, a `<table value
    /// constructor>`, TABLE `<table name>`, which is (SELECT * FROM `<table
    /// name>`) \[SR2\], or a `<joined table>`, which is SELECT * FROM it
    /// \[SR13b\].
    fn query_primary(&mut self) -> Result<QueryExpression> {
        let query_next = |p: &Self| {
            matches!(
                p.peek_at(1),
                Tok::KeyWord(kw::SELECT) | Tok::KeyWord(kw::VALUES) | Tok::KeyWord(kw::TABLE) | Tok::Delimiter(delim::LEFT_PAREN)
            )
        };
        if self.is_delim(delim::LEFT_PAREN) && query_next(self) {
            self.advance();
            let q = self.query_expression()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            return Ok(q);
        }
        if self.is_kw(kw::SELECT) {
            return Ok(QueryExpression::Spec(Box::new(self.query_specification()?)));
        }
        if self.accept_kw(kw::VALUES) {
            return self.table_value_constructor();
        }
        if self.accept_kw(kw::TABLE) {
            let table = self.table_name()?;
            return Ok(all_of(TableReference::Table(NamedTable { table, correlation: None, columns: None })));
        }
        let at = self.here();
        let joined = self.table_reference()?;
        if !matches!(joined, TableReference::Joined(_)) {
            return Err(SqlError::at(
                Code::Syntax,
                at,
                "[ISO/IEC 9075:1992 §7.10 F] a <query primary> that names a table is a <joined table>",
            ));
        }
        Ok(all_of(joined))
    }

    /// [ISO/IEC 9075:1992 §7.2] The rest of a `<table value constructor>`,
    /// after VALUES: its `<row value constructor>`s, each one element or a
    /// list of them in parentheses [§7.1].
    fn table_value_constructor(&mut self) -> Result<QueryExpression> {
        let mut rows = vec![self.values_row()?];
        while self.accept_delim(delim::COMMA) {
            rows.push(self.values_row()?);
        }
        Ok(QueryExpression::Values(rows))
    }

    /// [ISO/IEC 9075:1992 §7.1] A `<row value constructor>` of a `<table
    /// value constructor>`: (`<row value constructor list>`), or one
    /// element.
    fn values_row(&mut self) -> Result<ValuesRow> {
        let (at, start) = (self.here(), self.pos);
        if self.is_delim(delim::LEFT_PAREN) && !self.at_subquery() {
            self.advance();
            let mut elements = vec![self.row_element()?];
            while self.accept_delim(delim::COMMA) {
                elements.push(self.row_element()?);
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
            // [SR2] "(V)" is a list, unless an operator after it makes it
            // the start of one element.
            let operator =
                [delim::PLUS, delim::MINUS, delim::ASTERISK, delim::SOLIDUS, delim::CONCATENATION].iter().any(|d| self.is_delim(d));
            if !(operator && elements.len() == 1) {
                return Ok(ValuesRow { elements, at });
            }
            self.pos = start;
        }
        Ok(ValuesRow { elements: vec![self.row_element()?], at })
    }

    /// [ISO/IEC 9075:1992 §7.1] `<row value constructor element>` ::=
    /// `<value expression>` | NULL | DEFAULT
    fn row_element(&mut self) -> Result<RowElement> {
        let at = self.here();
        if self.accept_kw(kw::NULL) {
            return Ok(RowElement::Null(at));
        }
        if self.accept_kw(kw::DEFAULT) {
            return Ok(RowElement::Default(at));
        }
        Ok(RowElement::Value(self.value_expression()?, at))
    }

    /// [X3.135-1986 §5.25; ISO/IEC 9075:1992 §7.9] `<select list>` ::= * |
    /// `<select sublist>` \[{, `<select sublist>`}...\]
    pub(super) fn select_list(&mut self) -> Result<SelectList> {
        if self.accept_delim(delim::ASTERISK) {
            return Ok(SelectList::Star);
        }
        let mut list = vec![self.select_sublist()?];
        while self.accept_delim(delim::COMMA) {
            list.push(self.select_sublist()?);
        }
        Ok(SelectList::Values(list))
    }

    /// [ISO/IEC 9075:1992 §7.9] `<select sublist>` ::= `<qualifier>`.* |
    /// `<value expression>` \[\[AS\] `<column name>`\]
    fn select_sublist(&mut self) -> Result<SelectSublist> {
        let star_after = |p: &Self, n: usize| matches!(p.peek_at(n), Tok::Delimiter(delim::ASTERISK));
        let period_at = |p: &Self, n: usize| matches!(p.peek_at(n), Tok::Delimiter(delim::PERIOD));
        let named = matches!(self.peek(), Tok::Identifier(_)) && period_at(self, 1);
        let schema = named && matches!(self.peek_at(2), Tok::Identifier(_)) && period_at(self, 3) && star_after(self, 4);
        let catalog = named
            && matches!(self.peek_at(2), Tok::Identifier(_))
            && period_at(self, 3)
            && matches!(self.peek_at(4), Tok::Identifier(_))
            && period_at(self, 5)
            && star_after(self, 6);
        if catalog {
            // [ISO/IEC 9075:1992 §7.9] <qualifier> . * of its <catalog
            // name>: refused.
            let at = self.here();
            self.catalog_refused(at);
            self.identifier()?;
            self.expect_delim(delim::PERIOD)?;
        }
        if catalog || schema || (named && star_after(self, 2)) {
            let at = self.here();
            let first = self.identifier()?;
            self.expect_delim(delim::PERIOD)?;
            let qualifier = if schema || catalog {
                let ident = self.identifier()?;
                self.expect_delim(delim::PERIOD)?;
                TableName { auth: Some(first), ident, at }
            } else {
                TableName { auth: None, ident: first, at }
            };
            self.expect_delim(delim::ASTERISK)?;
            return Ok(SelectSublist::All(qualifier));
        }
        let at = self.here();
        let value = self.value_expression()?;
        let alias = self.correlation()?;
        Ok(SelectSublist::Derived(SelectItem { value, alias, at }))
    }

    /// [X3.135-1986 §5.25] `<query specification>` ::= SELECT \[ALL | DISTINCT\] `<select list>` `<table expression>`
    pub(super) fn query_specification(&mut self) -> Result<QuerySpecification> {
        self.expect_kw(kw::SELECT)?;
        let distinct = if self.accept_kw(kw::DISTINCT) {
            true
        } else {
            self.accept_kw(kw::ALL);
            false
        };
        // <select list> ::= <value expression> [{, <value expression>}...] | *
        let select_list = self.select_list()?;
        let table_expression = self.table_expression()?;
        Ok(QuerySpecification { distinct, select_list, table_expression })
    }

    /// [X3.135-1986 §5.19] `<table expression>` ::=
    ///     `<from clause>` \[`<where clause>`\] \[`<group by clause>`\] \[`<having clause>`\]
    pub(super) fn table_expression(&mut self) -> Result<TableExpression> {
        // [X3.135-1986 §5.20] <from clause> ::= FROM <table reference> [{,<table reference>}...]
        self.expect_kw(kw::FROM)?;
        let saved = self.depth;
        let from = self.table_references();
        self.depth = saved;
        let from = from?;
        // [X3.135-1986 §5.21] <where clause> ::= WHERE <search condition>
        let where_clause = if self.accept_kw(kw::WHERE) { Some(self.search_condition()?) } else { None };
        // [X3.135-1986 §5.22] <group by clause> ::= GROUP BY <column specification> [{, <column specification>}...]
        let mut group_by = Vec::new();
        if self.accept_kw(kw::GROUP) {
            self.expect_kw(kw::BY)?;
            // [ISO/IEC 9075:1992 §7.7] <grouping column reference> ::=
            // <column reference> [<collate clause>]: refused.
            group_by.push(self.column_specification()?);
            self.collate_clause()?;
            while self.accept_delim(delim::COMMA) {
                group_by.push(self.column_specification()?);
                self.collate_clause()?;
            }
        }
        // [X3.135-1986 §5.23] <having clause> ::= HAVING <search condition>
        let having = if self.accept_kw(kw::HAVING) { Some(self.search_condition()?) } else { None };
        Ok(TableExpression { from, where_clause, group_by, having })
    }

    /// The `<table reference>`s of a `<from clause>`. The plan joins them one
    /// level per reference after the first, so each counts toward
    /// `MAX_NESTING_DEPTH`; the caller restores the depth.
    fn table_references(&mut self) -> Result<Vec<TableReference>> {
        let mut from = vec![self.table_reference()?];
        while self.accept_delim(delim::COMMA) {
            self.deepen()?;
            from.push(self.table_reference()?);
        }
        Ok(from)
    }

    /// [X3.135-1986 §5.20; ISO/IEC 9075:1992 §6.3, §7.5] `<table
    /// reference>`: a table, derived table or parenthesized joined table,
    /// then the joins after it, each joining what is before it, as it
    /// counts one level toward the nesting limit.
    pub(super) fn table_reference(&mut self) -> Result<TableReference> {
        let first = self.table_primary()?;
        self.joins(first)
    }

    /// `left`, then the joins after it, each joining what is before it.
    ///
    /// [ISO/IEC 9075:1992 §7.5 F] A join's second operand is a `<table
    /// reference>`, so a joined table itself: one that needs a `<join
    /// specification>` and is followed by another join before it has
    /// taken the join that follows as its second operand's, as in "A JOIN B
    /// LEFT JOIN C ON c1 ON c2".
    fn joins(&mut self, first: TableReference) -> Result<TableReference> {
        let mut left = first;
        loop {
            let at = self.here();
            let Some((kind, natural)) = self.join_type()? else { return Ok(left) };
            self.deepen()?;
            let mut right = self.table_primary()?;
            let specified = !(kind == JoinKind::Cross || kind == JoinKind::Union || natural);
            if specified && self.at_join() {
                right = self.joins(right)?;
            }
            let on = self.join_specification(kind, natural)?;
            left = TableReference::Joined(Box::new(JoinedTable { left, right, kind, natural, on, at }));
        }
    }

    /// Whether a join begins here, its key words not taken.
    fn at_join(&self) -> bool {
        [kw::CROSS, kw::NATURAL, kw::INNER, kw::LEFT, kw::RIGHT, kw::FULL, kw::JOIN].iter().any(|w| self.is_kw(w))
            || (self.is_kw(kw::UNION) && self.next_is_kw(kw::JOIN))
    }

    /// [ISO/IEC 9075:1992 §7.5] The start of a join, taken: CROSS JOIN, or
    /// \[NATURAL\] \[`<join type>`\] JOIN, INNER if no type is written
    /// \[SR3\]; none if no join begins here.
    fn join_type(&mut self) -> Result<Option<(JoinKind, bool)>> {
        if self.accept_kw(kw::CROSS) {
            self.expect_kw(kw::JOIN)?;
            return Ok(Some((JoinKind::Cross, false)));
        }
        let natural = self.accept_kw(kw::NATURAL);
        let outer = [(kw::LEFT, JoinKind::Left), (kw::RIGHT, JoinKind::Right), (kw::FULL, JoinKind::Full)];
        let kind = if self.accept_kw(kw::INNER) {
            JoinKind::Inner
        } else if let Some((_, kind)) = outer.into_iter().find(|(w, _)| self.is_kw(w)) {
            self.advance();
            self.accept_kw(kw::OUTER);
            kind
        } else if self.is_kw(kw::UNION) && self.next_is_kw(kw::JOIN) {
            self.advance();
            JoinKind::Union
        } else if natural || self.is_kw(kw::JOIN) {
            JoinKind::Inner
        } else {
            return Ok(None);
        };
        self.expect_kw(kw::JOIN)?;
        Ok(Some((kind, natural)))
    }

    /// [ISO/IEC 9075:1992 §7.5] The `<join specification>` of a join of
    /// `kind`: none for CROSS or UNION, or NATURAL \[SR2a, SR2b\]; else ON
    /// `<search condition>` or USING (`<join column list>`) \[SR2c\].
    fn join_specification(&mut self, kind: JoinKind, natural: bool) -> Result<Option<JoinSpecification>> {
        let written = self.is_kw(kw::ON) || self.is_kw(kw::USING);
        if kind == JoinKind::Cross || kind == JoinKind::Union || natural {
            if written {
                return Err(self.rule("[ISO/IEC 9075:1992 §7.5 SR2a, SR2b] a CROSS, UNION or NATURAL join has no join specification"));
            }
            if kind == JoinKind::Union && natural {
                return Err(self.rule("[ISO/IEC 9075:1992 §7.5 SR2b] a UNION join is not NATURAL"));
            }
            return Ok(None);
        }
        if self.accept_kw(kw::ON) {
            return Ok(Some(JoinSpecification::On(self.search_condition()?)));
        }
        if !self.accept_kw(kw::USING) {
            return Err(self.rule("[ISO/IEC 9075:1992 §7.5 SR2c] the join needs ON or USING"));
        }
        Ok(Some(JoinSpecification::Using(self.name_list()?)))
    }

    /// A `<table reference>` but a joined table not in parentheses: a
    /// table, a `<derived table>`, or (`<joined table>`).
    fn table_primary(&mut self) -> Result<TableReference> {
        if self.at_subquery() {
            // <derived table> [AS] <correlation name> [(<derived column list>)]
            let query = Box::new(self.subquery()?);
            let correlation =
                self.correlation()?.ok_or_else(|| self.syntax("[ISO/IEC 9075:1992 §6.3 F] a <derived table> has a correlation name"))?;
            let columns = self.derived_columns()?;
            return Ok(TableReference::Derived(DerivedTable { query, correlation, columns }));
        }
        if self.accept_delim(delim::LEFT_PAREN) {
            let inner = self.table_reference()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            if !matches!(inner, TableReference::Joined(_)) {
                return Err(self.syntax("[ISO/IEC 9075:1992 §7.5 F] only a <joined table> is in parentheses"));
            }
            return Ok(inner);
        }
        let table = self.table_name()?;
        let correlation = self.correlation()?;
        let columns = if correlation.is_some() { self.derived_columns()? } else { None };
        Ok(TableReference::Table(NamedTable { table, correlation, columns }))
    }

    /// \[\[AS\] `<correlation name>`\], if written.
    fn correlation(&mut self) -> Result<Option<String>> {
        if self.accept_kw(kw::AS) {
            return self.identifier().map(Some);
        }
        Ok(match self.peek() {
            Tok::Identifier(_) => Some(self.identifier()?),
            Tok::KeyWord(_)
            | Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BitString(_)
            | Tok::Introducer(_)
            | Tok::Delimiter(_)
            | Tok::End => None,
        })
    }

    /// \[(`<derived column list>`)\], if written.
    fn derived_columns(&mut self) -> Result<Option<Vec<String>>> {
        if self.is_delim(delim::LEFT_PAREN) { self.name_list().map(Some) } else { Ok(None) }
    }
}

/// SELECT * FROM `from`: what TABLE `<table name>` and a `<joined table>`
/// are as a `<query primary>` [ISO/IEC 9075:1992 §7.10 SR2, SR13b].
fn all_of(from: TableReference) -> QueryExpression {
    let table_expression = TableExpression { from: vec![from], where_clause: None, group_by: Vec::new(), having: None };
    QueryExpression::Spec(Box::new(QuerySpecification { distinct: false, select_list: SelectList::Star, table_expression }))
}
