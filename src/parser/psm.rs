//! [ISO/IEC 9075-4:2006 draft §11.2, Clause 14, §17.2, §17.3] The SQL
//! control statements of SQL/PSM, and SIGNAL and RESIGNAL (Feature P002).
//!
//! The words of their Formats that are no key words of SQL-92, IF, LOOP,
//! WHILE, LEAVE and the others, are identifiers, each read as a word only
//! where a statement may begin. A control statement is read in an `<SQL
//! routine body>`, in a `<triggered action>`, or alone, as a directly
//! executed statement [§4.10.7], which is read as the body of a procedure
//! of no parameters, so its statements end where their Formats do.
//!
//! The parser keeps the control statements it is in (`Block`): their
//! labels, which LEAVE and ITERATE name [§14.8 SR1, §14.9 SR1], and the
//! SQL variables, conditions and cursors each declares, so that a `<target
//! specification>` that names a variable, or `L`.`V` of a label, is read
//! as one, and OPEN, FETCH and CLOSE of a declared cursor are allowed.
//! Whether a name is a variable, a parameter or a column is decided as the
//! statement is bound, where the columns are known.

mod compound;
mod control;
mod signal;

use super::Parser;
use super::routines::{RoutineScope, cursor_refused};
use crate::ast::{ParameterSpecification, RoutineKind, SqlStatement, TargetKind};
use crate::error::{Code, Result, SqlError};
use crate::lexer::{Tok, delim, kw};

/// IF, of an `<if statement>` [§14.7].
const IF: &str = "IF";
/// ELSEIF, of an `<if statement elseif clause>` [§14.7].
const ELSEIF: &str = "ELSEIF";
/// LOOP, of a `<loop statement>` [§14.10].
const LOOP: &str = "LOOP";
/// WHILE, of a `<while statement>` [§14.11].
const WHILE: &str = "WHILE";
/// REPEAT, of a `<repeat statement>` [§14.12].
const REPEAT: &str = "REPEAT";
/// UNTIL, of a `<repeat statement>` [§14.12].
const UNTIL: &str = "UNTIL";
/// DO, of a `<while statement>` and a `<for statement>`.
const DO: &str = "DO";
/// LEAVE, of a `<leave statement>` [§14.9].
const LEAVE: &str = "LEAVE";
/// ITERATE, of an `<iterate statement>` [§14.8].
const ITERATE: &str = "ITERATE";
/// SIGNAL, of a `<signal statement>` [§17.2].
/// Why a DECLARE is out of place: none is a statement, and a compound
/// statement's come first, in order.
pub(super) const DECLARED_FIRST: &str = "[ISO/IEC 9075-4:2006 draft §14.1] a compound statement declares its variables and conditions, then its \
                                         cursors, then its handlers, before its statements";
/// The tokens of a qualified SQL variable reference, `L`.`V`.
const QUALIFIED_TOKENS: usize = 3;
pub(crate) const SIGNAL: &str = "SIGNAL";
/// RESIGNAL, of a `<resignal statement>` [§17.3].
pub(crate) const RESIGNAL: &str = "RESIGNAL";
/// ATOMIC, of a `<compound statement>` [§14.1].
const ATOMIC: &str = "ATOMIC";

/// [ISO/IEC 9075-4:2006 draft §5.2] The `<reserved word>`s Part 4 adds,
/// which no `<correlation name>` written without AS is: so `FROM T DO`
/// ends a FOR statement's query.
const RESERVED: [&str; 12] = [DO, ELSEIF, "HANDLER", IF, ITERATE, LEAVE, LOOP, REPEAT, RESIGNAL, SIGNAL, UNTIL, WHILE];

impl Parser {
    /// Whether the next token is a `<reserved word>` Part 4 adds.
    pub(super) fn at_psm_reserved(&self) -> bool {
        RESERVED.iter().any(|w| self.is_word(w))
    }
}

