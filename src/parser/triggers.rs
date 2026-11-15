//! The `<trigger definition>` [ISO/IEC 9075-2:2023 §11.49] and the `<drop
//! trigger statement>` [§11.50], with the Syntax Rules of §11.49 that need
//! no catalog. TRIGGER, BEFORE, AFTER, INSTEAD, REFERENCING, OLD, NEW, ROW,
//! EACH, STATEMENT and ATOMIC are no key words of SQL-92, so each is read
//! as an identifier, a key word only here.
//!
//! While a `<triggered action>` is read the parser knows it is in one
//! (`TriggerScope`). A statement then ends where its Format does, not at
//! the end of the text, no parameter is declared, and a `<target
//! specification>` may be a new transition variable column reference
//! [§6.4 SR7, §6.6 SR13].

use super::Parser;
use super::diagnostics::STATEMENT;
use crate::ast::{
    ActionTime, DropBehavior, DropObject, ParameterSpecification, Referencing, SearchCondition, SqlStatement, TriggerDefinition,
    TriggerEvent, TriggerLevel,
};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

// [ISO/IEC 9075-2:2023 §11.49, §11.50] The words of the Formats that are
// no key words of SQL-92.

/// TRIGGER, after CREATE or DROP.
pub(super) const TRIGGER: &str = "TRIGGER";
/// BEFORE, a `<trigger action time>`.
const BEFORE: &str = "BEFORE";
/// AFTER, a `<trigger action time>`.
const AFTER: &str = "AFTER";
/// INSTEAD, before OF in a `<trigger action time>`.
const INSTEAD: &str = "INSTEAD";
/// REFERENCING, before a `<transition table or variable list>`.
const REFERENCING: &str = "REFERENCING";
/// OLD, of a `<transition table or variable>`.
const OLD: &str = "OLD";
/// NEW, of a `<transition table or variable>`.
const NEW: &str = "NEW";
/// ROW, of a `<transition table or variable>` and of FOR EACH ROW.
const ROW: &str = "ROW";
/// EACH, of FOR EACH ROW and FOR EACH STATEMENT.
const EACH: &str = "EACH";
/// ATOMIC, after BEGIN in a `<compound triggered SQL statement>`.
const ATOMIC: &str = "ATOMIC";

/// What the parser knows of the `<triggered action>` it is reading.
#[derive(Debug)]
pub(super) struct TriggerScope {
    /// [ISO/IEC 9075-2:2023 §6.6 SR13] The `<new transition variable
    /// name>` of a BEFORE trigger, which qualifies a new transition
    /// variable column reference; none for any other trigger.
    new_row: Option<String>,
}

/// One kind of `<transition table or variable>`.
#[derive(Debug, Clone, Copy)]
enum Transition {
    /// OLD \[ROW\].
    OldRow,
    /// NEW \[ROW\].
    NewRow,
    /// OLD TABLE.
    OldTable,
    /// NEW TABLE.
    NewTable,
}

impl Transition {
    /// The kind as written, for messages.
    fn written(self) -> &'static str {
        match self {
            Transition::OldRow => "OLD ROW",
            Transition::NewRow => "NEW ROW",
            Transition::OldTable => "OLD TABLE",
            Transition::NewTable => "NEW TABLE",
        }
    }

    /// The place in `referencing` of the name of this kind.
    fn slot(self, referencing: &mut Referencing) -> &mut Option<(String, usize)> {
        match self {
            Transition::OldRow => &mut referencing.old_row,
            Transition::NewRow => &mut referencing.new_row,
            Transition::OldTable => &mut referencing.old_table,
            Transition::NewTable => &mut referencing.new_table,
        }
    }

    /// Whether it is OLD ROW or OLD TABLE.
    fn is_old(self) -> bool {
        match self {
            Transition::OldRow | Transition::OldTable => true,
            Transition::NewRow | Transition::NewTable => false,
        }
    }

    /// Whether it is OLD TABLE or NEW TABLE.
    fn is_table(self) -> bool {
        match self {
            Transition::OldTable | Transition::NewTable => true,
            Transition::OldRow | Transition::NewRow => false,
        }
    }

    /// [ISO/IEC 9075-2:2023 §11.49 SR7c] Whether a trigger on `event`
    /// may not name it: OLD of INSERT, NEW of DELETE.
    fn refused_by(self, event: &TriggerEvent) -> bool {
        match event {
            TriggerEvent::Insert => self.is_old(),
            TriggerEvent::Delete => !self.is_old(),
            TriggerEvent::Update(_) => false,
        }
    }
}

