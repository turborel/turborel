//! [ISO/IEC 9075-4:2006 draft §14.5 to §14.13] The assignment, CASE, IF,
//! LOOP, WHILE, REPEAT and FOR statements.

use super::compound::{ASENSITIVE, SENSITIVE};
use super::{Block, BlockKind, DO, ELSEIF, IF, LOOP, REPEAT, UNTIL, WHILE};
use crate::ast::{
    Assignment, AssignmentSource, AssignmentTarget, CaseStatement, CaseWhen, ForStatement, IfStatement, Iterated, Iteration, Label,
};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

impl Parser {
    /// [§14.6] `<case statement>` ::= CASE \[`<case operand>`\] {WHEN ...
    /// THEN `<SQL statement list>`}... \[ELSE `<SQL statement list>`\] END
    /// CASE: simple, its `<when operand list>`s of one operand or more
    /// (Feature P008), or searched.
    pub(super) fn case_statement(&mut self) -> Result<CaseStatement> {
        let at = self.here();
        self.expect_kw(kw::CASE)?;
        let operand = if self.is_kw(kw::WHEN) { None } else { Some(self.case_operand()?) };
        let mut whens = Vec::new();
        while self.accept_kw(kw::WHEN) {
            let when = match &operand {
                Some(o) => {
                    let mut list = vec![self.when_operand(o)?];
                    while self.accept_delim(delim::COMMA) {
                        list.push(self.when_operand(o)?);
                    }
                    CaseWhen::Operands(list)
                }
                None => CaseWhen::Condition(self.search_condition()?),
            };
            self.expect_kw(kw::THEN)?;
            whens.push((when, self.statement_list(&[kw::WHEN, kw::ELSE, kw::END])?));
        }
        if whens.is_empty() {
            return Err(self.syntax("expected WHEN [ISO/IEC 9075-4:2006 draft §14.6]"));
        }
        let otherwise = if self.accept_kw(kw::ELSE) { Some(self.statement_list(&[kw::END])?) } else { None };
        self.expect_kw(kw::END)?;
        self.expect_kw(kw::CASE)?;
        Ok(CaseStatement { operand, whens, otherwise, at })
    }

    /// [§14.7] `<if statement>` ::= IF `<search condition>` THEN `<SQL
    /// statement list>` \[ELSEIF `<search condition>` THEN `<SQL statement
    /// list>`\]... \[ELSE `<SQL statement list>`\] END IF
    pub(super) fn if_statement(&mut self) -> Result<IfStatement> {
        let at = self.here();
        self.expect_word(IF)?;
        let mut branches = Vec::new();
        loop {
            let condition = self.search_condition()?;
            self.expect_kw(kw::THEN)?;
            branches.push((condition, self.statement_list(&[ELSEIF, kw::ELSE, kw::END])?));
            if !self.accept_word(ELSEIF) {
                break;
            }
        }
        let otherwise = if self.accept_kw(kw::ELSE) { Some(self.statement_list(&[kw::END])?) } else { None };
        self.expect_kw(kw::END)?;
        self.expect_word(IF)?;
        Ok(IfStatement { branches, otherwise, at })
    }

    /// [§14.10 to §14.12] `<loop statement>` ::= LOOP `<SQL statement
    /// list>` END LOOP, `<while statement>` ::= WHILE `<search condition>`
    /// DO `<SQL statement list>` END WHILE, or `<repeat statement>` ::=
    /// REPEAT `<SQL statement list>` UNTIL `<search condition>` END
    /// REPEAT; each \[`<beginning label>` `<colon>`\] before and
    /// \[`<ending label>`\] after, its label `label`.
    pub(super) fn iterated(&mut self, label: Option<Label>) -> Result<Iterated> {
        let at = self.here();
        let (word, condition) = if self.accept_word(LOOP) {
            (LOOP, None)
        } else if self.accept_word(WHILE) {
            let condition = self.search_condition()?;
            self.expect_word(DO)?;
            (WHILE, Some(condition))
        } else {
            self.expect_word(REPEAT)?;
            (REPEAT, None)
        };
        self.blocks.push(Block { kind: BlockKind::Iterated, ..Block::labelled(label.as_ref()) });
        let result = self.iterated_body(word);
        self.blocks.pop();
        let (statements, until) = result?;
        let iteration = match (word, condition, until) {
            (WHILE, Some(c), _) => Iteration::While(c),
            (_, _, Some(c)) => Iteration::Repeat(c),
            (_, _, None) => Iteration::Loop,
        };
        self.expect_kw(kw::END)?;
        self.expect_word(word)?;
        let ending = self.ending_label(label.as_ref())?;
        Ok(Iterated { label, iteration, statements, ending, at })
    }