/// A control statement being read, which encloses what is read now.
#[derive(Debug, Default)]
pub(super) struct Block {
    /// Its `<beginning label>`, if written: of a FOR, its `<for loop
    /// variable name>` here, and its label on the block within.
    label: Option<String>,
    /// What kind of statement it is.
    kind: BlockKind,
    /// Whether its label is out of scope, as its handlers are read [§14.1
    /// SR4].
    hidden: bool,
    /// The SQL variables it declares.
    variables: Vec<String>,
    /// The conditions it declares.
    conditions: Vec<String>,
    /// The cursors it declares.
    cursors: Vec<String>,
    /// A FOR's cursor, which no OPEN, FETCH or CLOSE in it names [§14.13
    /// SR10].
    held: Option<String>,
}

/// What kind of control statement a `Block` is.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlockKind {
    /// A compound statement NOT ATOMIC.
    #[default]
    Compound,
    /// A compound statement ATOMIC, where no COMMIT or ROLLBACK is \[SR6\].
    Atomic,
    /// An iterated statement, which ITERATE may name.
    Iterated,
    /// A FOR's `<for loop variable name>`, which LEAVE may not name
    /// [§14.13 SR8], and any column name of which is one of its variables,
    /// its columns known as it is bound [§14.13 SR7].
    ForVariable,
}

impl Block {
    /// Whether ITERATE may name its label.
    fn iterated(&self) -> bool {
        self.kind == BlockKind::Iterated
    }

    /// Whether LEAVE may name its label.
    fn leavable(&self) -> bool {
        self.kind != BlockKind::ForVariable
    }

    /// Whether any column name may be one of its variables.
    fn open(&self) -> bool {
        self.kind == BlockKind::ForVariable
    }
    /// A block labelled `label`, which LEAVE may name.
    fn labelled(label: Option<&crate::ast::Label>) -> Block {
        Block { label: label.map(|l| l.name.clone()), ..Block::default() }
    }
}

/// The cursor `statement` names, and whether it opens, fetches from or
/// closes it, rather than changing its row; none if it names none.
fn cursor_of(statement: &SqlStatement) -> Option<(&String, bool)> {
    if let SqlStatement::Open(c) | SqlStatement::Close(c) | SqlStatement::Fetch { cursor: c, .. } = statement {
        return Some((c, true));
    }
    if let SqlStatement::UpdatePositioned { cursor: (c, _), .. } | SqlStatement::DeletePositioned { cursor: (c, _), .. } = statement {
        return Some((c, false));
    }
    None
}

/// A Syntax Rule violation at `at`.
fn rule_at(at: usize, msg: impl Into<String>) -> SqlError {
    SqlError::at(Code::SyntaxRule, at, msg)
}

impl Parser {
    /// [ISO/IEC 9075-4:2006 draft §11.2, §4.10.7] The SQL control
    /// statement, SIGNAL or RESIGNAL next, if one is. Alone, it is read
    /// as the body of a procedure of no parameters; prepared, it is
    /// refused [§4.10.6].
    ///
    /// # Errors
    ///
    /// It is not one, or breaks a Syntax Rule.
    pub(super) fn control_statement(&mut self) -> Result<Option<SqlStatement>> {
        if !self.begins_control() {
            return Ok(None);
        }
        if self.trigger.is_some() || self.routine.is_some() {
            return self.nested(Parser::control).map(Some);
        }
        if self.dynamic.is_some() && !self.at_signal() {
            return Err(self.rule("[ISO/IEC 9075-4:2006 draft §4.10.6] an SQL control statement is not preparable"));
        }
        let scope = RoutineScope { kind: RoutineKind::Procedure, name: String::new(), parameters: Vec::new(), direct: true };
        let outer = self.routine.replace(scope);
        let result = self.nested(Parser::control);
        self.routine = outer;
        let statement = result?;
        self.end()?;
        Ok(Some(statement))
    }

    /// Whether SIGNAL or RESIGNAL is next.
    fn at_signal(&self) -> bool {
        self.is_word(SIGNAL) || self.is_word(RESIGNAL)
    }

    /// Whether a control statement begins here: a label, or its first
    /// word; SET only in a body, where it assigns a variable or parameter.
    fn begins_control(&self) -> bool {
        let labelled = matches!(self.peek(), Tok::Identifier(_)) && *self.peek_at(1) == Tok::Delimiter(delim::COLON);
        if labelled || self.at_signal() || self.is_kw(kw::BEGIN) || self.is_kw(kw::CASE) || self.is_kw(kw::FOR) {
            return true;
        }
        if self.is_kw(kw::SET) {
            return self.at_assignment();
        }
        [IF, LOOP, WHILE, REPEAT, LEAVE, ITERATE].iter().any(|w| self.is_word(w))
    }