/// What a statement of a `<triggered action>` is, by its first words.
#[derive(Debug, Clone, Copy)]
enum ActionStatement {
    /// SELECT ... INTO or GET DIAGNOSTICS, which change no data.
    Reading,
    /// INSERT, UPDATE, DELETE or MERGE, which possibly modify SQL-data.
    Changing,
    /// TRUNCATE TABLE.
    Truncating,
    /// A statement of a kind §11.49 SR13 names, as the standard calls it.
    Refused(&'static str),
    /// A cursor statement: the rule its `<cursor name>` breaks and the
    /// statement's production.
    Cursor(&'static str, &'static str),
    /// No `<SQL procedure statement>`.
    Other,
}

/// [ISO/IEC 9075-2:2023 §11.49 SR13] The key words that begin an `<SQL
/// schema statement>`.
const SCHEMA_STATEMENTS: [&str; 5] = [kw::CREATE, kw::DROP, kw::ALTER, kw::GRANT, kw::REVOKE];
/// [ISO/IEC 9075-2:2023 §11.49 SR13] The key words that begin an `<SQL
/// dynamic statement>`.
const DYNAMIC_STATEMENTS: [&str; 5] = [kw::PREPARE, kw::DESCRIBE, kw::EXECUTE, kw::ALLOCATE, kw::DEALLOCATE];
/// The key words that begin a statement that possibly modifies SQL-data.
const CHANGING_STATEMENTS: [&str; 3] = [kw::INSERT, kw::UPDATE, kw::DELETE];

/// [ISO/IEC 9075-2:2023 §11.49 SR10] The refusal of a parameter or
/// variable in a `<triggered action>`, at `at`.
fn parameter_refused(at: usize) -> SqlError {
    SqlError::at(
        Code::SyntaxRule,
        at,
        "[ISO/IEC 9075-2:2023 §11.49 SR10] a <triggered action> shall not contain an <SQL parameter reference>, a <host parameter \
         name>, a <dynamic parameter specification> or an <embedded variable name>",
    )
}

/// [ISO/IEC 9075-2:2023 §11.49 SR14a] The refusal of a statement that
/// possibly modifies SQL-data in a BEFORE trigger.
const MODIFIES_BEFORE: &str = "[ISO/IEC 9075-2:2023 §11.49 SR14a] the <triggered action> of a BEFORE trigger shall not contain an \
                               SQL-statement that possibly modifies SQL-data, which turborel refuses (implementation-defined, IA047)";

/// [ISO/IEC 9075-2:2023 §11.49 SR13] The refusal of `what`, a kind of
/// statement no trigger may hold, at `at`.
fn kind_refused(what: &str, at: usize) -> SqlError {
    let msg = format!(
        "[ISO/IEC 9075-2:2023 §11.49 SR13] a <triggered SQL statement> shall not contain {what}, a check turborel makes as a Syntax \
         Rule (implementation-defined, IA046)"
    );
    SqlError::at(Code::SyntaxRule, at, msg)
}

/// The refusal of `what`, a cursor statement at `at` whose `<cursor name>`
/// breaks `rule`, since no cursor is declared in a `<triggered action>`'s
/// scope.
fn cursor_refused(rule: &str, what: &str, at: usize) -> SqlError {
    let msg = format!(
        "[ISO/IEC 9075-2:2023 {rule}] the <cursor name> of {what} shall be contained within the scope of a declared <cursor name>, \
         and a <triggered action> declares none"
    );
    SqlError::at(Code::SyntaxRule, at, msg)
}

/// What a statement begun by the identifier `word` is. MERGE and TRUNCATE
/// are the words that begin statements a trigger may hold.
fn word_statement_kind(word: &str) -> ActionStatement {
    if word == super::data_statements::MERGE {
        return ActionStatement::Changing;
    }
    if word == super::TRUNCATE {
        return ActionStatement::Truncating;
    }
    ActionStatement::Other
}

/// Whether `tok` begins a `<dynamic parameter specification>` or an
/// `<embedded variable name>`.
fn is_parameter_token(tok: &Tok) -> bool {
    *tok == Tok::Delimiter(delim::QUESTION) || *tok == Tok::Delimiter(delim::COLON)
}

impl Parser {
    /// Whether a `<trigger definition>` begins here: CREATE, then TRIGGER.
    pub(super) fn next_is_trigger(&self) -> bool {
        let Tok::Identifier(word) = self.peek_at(1) else { return false };
        word == TRIGGER
    }

