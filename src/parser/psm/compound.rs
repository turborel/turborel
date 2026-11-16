//! [ISO/IEC 9075-4:2006 draft §14.1 to §14.4, §12.1] `<compound
//! statement>`, and the declarations of its lists, in their order:
//! variables and conditions, cursors, then handlers.

use super::{ATOMIC, Block, BlockKind, DECLARED_FIRST, rule_at};
use crate::ast::{
    CompoundStatement, ConditionDeclaration, ConditionValue, HandlerDeclaration, HandlerType, Label, LocalCursor, LocalDeclaration,
    VariableDeclaration,
};
use crate::error::Result;
use crate::lexer::{Tok, delim, kw};
use crate::parser::Parser;

/// HANDLER, of a `<handler declaration>` [§14.2].
const HANDLER: &str = "HANDLER";
/// EXIT, a `<handler type>`.
const EXIT: &str = "EXIT";
/// UNDO, a `<handler type>`.
const UNDO: &str = "UNDO";
/// CONDITION, of a `<condition declaration>` [§14.3].
const CONDITION: &str = "CONDITION";
/// SQLEXCEPTION, a `<condition value>`.
const SQLEXCEPTION: &str = "SQLEXCEPTION";
/// SQLWARNING, a `<condition value>`.
const SQLWARNING: &str = "SQLWARNING";
/// SENSITIVE, a `<cursor sensitivity>`.
pub(super) const SENSITIVE: &str = "SENSITIVE";
/// ASENSITIVE, a `<cursor sensitivity>`.
pub(super) const ASENSITIVE: &str = "ASENSITIVE";

impl Parser {
    /// [ISO/IEC 9075-4:2006 draft §14.1] `<compound statement>` ::=
    /// \[`<beginning label>` `<colon>`\] BEGIN \[\[NOT\] ATOMIC\] \[`<local
    /// declaration list>`\] \[`<local cursor declaration list>`\]
    /// \[`<local handler declaration list>`\] \[`<SQL statement list>`\]
    /// END \[`<ending label>`\], its label `label`.
    pub(super) fn compound_statement(&mut self, label: Option<Label>) -> Result<CompoundStatement> {
        let at = self.here();
        self.expect_kw(kw::BEGIN)?;
        let atomic = if self.accept_kw(kw::NOT) {
            self.expect_word(ATOMIC)?;
            false
        } else {
            self.accept_word(ATOMIC)
        };
        self.blocks.push(Block { kind: if atomic { BlockKind::Atomic } else { BlockKind::Compound }, ..Block::labelled(label.as_ref()) });
        let result = self.compound_body(label, atomic, at);
        self.blocks.pop();
        result
    }

    /// What follows BEGIN \[\[NOT\] ATOMIC\] of a compound statement at
    /// `at` labelled `label`.
    fn compound_body(&mut self, label: Option<Label>, atomic: bool, at: usize) -> Result<CompoundStatement> {
        let mut declarations = Vec::new();
        while self.at_local_declaration() {
            declarations.push(self.local_declaration()?);
            self.expect_delim(delim::SEMICOLON)?;
        }
        let mut cursors = Vec::new();
        while self.at_local_cursor() {
            cursors.push(self.local_cursor_declaration()?);
            self.expect_delim(delim::SEMICOLON)?;
        }
        let mut handlers = Vec::new();
        while self.at_handler() {
            handlers.push(self.handler_declaration(atomic)?);
            self.expect_delim(delim::SEMICOLON)?;
        }
        if self.is_kw(kw::DECLARE) {
            return Err(self.syntax(DECLARED_FIRST));
        }
        let statements = if self.is_kw(kw::END) { Vec::new() } else { self.statement_list(&[kw::END])? };
        self.expect_kw(kw::END)?;
        let ending = self.ending_label(label.as_ref())?;
        Ok(CompoundStatement { label, atomic, declarations, cursors, handlers, statements, ending, at })
    }

    /// Whether DECLARE begins a `<handler declaration>` here.
    fn at_handler(&self) -> bool {
        let kind = matches!(self.peek_at(1), Tok::KeyWord(kw::CONTINUE))
            || matches!(self.peek_at(1), Tok::Identifier(w) if w == EXIT || w == UNDO);
        self.is_kw(kw::DECLARE) && kind && matches!(self.peek_at(2), Tok::Identifier(w) if w == HANDLER)
    }

    /// Whether DECLARE begins a `<declare cursor>` here.
    fn at_local_cursor(&self) -> bool {
        let cursor = |t: &Tok| {
            matches!(t, Tok::KeyWord(kw::INSENSITIVE | kw::SCROLL | kw::CURSOR))
                || matches!(t, Tok::Identifier(w) if w == SENSITIVE || w == ASENSITIVE)
        };
        self.is_kw(kw::DECLARE) && matches!(self.peek_at(1), Tok::Identifier(_)) && cursor(self.peek_at(2))
    }

    /// Whether DECLARE begins a `<local declaration>` here: of a variable
    /// or a condition.
    fn at_local_declaration(&self) -> bool {
        self.is_kw(kw::DECLARE) && matches!(self.peek_at(1), Tok::Identifier(_)) && !self.at_handler() && !self.at_local_cursor()
    }