    /// The control statement next, with its `<beginning label>` if one is
    /// written.
    fn control(&mut self) -> Result<SqlStatement> {
        use crate::ast::ControlStatement as C;
        let label = self.beginning_label()?;
        let labelled = |what: &str| match &label {
            Some(l) => Err(rule_at(l.at, format!("[ISO/IEC 9075-4:2006 draft §14.1] {what} has no <beginning label>"))),
            None => Ok(()),
        };
        let control = if self.is_kw(kw::BEGIN) {
            C::Compound(self.compound_statement(label)?)
        } else if self.is_kw(kw::FOR) {
            C::For(self.for_statement(label)?)
        } else if self.is_word(LOOP) || self.is_word(WHILE) || self.is_word(REPEAT) {
            C::Iterated(self.iterated(label)?)
        } else if self.at_signal() {
            labelled("a <signal statement>")?;
            return Ok(SqlStatement::Signal(Box::new(self.signal_statement()?)));
        } else {
            labelled("this statement")?;
            self.unlabelled()?
        };
        Ok(SqlStatement::Control(Box::new(control)))
    }

    /// The control statement next that takes no label: CASE, IF, LEAVE,
    /// ITERATE, or an assignment.
    fn unlabelled(&mut self) -> Result<crate::ast::ControlStatement> {
        use crate::ast::ControlStatement as C;
        if self.is_kw(kw::CASE) {
            return Ok(C::Case(self.case_statement()?));
        }
        if self.is_word(IF) {
            return Ok(C::If(self.if_statement()?));
        }
        if self.accept_word(LEAVE) {
            return Ok(C::Leave(self.left_label(false)?));
        }
        if self.accept_word(ITERATE) {
            return Ok(C::Iterate(self.left_label(true)?));
        }
        if self.is_kw(kw::SET) {
            return Ok(C::Assignment(self.assignment()?));
        }
        Err(self.syntax("expected an <SQL control statement> [ISO/IEC 9075-4:2006 draft §11.2]"))
    }

    /// `<beginning label>` `<colon>`, if one is next: not the label of a
    /// statement it is in [§14.1 SR4].
    fn beginning_label(&mut self) -> Result<Option<crate::ast::Label>> {
        let Tok::Identifier(name) = self.peek() else { return Ok(None) };
        if *self.peek_at(1) != Tok::Delimiter(delim::COLON) {
            return Ok(None);
        }
        let (name, at) = (name.clone(), self.here());
        self.advance();
        self.advance();
        if self.blocks.iter().any(|b| !b.hidden && b.label.as_deref() == Some(name.as_str())) {
            let msg = format!("[ISO/IEC 9075-4:2006 draft §14.1 SR4] {name} is the <beginning label> of a statement this one is in");
            return Err(rule_at(at, msg));
        }
        Ok(Some(crate::ast::Label { name, at }))
    }

    /// The `<ending label>` after END, if one is written: the beginning
    /// label `label` [§14.1 SR3, §14.10 to §14.13].
    fn ending_label(&mut self, label: Option<&crate::ast::Label>) -> Result<Option<crate::ast::Label>> {
        let Tok::Identifier(name) = self.peek() else { return Ok(None) };
        let (name, at) = (name.clone(), self.here());
        match label {
            Some(l) if l.name == name => {
                self.advance();
                Ok(Some(crate::ast::Label { name, at }))
            }
            Some(l) => Err(rule_at(at, format!("[ISO/IEC 9075-4:2006 draft §14.1 SR3] the <ending label> {name} is not {}", l.name))),
            None => Err(rule_at(at, format!("[ISO/IEC 9075-4:2006 draft §14.1 SR3] the <ending label> {name} has no <beginning label>"))),
        }
    }