    /// [ISO/IEC 9075-2:2023 §11.50] `<drop trigger statement>` ::= DROP
    /// TRIGGER `<trigger name>`, after DROP, if TRIGGER is next. It has no
    /// `<drop behavior>`, so RESTRICT.
    pub(super) fn drop_trigger(&mut self) -> Result<Option<SqlStatement>> {
        if !self.accept_word(TRIGGER) {
            return Ok(None);
        }
        Ok(Some(SqlStatement::Drop { object: DropObject::Trigger(self.table_name()?), behavior: DropBehavior::Restrict }))
    }

    /// [ISO/IEC 9075-2:2023 §11.49] `<trigger definition>` ::= CREATE
    /// TRIGGER `<trigger name>` `<trigger action time>` `<trigger event>`
    /// ON `<table name>` \[REFERENCING `<transition table or variable
    /// list>`\] `<triggered action>`
    pub(super) fn trigger_definition(&mut self) -> Result<TriggerDefinition> {
        let start = self.here();
        self.expect_kw(kw::CREATE)?;
        self.expect_word(TRIGGER)?;
        let name = self.table_name()?;
        let time = self.trigger_action_time()?;
        let event = self.trigger_event(time)?;
        self.expect_kw(kw::ON)?;
        let table = self.table_name()?;
        let referencing = if self.accept_word(REFERENCING) { self.transition_list(time, &event)? } else { Referencing::default() };
        let level = self.trigger_level(&referencing)?;
        let new_row = if time == ActionTime::Before { referencing.new_row.as_ref().map(|(new_row, _)| new_row.clone()) } else { None };
        let (when, action, action_text) = self.in_triggered_action(TriggerScope { new_row }, |p| p.triggered_action(time))?;
        let text = self.text_from(start);
        Ok(TriggerDefinition { name, time, event, table, referencing, level, when, action, action_text, text })
    }

    /// `<trigger action time>` ::= BEFORE | AFTER | INSTEAD OF
    fn trigger_action_time(&mut self) -> Result<ActionTime> {
        if self.accept_word(BEFORE) {
            return Ok(ActionTime::Before);
        }
        if self.accept_word(AFTER) {
            return Ok(ActionTime::After);
        }
        if !self.accept_word(INSTEAD) {
            return Err(self.syntax("expected BEFORE, AFTER or INSTEAD OF"));
        }
        self.expect_kw(kw::OF)?;
        Ok(ActionTime::InsteadOf)
    }

    /// `<trigger event>` ::= INSERT | DELETE | UPDATE \[OF `<trigger
    /// column list>`\], of a trigger at `time`.
    fn trigger_event(&mut self, time: ActionTime) -> Result<TriggerEvent> {
        if self.accept_kw(kw::INSERT) {
            return Ok(TriggerEvent::Insert);
        }
        if self.accept_kw(kw::DELETE) {
            return Ok(TriggerEvent::Delete);
        }
        if !self.accept_kw(kw::UPDATE) {
            return Err(self.syntax("expected INSERT, DELETE or UPDATE"));
        }
        let at = self.here();
        if !self.accept_kw(kw::OF) {
            return Ok(TriggerEvent::Update(Vec::new()));
        }
        if time == ActionTime::InsteadOf {
            return Err(SqlError::at(
                Code::SyntaxRule,
                at,
                "[ISO/IEC 9075-2:2023 §11.49 SR15b] an INSTEAD OF trigger has no <trigger column list>",
            ));
        }
        // <trigger column list> ::= <column name list>, no parentheses.
        let mut columns: Vec<(String, usize)> = Vec::new();
        loop {
            let column = self.column_name()?;
            if columns.iter().any(|(name, _)| *name == column.name) {
                let msg =
                    format!("[ISO/IEC 9075-2:2023 §11.49 SR6a] column {} appears more than once in the <trigger column list>", column.name);
                return Err(SqlError::at(Code::SyntaxRule, column.at, msg));
            }
            columns.push((column.name, column.at));
            if !self.accept_delim(delim::COMMA) {
                return Ok(TriggerEvent::Update(columns));
            }
        }
    }