    /// [§14.4] `<SQL variable declaration>` ::= DECLARE `<SQL variable name
    /// list>` `<data type>` \[`<default clause>`\], or [§14.3] `<condition
    /// declaration>` ::= DECLARE `<condition name>` CONDITION \[FOR
    /// `<sqlstate value>`\]: each name new to the block \[SR10, SR11\].
    fn local_declaration(&mut self) -> Result<LocalDeclaration> {
        self.expect_kw(kw::DECLARE)?;
        let mut names = vec![self.label()?];
        while self.accept_delim(delim::COMMA) {
            names.push(self.label()?);
        }
        if let ([name], true) = (names.as_slice(), self.accept_word(CONDITION)) {
            let sqlstate = if self.accept_kw(kw::FOR) { Some(self.sqlstate_value()?) } else { None };
            self.declared(name, |b| &mut b.conditions, "§14.3 SR2", "condition")?;
            return Ok(LocalDeclaration::Condition(ConditionDeclaration { name: name.clone(), sqlstate }));
        }
        let data_type = self.type_name()?;
        let default = if self.accept_kw(kw::DEFAULT) { Some(self.default_option()?) } else { None };
        for name in &names {
            self.declared(name, |b| &mut b.variables, "§14.1 SR10", "variable")?;
        }
        Ok(LocalDeclaration::Variable(VariableDeclaration { names, data_type, default }))
    }

    /// An `<identifier>`, and where it is written.
    pub(super) fn label(&mut self) -> Result<Label> {
        let at = self.here();
        Ok(Label { name: self.identifier()?, at })
    }

    /// Notes `name` among the names `list` gives of the innermost block,
    /// refused as `rule` says if it is there already: a `what`.
    fn declared(&mut self, name: &Label, list: impl Fn(&mut Block) -> &mut Vec<String>, rule: &str, what: &str) -> Result<()> {
        let Some(block) = self.blocks.last_mut() else { return Ok(()) };
        let names = list(block);
        if names.contains(&name.name) {
            let msg = format!("[ISO/IEC 9075-4:2006 draft {rule}] the compound statement declares the {what} {} twice", name.name);
            return Err(rule_at(name.at, msg));
        }
        names.push(name.name.clone());
        Ok(())
    }

    /// [§12.1] A `<declare cursor>` of a `<local cursor declaration
    /// list>`: DECLARE `<cursor name>` \[`<cursor sensitivity>`\]
    /// \[SCROLL\] CURSOR FOR `<cursor specification>`, its name new to the
    /// block \[SR12\].
    fn local_cursor_declaration(&mut self) -> Result<LocalCursor> {
        self.expect_kw(kw::DECLARE)?;
        let name = self.label()?;
        let insensitive = self.cursor_sensitivity();
        let scroll = self.accept_kw(kw::SCROLL);
        self.expect_kw(kw::CURSOR)?;
        self.expect_kw(kw::FOR)?;
        let start = self.here();
        let specification = self.cursor_specification()?;
        let text = self.text_from(start);
        self.declared(&name, |b| &mut b.cursors, "§14.1 SR12", "cursor")?;
        Ok(LocalCursor { name, insensitive, scroll, specification, text })
    }

    /// `<cursor sensitivity>` ::= SENSITIVE | INSENSITIVE | ASENSITIVE, if
    /// one is next: whether INSENSITIVE.
    pub(super) fn cursor_sensitivity(&mut self) -> bool {
        if self.accept_kw(kw::INSENSITIVE) {
            return true;
        }
        let _ = self.accept_word(SENSITIVE) || self.accept_word(ASENSITIVE);
        false
    }

    /// [§14.2] `<handler declaration>` ::= DECLARE `<handler type>` HANDLER
    /// FOR `<condition value list>` `<handler action>`, of a compound
    /// statement ATOMIC if `atomic`, which UNDO needs \[§14.1 SR17\]. The
    /// block's label is out of the action's scope \[§14.1 SR4\].
    fn handler_declaration(&mut self, atomic: bool) -> Result<HandlerDeclaration> {
        let at = self.here();
        self.expect_kw(kw::DECLARE)?;
        let kind = if self.accept_kw(kw::CONTINUE) {
            HandlerType::Continue
        } else if self.accept_word(EXIT) {
            HandlerType::Exit
        } else {
            self.expect_word(UNDO)?;
            if !atomic {
                return Err(rule_at(at, "[ISO/IEC 9075-4:2006 draft §14.1 SR17] an UNDO handler is of an ATOMIC compound statement"));
            }
            HandlerType::Undo
        };
        self.expect_word(HANDLER)?;
        self.expect_kw(kw::FOR)?;
        let mut conditions = vec![self.condition_value()?];
        while self.accept_delim(delim::COMMA) {
            conditions.push(self.condition_value()?);
        }
        if let Some(block) = self.blocks.last_mut() {
            block.hidden = true;
        }
        let action = self.body_statement();
        if let Some(block) = self.blocks.last_mut() {
            block.hidden = false;
        }
        Ok(HandlerDeclaration { kind, conditions, action: Box::new(action?), at })
    }

    /// [§14.2] `<condition value>` ::= `<sqlstate value>` | `<condition
    /// name>` | SQLEXCEPTION | SQLWARNING | NOT FOUND, and where it is.
    fn condition_value(&mut self) -> Result<(ConditionValue, usize)> {
        let at = self.here();
        let value = if self.is_kw(kw::SQLSTATE) {
            ConditionValue::Sqlstate(self.sqlstate_value()?.0)
        } else if self.accept_word(SQLEXCEPTION) {
            ConditionValue::Exception
        } else if self.accept_word(SQLWARNING) {
            ConditionValue::Warning
        } else if self.accept_kw(kw::NOT) {
            self.expect_kw(kw::FOUND)?;
            ConditionValue::NotFound
        } else {
            ConditionValue::Name(self.identifier()?)
        };
        Ok((value, at))
    }
}