    /// The `<SQL statement list>` of the iterated statement that `word`
    /// begins, and of REPEAT its UNTIL `<search condition>`.
    fn iterated_body(&mut self, word: &str) -> Result<(Vec<crate::ast::SqlStatement>, Option<crate::ast::SearchCondition>)> {
        if word != REPEAT {
            return Ok((self.statement_list(&[kw::END])?, None));
        }
        let statements = self.statement_list(&[UNTIL])?;
        self.expect_word(UNTIL)?;
        Ok((statements, Some(self.search_condition()?)))
    }

    /// [§14.13] `<for statement>` ::= \[`<beginning label>` `<colon>`\] FOR
    /// \[`<for loop variable name>` AS\] \[`<cursor name>` \[`<cursor
    /// sensitivity>`\] CURSOR FOR\] `<cursor specification>` DO `<SQL
    /// statement list>` END FOR \[`<ending label>`\], read as SR7 has it:
    /// a block of the loop variable name, whose variables are the columns
    /// and whose cursor is the FOR's, around a LOOP of the label `label`.
    pub(super) fn for_statement(&mut self, label: Option<Label>) -> Result<ForStatement> {
        let at = self.here();
        self.expect_kw(kw::FOR)?;
        let variable = if matches!(self.peek(), Tok::Identifier(_)) && *self.peek_at(1) == Tok::KeyWord(kw::AS) {
            let variable = self.label()?;
            self.advance();
            Some(variable)
        } else {
            None
        };
        let (cursor, insensitive) = if self.at_for_cursor() {
            let cursor = self.label()?;
            let insensitive = self.cursor_sensitivity();
            self.expect_kw(kw::CURSOR)?;
            self.expect_kw(kw::FOR)?;
            (Some(cursor), insensitive)
        } else {
            (None, false)
        };
        let start = self.here();
        let specification = self.cursor_specification()?;
        let text = self.text_from(start);
        self.expect_word(DO)?;
        let held = cursor.as_ref().map(|c| c.name.clone());
        let outer =
            Block { kind: BlockKind::ForVariable, cursors: held.iter().cloned().collect(), held, ..Block::labelled(variable.as_ref()) };
        self.blocks.push(outer);
        self.blocks.push(Block { kind: BlockKind::Iterated, ..Block::labelled(label.as_ref()) });
        let statements = self.statement_list(&[kw::END]);
        self.blocks.truncate(self.blocks.len().saturating_sub(2));
        let statements = statements?;
        self.expect_kw(kw::END)?;
        self.expect_kw(kw::FOR)?;
        let ending = self.ending_label(label.as_ref())?;
        Ok(ForStatement { label, variable, cursor, insensitive, specification, text, statements, ending, at })
    }

    /// Whether `<cursor name>` \[`<cursor sensitivity>`\] CURSOR is next.
    fn at_for_cursor(&self) -> bool {
        let sensitivity = matches!(self.peek_at(1), Tok::KeyWord(kw::INSENSITIVE))
            || matches!(self.peek_at(1), Tok::Identifier(w) if w == SENSITIVE || w == ASENSITIVE);
        let cursor = if sensitivity { self.peek_at(2) } else { self.peek_at(1) };
        matches!(self.peek(), Tok::Identifier(_)) && *cursor == Tok::KeyWord(kw::CURSOR)
    }

    /// [§14.5] `<assignment statement>` ::= SET `<assignment target>` =
    /// `<assignment source>` | SET ( `<assignment target>` \[{,
    /// `<assignment target>`}...\] ) = `<assigned row>` (Feature P006).
    pub(super) fn assignment(&mut self) -> Result<Assignment> {
        let at = self.here();
        self.expect_kw(kw::SET)?;
        if self.accept_delim(delim::LEFT_PAREN) {
            let mut targets = vec![self.assignment_target()?];
            while self.accept_delim(delim::COMMA) {
                targets.push(self.assignment_target()?);
            }
            self.expect_delim(delim::RIGHT_PAREN)?;
            self.expect_delim(delim::EQUALS)?;
            let row_at = self.here();
            let source = AssignmentSource::Row(self.row_value()?, row_at);
            return Ok(Assignment { targets, source, at });
        }
        let targets = vec![self.assignment_target()?];
        self.expect_delim(delim::EQUALS)?;
        let null_at = self.here();
        let source =
            if self.accept_kw(kw::NULL) { AssignmentSource::Null(null_at) } else { AssignmentSource::Value(self.value_expression()?) };
        Ok(Assignment { targets, source, at })
    }

    /// [§14.5] `<assignment target>` ::= `<target specification>` |
    /// `<modified field reference>`: a target, then each `<field name>`
    /// of a field of the row it holds (Feature T051).
    fn assignment_target(&mut self) -> Result<AssignmentTarget> {
        let target = self.target_specification()?;
        let mut fields = Vec::new();
        while self.accept_delim(delim::PERIOD) {
            fields.push(self.label()?);
        }
        Ok(AssignmentTarget { target, fields })
    }
}