    /// `<transition table or variable list>` ::= `<transition table or
    /// variable>`..., after REFERENCING, of a trigger at `time` on
    /// `event`.
    fn transition_list(&mut self, time: ActionTime, event: &TriggerEvent) -> Result<Referencing> {
        let mut referencing = Referencing::default();
        loop {
            let at = self.here();
            let kind = self.transition_kind()?;
            if kind.slot(&mut referencing).is_some() {
                let msg = format!("[ISO/IEC 9075-2:2023 §11.49 SR7b] {} is specified more than once", kind.written());
                return Err(SqlError::at(Code::SyntaxRule, at, msg));
            }
            Self::transition_allowed(kind, at, time, event)?;
            self.accept_kw(kw::AS);
            let named_at = self.here();
            let name = self.identifier()?;
            let names = [&referencing.old_row, &referencing.new_row, &referencing.old_table, &referencing.new_table];
            if names.into_iter().flatten().any(|(other, _)| *other == name) {
                let msg = format!("[ISO/IEC 9075-2:2023 §11.49 SR7d] transition name {name} is written twice");
                return Err(SqlError::at(Code::SyntaxRule, named_at, msg));
            }
            *kind.slot(&mut referencing) = Some((name, named_at));
            if !self.is_word(OLD) && !self.is_word(NEW) {
                return Ok(referencing);
            }
        }
    }

    /// OLD \[ROW\] | NEW \[ROW\] | OLD TABLE | NEW TABLE, before \[AS\] and
    /// a name.
    fn transition_kind(&mut self) -> Result<Transition> {
        let old = self.accept_word(OLD);
        if !old && !self.accept_word(NEW) {
            return Err(self.syntax("expected OLD or NEW"));
        }
        let table = self.accept_kw(kw::TABLE);
        if !table {
            self.accept_word(ROW);
        }
        Ok(match (old, table) {
            (true, false) => Transition::OldRow,
            (false, false) => Transition::NewRow,
            (true, true) => Transition::OldTable,
            (false, true) => Transition::NewTable,
        })
    }

    /// [ISO/IEC 9075-2:2023 §11.49 SR7c, SR14b] Refuses `kind`, written at
    /// `at`, of a trigger at `time` on `event` that may not name it.
    fn transition_allowed(kind: Transition, at: usize, time: ActionTime, event: &TriggerEvent) -> Result<()> {
        if kind.refused_by(event) {
            let of = if kind.is_old() { "INSERT" } else { "DELETE" };
            let msg = format!("[ISO/IEC 9075-2:2023 §11.49 SR7c] a trigger on {of} has no {}", kind.written());
            return Err(SqlError::at(Code::SyntaxRule, at, msg));
        }
        if time == ActionTime::Before && kind.is_table() {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.49 SR14b] a BEFORE trigger has no {}", kind.written());
            return Err(SqlError::at(Code::SyntaxRule, at, msg));
        }
        Ok(())
    }