    /// The `<statement label>` of LEAVE, or of ITERATE if `iterate`: of a
    /// statement it is in [§14.9 SR1], an iterated one for ITERATE [§14.8
    /// SR1], and not a FOR's `<for loop variable name>` [§14.13 SR8].
    fn left_label(&mut self, iterate: bool) -> Result<crate::ast::Label> {
        let at = self.here();
        let name = self.identifier()?;
        let block = self.blocks.iter().rev().find(|b| !b.hidden && b.label.as_deref() == Some(name.as_str()));
        let msg = match block {
            None if iterate => format!("[ISO/IEC 9075-4:2006 draft §14.8 SR1] ITERATE {name} names no iterated statement it is in"),
            None => format!("[ISO/IEC 9075-4:2006 draft §14.9 SR1] LEAVE {name} names no statement it is in"),
            Some(b) if iterate && !b.iterated() => {
                format!("[ISO/IEC 9075-4:2006 draft §14.8 SR1] ITERATE {name} names no iterated statement it is in")
            }
            Some(b) if !b.leavable() => {
                format!(
                    "[ISO/IEC 9075-4:2006 draft §14.13 SR8] the statements of a FOR shall not LEAVE its <for loop variable name> {name}"
                )
            }
            Some(_) => return Ok(crate::ast::Label { name, at }),
        };
        Err(rule_at(at, msg))
    }

    /// `<SQL statement list>` ::= {`<SQL procedure statement>`
    /// `<semicolon>`}..., ended by one of the words `ends`.
    fn statement_list(&mut self, ends: &[&str]) -> Result<Vec<SqlStatement>> {
        let mut list = Vec::new();
        loop {
            // END ends no statement of the list here: an empty list, or one
            // whose end word is missing [§14.6 to §14.13 Format].
            if self.is_kw(kw::END) {
                let wanted = if list.is_empty() { "an <SQL procedure statement>".to_owned() } else { ends.join(" or ") };
                return Err(self.syntax(format!("expected {wanted}")));
            }
            list.push(self.body_statement()?);
            self.expect_delim(delim::SEMICOLON)?;
            if ends.iter().any(|w| self.is_kw(w) || self.is_word(w)) {
                return Ok(list);
            }
        }
    }

    /// An `<SQL procedure statement>` within a control statement: of the
    /// kinds a triggered action may hold, in one; OPEN, FETCH, CLOSE and a
    /// positioned UPDATE or DELETE only of a cursor declared in a compound
    /// statement it is in [ISO/IEC 9075-4:2006 draft §12.1].
    pub(super) fn body_statement(&mut self) -> Result<SqlStatement> {
        if self.is_kw(kw::DECLARE) {
            return Err(self.syntax(DECLARED_FIRST));
        }
        if self.begins_control() {
            return self.nested(Parser::control);
        }
        // A triggered action's refuses them by its own rule [ISO/IEC
        // 9075-2:2023 §11.49 SR13].
        if (self.is_kw(kw::COMMIT) || self.is_kw(kw::ROLLBACK)) && self.trigger.is_none() {
            if let Some(refused) = self.transaction_end_refused() {
                return Err(refused);
            }
            return Ok(self.transaction_end());
        }
        let at = self.here();
        let cursor = self.is_kw(kw::OPEN) || self.is_kw(kw::FETCH) || self.is_kw(kw::CLOSE);
        let statement = if (cursor && self.blocks.iter().any(|b| !b.cursors.is_empty())) || (self.routine.is_some() && self.at_ptf_fetch())
        {
            self.sql_statement()?
        } else if let Some(t) = &self.trigger {
            let time = t.time;
            self.triggered_statement(time)?
        } else {
            if let Some(refused) = self.cursor_refusal() {
                return Err(refused);
            }
            self.sql_statement()?
        };
        self.statement_allowed(&statement, at)?;
        if self.routine.is_some() {
            Self::dynamic_allowed(&statement, at)?;
        }
        Ok(statement)
    }