    /// \[FOR EACH {ROW | STATEMENT}\], STATEMENT if not written \[SR8\], of
    /// a trigger naming `referencing`.
    fn trigger_level(&mut self, referencing: &Referencing) -> Result<TriggerLevel> {
        let level = if self.accept_kw(kw::FOR) {
            self.expect_word(EACH)?;
            if self.accept_word(ROW) {
                TriggerLevel::Row
            } else if self.accept_word(STATEMENT) {
                TriggerLevel::Statement
            } else {
                return Err(self.syntax("expected ROW or STATEMENT"));
            }
        } else {
            TriggerLevel::Statement
        };
        let row = referencing.old_row.as_ref().or(referencing.new_row.as_ref());
        if let (TriggerLevel::Statement, Some((name, at))) = (level, row) {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.49 SR9] transition variable {name} needs FOR EACH ROW");
            return Err(SqlError::at(Code::SyntaxRule, *at, msg));
        }
        Ok(level)
    }

    /// Runs `read` in a `<triggered action>` of `scope`, where no parameter
    /// is declared and none may be prepared \[SR10\], restoring both after.
    fn in_triggered_action<T>(&mut self, scope: TriggerScope, read: impl FnOnce(&mut Parser) -> Result<T>) -> Result<T> {
        let params = std::mem::take(&mut self.params);
        let dynamic = self.dynamic.take();
        self.trigger = Some(scope);
        let result = read(self);
        self.trigger = None;
        self.params = params;
        self.dynamic = dynamic;
        result.map_err(|e| self.parameter_refusal(e))
    }

    /// [ISO/IEC 9075-2:2023 §11.49 SR10] `e`, or the refusal of SR10 if `e`
    /// is the refusal of a `<dynamic parameter specification>` or an
    /// `<embedded variable name>`, which the statement parsers give where
    /// one is written.
    fn parameter_refusal(&self, e: SqlError) -> SqlError {
        let Some(at) = e.pos else { return e };
        if self.toks.iter().any(|t| t.start == at && is_parameter_token(&t.tok)) { parameter_refused(at) } else { e }
    }

    /// `<triggered action>` after FOR EACH: \[`<triggered when clause>`\]
    /// `<triggered SQL statement>`, of a trigger at `time`, with the
    /// latter's text.
    fn triggered_action(&mut self, time: ActionTime) -> Result<(Option<SearchCondition>, Vec<SqlStatement>, String)> {
        let when = if self.is_kw(kw::WHEN) {
            if time == ActionTime::InsteadOf {
                return Err(self.rule("[ISO/IEC 9075-2:2023 §11.49 SR15a] an INSTEAD OF trigger has no <triggered when clause>"));
            }
            self.advance();
            self.expect_delim(delim::LEFT_PAREN)?;
            let condition = self.search_condition()?;
            self.expect_delim(delim::RIGHT_PAREN)?;
            Some(condition)
        } else {
            None
        };
        let start = self.here();
        let action = if self.accept_kw(kw::BEGIN) {
            // <compound triggered SQL statement> ::= BEGIN ATOMIC {<SQL
            // procedure statement> <semicolon>}... END
            self.expect_word(ATOMIC)?;
            let mut action = Vec::new();
            loop {
                action.push(self.triggered_statement(time)?);
                self.expect_delim(delim::SEMICOLON)?;
                if self.accept_kw(kw::END) {
                    break action;
                }
            }
        } else {
            vec![self.triggered_statement(time)?]
        };
        Ok((when, action, self.text_from(start)))
    }

    /// One `<SQL procedure statement>` of a `<triggered action>` of a
    /// trigger at `time`, of the kinds a trigger may hold.
    fn triggered_statement(&mut self, time: ActionTime) -> Result<SqlStatement> {
        let at = self.here();
        let before = time == ActionTime::Before;
        match self.action_statement() {
            ActionStatement::Reading => self.cursorless_statement(),
            ActionStatement::Changing => {
                if before {
                    return Err(SqlError::at(Code::SyntaxRule, at, MODIFIES_BEFORE));
                }
                self.cursorless_statement()
            }
            ActionStatement::Truncating => {
                if before {
                    return Err(SqlError::at(Code::SyntaxRule, at, MODIFIES_BEFORE));
                }
                Err(self.unsupported("TRUNCATE TABLE in a <triggered action>"))
            }
            ActionStatement::Refused(what) => Err(kind_refused(what, at)),
            ActionStatement::Cursor(rule, what) => Err(cursor_refused(rule, what, at)),
            ActionStatement::Other => Err(self.syntax("expected an <SQL procedure statement> of a <triggered action>")),
        }
    }

    /// A statement read whole, refused if it names a cursor: a positioned
    /// UPDATE or DELETE, whose cursor no `<triggered action>` declares
    /// [ISO/IEC 9075-2:2023 §14.8 SR1, §14.13 SR1], or one of dynamic SQL
    /// [§11.49 SR13].
    fn cursorless_statement(&mut self) -> Result<SqlStatement> {
        let at = self.here();
        let statement = self.sql_statement()?;
        if let SqlStatement::UpdatePositioned { cursor: (_, cursor), .. } = &statement {
            return Err(cursor_refused("§14.13 SR1", "an <update statement: positioned>", *cursor));
        }
        if let SqlStatement::DeletePositioned { cursor: (_, cursor), .. } = &statement {
            return Err(cursor_refused("§14.8 SR1", "a <delete statement: positioned>", *cursor));
        }
        if let SqlStatement::Dynamic(_) = &statement {
            return Err(kind_refused("an <SQL dynamic statement>", at));
        }
        Ok(statement)
    }

    /// What the statement next is, by its first words.
    fn action_statement(&self) -> ActionStatement {
        match self.peek() {
            Tok::KeyWord(word) => self.key_word_statement(word),
            Tok::Identifier(word) => word_statement_kind(word),
            Tok::ExactNumeric(_)
            | Tok::ApproximateNumeric(_)
            | Tok::CharacterString(_)
            | Tok::BinaryString(_)
            | Tok::Introducer(_)
            | Tok::Delimiter(_)
            | Tok::End => ActionStatement::Other,
        }
    }

    /// What the statement next is, begun by the key word `word`.
    fn key_word_statement(&self, word: &str) -> ActionStatement {
        if CHANGING_STATEMENTS.contains(&word) {
            return ActionStatement::Changing;
        }
        if SCHEMA_STATEMENTS.contains(&word) {
            return ActionStatement::Refused("an <SQL schema statement>");
        }
        let dynamic = DYNAMIC_STATEMENTS.contains(&word) || (word == kw::GET && self.next_is_kw(kw::DESCRIPTOR));
        if dynamic {
            return ActionStatement::Refused("an <SQL dynamic statement>");
        }
        match word {
            kw::SELECT | kw::GET => ActionStatement::Reading,
            kw::COMMIT | kw::ROLLBACK => ActionStatement::Refused("an <SQL transaction statement>"),
            kw::CONNECT | kw::DISCONNECT => ActionStatement::Refused("an <SQL connection statement>"),
            kw::SET => self.set_statement_kind(),
            kw::OPEN => ActionStatement::Cursor("§14.4 SR1", "an <open statement>"),
            kw::FETCH => ActionStatement::Cursor("§14.5 SR3", "a <fetch statement>"),
            kw::CLOSE => ActionStatement::Cursor("§14.6 SR1", "a <close statement>"),
            _ => ActionStatement::Other,
        }
    }

    /// [ISO/IEC 9075-2:2023 §11.49 SR13] The kind of the statement SET
    /// begins: of a transaction, a connection, dynamic SQL or else the
    /// session.
    fn set_statement_kind(&self) -> ActionStatement {
        if self.next_is_kw(kw::TRANSACTION) || self.next_is_kw(kw::CONSTRAINTS) {
            return ActionStatement::Refused("an <SQL transaction statement>");
        }
        if self.next_is_kw(kw::CONNECTION) {
            return ActionStatement::Refused("an <SQL connection statement>");
        }
        if self.next_is_kw(kw::DESCRIPTOR) {
            return ActionStatement::Refused("an <SQL dynamic statement>");
        }
        ActionStatement::Refused("an <SQL session statement>")
    }

    /// [ISO/IEC 9075-2:2023 §6.4] A `<target specification>` or `<simple
    /// target specification>` in a `<triggered action>`, where the one
    /// kind it may be is a new transition variable column reference
    /// `N`.`C` of a BEFORE trigger's `<new transition variable name>` N
    /// [§6.4 SR7, §6.6 SR13]: C, marked as such.
    pub(super) fn transition_target(&mut self) -> Result<ParameterSpecification> {
        let at = self.here();
        if self.at_new_transition_column() {
            self.advance();
            self.advance();
            let name = self.identifier()?;
            return Ok(ParameterSpecification { name, indicator: None, at, transition: true });
        }
        let identifier = matches!(self.peek(), Tok::Identifier(_));
        if identifier && *self.peek_at(1) == Tok::Delimiter(delim::PERIOD) {
            return Err(self.rule(
                "[ISO/IEC 9075-2:2023 §6.4 SR7] a <target specification> that is a <column reference> shall be a new transition \
                 variable column reference, qualified by the <new transition variable name> of a BEFORE trigger [§6.6 SR13]",
            ));
        }
        if identifier || is_parameter_token(self.peek()) {
            return Err(parameter_refused(at));
        }
        Err(self.syntax("expected a <target specification>"))
    }

    /// Whether `N` . is next, N the new transition variable name of the
    /// BEFORE trigger whose action is being read.
    fn at_new_transition_column(&self) -> bool {
        let Some(TriggerScope { new_row: Some(new_row) }) = &self.trigger else { return false };
        let Tok::Identifier(word) = self.peek() else { return false };
        word == new_row && *self.peek_at(1) == Tok::Delimiter(delim::PERIOD)
    }
}

// Unit tests: each form of the statements parsed, and each Syntax Rule
// refused (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