    /// Why the COMMIT or ROLLBACK next is refused, if it is: none is in an
    /// ATOMIC compound statement [§14.1 SR6] or a FOR [§14.13 SR9], and a
    /// routine's body holds none (IA078) [ISO/IEC 9075-2:2023 §11.60
    /// SR23f]. An SQL control statement NOT ATOMIC run directly may hold
    /// one, which ends the transaction it runs in [§4.10.4].
    fn transaction_end_refused(&self) -> Option<SqlError> {
        let at = self.here();
        if self.blocks.iter().any(|b| b.kind == BlockKind::Atomic || b.held.is_some() || b.open()) {
            let msg = "[ISO/IEC 9075-4:2006 draft §14.1 SR6, §14.13 SR9] an ATOMIC compound statement or a FOR holds no COMMIT or \
                       ROLLBACK";
            return Some(rule_at(at, msg));
        }
        if self.routine.as_ref().is_some_and(|r| !r.direct) {
            return Some(rule_at(at, "[ISO/IEC 9075-2:2023 §11.60 SR23f] an SQL routine body holds no COMMIT or ROLLBACK (IA078)"));
        }
        None
    }

    /// That `statement`, at `at`, may be where it is: its cursor declared
    /// in scope.
    fn statement_allowed(&self, statement: &SqlStatement, at: usize) -> Result<()> {
        let Some((cursor, opens)) = cursor_of(statement) else { return Ok(()) };
        if !self.blocks.iter().any(|b| b.cursors.contains(cursor)) {
            if self.routine.is_some() || self.trigger.is_some() {
                return Err(cursor_refused("§14.4 SR1", &format!("a statement of cursor {cursor}"), at));
            }
            return Ok(());
        }
        if opens && self.blocks.iter().any(|b| b.held.as_ref() == Some(cursor)) {
            let msg = format!(
                "[ISO/IEC 9075-4:2006 draft §14.13 SR10] the statements of a FOR shall not OPEN, FETCH or CLOSE its cursor {cursor}"
            );
            return Err(rule_at(at, msg));
        }
        Ok(())
    }

    /// Whether SET next begins an `<assignment statement>`: in a body, of
    /// a target list, or of an SQL variable, an SQL parameter or a new
    /// transition variable's column.
    fn at_assignment(&self) -> bool {
        if self.routine.is_none() && self.trigger.is_none() {
            return false;
        }
        let Tok::Identifier(first) = self.peek_at(1) else { return *self.peek_at(1) == Tok::Delimiter(delim::LEFT_PAREN) };
        let period = *self.peek_at(2) == Tok::Delimiter(delim::PERIOD);
        let variable = self.blocks.iter().any(|b| b.open() || b.variables.contains(first) || (period && b.label.as_ref() == Some(first)));
        let parameter = self.routine.as_ref().is_some_and(|r| r.parameters.contains(first) || (period && r.name == *first));
        let new_row = self.trigger.as_ref().is_some_and(|t| period && t.new_row.as_ref() == Some(first));
        variable || parameter || new_row
    }

    /// [ISO/IEC 9075-4:2006 draft §6.2] The SQL variable reference next as
    /// a target, `V` or `L`.`V` of a label `L`, if one is: of a variable a
    /// statement it is in declares, or a FOR's column.
    pub(super) fn variable_target(&mut self) -> Option<ParameterSpecification> {
        let Tok::Identifier(first) = self.peek() else { return None };
        let at = self.here();
        let first = first.clone();
        if *self.peek_at(1) == Tok::Delimiter(delim::PERIOD)
            && let Tok::Identifier(second) = self.peek_at(2)
        {
            // A label is out of scope in its block's handlers [§14.1 SR4].
            let labelled = self.blocks.iter().rev().filter(|b| !b.hidden).find(|b| b.label.as_ref() == Some(&first));
            if labelled.is_some_and(|b| b.open() || b.variables.contains(second)) {
                let name = second.clone();
                (0..QUALIFIED_TOKENS).for_each(|_| self.advance());
                return Some(ParameterSpecification {
                    qualifier: Some(first),
                    ..ParameterSpecification::of(name, TargetKind::Routine, at)
                });
            }
        }
        if !self.blocks.iter().any(|b| b.open() || b.variables.contains(&first)) {
            return None;
        }
        self.advance();
        Some(ParameterSpecification::of(first, TargetKind::Routine, at))
    }
}

// Unit tests: each statement parsed, and each Syntax Rule refused
// (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
