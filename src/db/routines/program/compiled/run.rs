//! \[DESIGN\] A compiled program run (RULES.md D841), with the semantics
//! the interpreter gives a program (`exec/program/run.rs`): each
//! statement's diagnostics area emptied as it begins and its condition
//! placed there as it ends; a condition a statement raises offered to the
//! handlers in scope where it was raised [ISO/IEC 9075-4:2006 draft
//! §14.2 GR1, GR3]; an atomic statement undone alone if it fails [ISO/IEC
//! 9075-2:2023 §4.41.5]; an ATOMIC compound statement at a savepoint
//! level of its own, undone if an exception ends it [§14.1 GR1, GR6].

use super::super::super::super::area::commands;
use super::super::super::super::{Completion, Session, StatementContext};
use super::super::super::kept::KeptProgram;
use super::super::Hosted;
use super::super::host::SessionHost;
use super::code::{Block, Code, Node};
use crate::ast::{HandlerType, SqlStatement};
use crate::error::{Code as ErrorCode, Result, SqlError, SqlState};
use crate::exec::{Host, appropriate_among, completed, completion};
use crate::logical::Handler;
use crate::params::Params;
use crate::txn::Mark;

/// Where control goes once a statement has run.
pub(super) enum Flow {
    Next,
    Leave(usize),
    Iterate(usize),
    Return(crate::value::Value),
}

/// An exception, as it leaves a statement: raised by it, to be offered to
/// the handlers in scope where it is, or raised within and handled by
/// none there.
pub(super) enum Stop {
    Raised(SqlError),
    Unhandled(SqlError),
}

/// What running a statement gives.
pub(super) type Ran = std::result::Result<(Flow, Completion), Stop>;

/// A compound statement being executed, and its mark, if it is ATOMIC.
pub(super) struct Scope<'a> {
    pub(super) block: &'a Block,
    pub(super) mark: Option<Mark>,
}

/// A compiled program being run in the statement of `cx`.
pub(super) struct Exec<'a> {
    pub(super) session: &'a Session,
    pub(super) cx: StatementContext<'a>,
    pub(super) kept: &'a KeptProgram,
    pub(super) host: SessionHost<'a, 'a>,
    /// What evaluates the rows of values reading nothing, one runner for
    /// the whole run.
    pub(super) hosted: Hosted<'a>,
    pub(super) frame: &'a mut Params,
    pub(super) scopes: Vec<Scope<'a>>,
    /// The conditions the handlers being executed handle, the innermost last.
    pub(super) handling: Vec<SqlError>,
    /// How many ATOMIC compound statements, out of scope of the handlers
    /// being executed, are being executed all the same.
    hidden_atomic: usize,
    /// Whether nothing of the program reads the diagnostics area
    /// (`unread`), so no statement's condition is placed there.
    pub(super) unread: bool,
}

/// \[DESIGN\] RULES.md D943. Whether nothing of `nodes` reads the
/// diagnostics area as it runs. No compound statement has a handler,
/// whose activation keeps the area on the stack, and each statement the
/// session runs changes or reads rows and no more, so none is GET
/// DIAGNOSTICS. The area of such a program is its own, discarded as its
/// procedure returns [ISO/IEC 9075-2:2023 §9.18 GR14], so what its
/// statements would place there is read by nothing.
pub(super) fn unread(nodes: &[Node]) -> bool {
    nodes.iter().all(|node| match &node.code {
        Code::Block(b) => b.handlers.is_empty() && unread(&b.statements),
        Code::If(b) | Code::Case(b) => b.arms.iter().all(|(_, arm)| unread(arm)) && b.otherwise.as_deref().is_none_or(unread),
        Code::Loop(l) => unread(&l.statements),
        Code::For(f) => unread(&f.statements),
        Code::Statement(s) => rows_alone(s.contained.statement()),
        Code::Assign(_) | Code::Leave(_) | Code::Iterate(_) | Code::Return(_) | Code::Signal(_) | Code::Into(_) | Code::Cursor(_) => true,
    })
}

/// Whether `stmt` is a statement that changes or reads rows and no more.
/// Any other, as a CALL, a dynamic statement or one of a savepoint, may
/// read the area or run what does.
fn rows_alone(stmt: &SqlStatement) -> bool {
    matches!(
        stmt,
        SqlStatement::Insert(_)
            | SqlStatement::Merge(_)
            | SqlStatement::UpdateSearched { .. }
            | SqlStatement::DeleteSearched { .. }
            | SqlStatement::UpdatePositioned { .. }
            | SqlStatement::DeletePositioned { .. }
            | SqlStatement::Select { .. }
            | SqlStatement::Open(_)
            | SqlStatement::Fetch { .. }
            | SqlStatement::Close(_)
    )
}

impl<'a> Exec<'a> {
    /// A run of `kept`'s compiled program by `session` in `cx`, over `frame`.
    pub(super) fn new(session: &'a Session, cx: StatementContext<'a>, kept: &'a KeptProgram, frame: &'a mut Params) -> Exec<'a> {
        let host = SessionHost::within(session, cx, Some(kept));
        let hosted = Hosted::new(Some(session), cx);
        Exec { session, cx, kept, host, hosted, frame, scopes: Vec::new(), handling: Vec::new(), hidden_atomic: 0, unread: false }
    }

    /// [§14.1 GR3] The statements `nodes` run in turn, each condition one
    /// raises offered where it was raised: the flow of the first that does
    /// not go on to the next, and the last completion condition no handler
    /// handled.
    pub(super) fn list(&mut self, nodes: &'a [Node]) -> Ran {
        let mut completed_with = Completion::Success;
        for node in nodes {
            let flow = match self.statement(node) {
                Ok((flow, Completion::Success)) => flow,
                Ok((flow, done)) => {
                    if let Some(handled) = self.offer(completion(done))? {
                        handled
                    } else {
                        completed_with = done;
                        flow
                    }
                }
                Err(Stop::Raised(e)) => match (self.offer(e.clone())?, completed(&e)) {
                    (Some(handled), _) => handled,
                    // [§17.3 GR6a iii NOTE 28] an unhandled completion
                    // condition: on without altering the flow of control.
                    (None, Some(done)) => {
                        self.host.noted(&e);
                        completed_with = done;
                        Flow::Next
                    }
                    (None, None) => return Err(Stop::Unhandled(e)),
                },
                Err(Stop::Unhandled(e)) => return Err(Stop::Unhandled(e)),
            };
            if !matches!(flow, Flow::Next) {
                return Ok((flow, completed_with));
            }
        }
        Ok((Flow::Next, completed_with))
    }

    /// The handler in scope for `condition` activated, if there is one,
    /// and the flow after it [§14.2 GR3]: none if there is none.
    fn offer(&mut self, condition: SqlError) -> std::result::Result<Option<Flow>, Stop> {
        let handlers: Vec<&[Handler]> = self.scopes.iter().map(|s| s.block.handlers.as_slice()).collect();
        let Some((k, h)) = appropriate_among(&handlers, &condition) else { return Ok(None) };
        let Some(block) = self.scopes.get(k).map(|s| s.block) else { return Ok(None) };
        let (Some(handler), Some(action)) = (block.handlers.get(h), block.actions.get(h)) else { return Ok(None) };
        if handler.kind == HandlerType::Undo {
            // [§14.2 GR3b iii 2] the compound statement's changes undone.
            let marks: Vec<Mark> = self.scopes.iter_mut().skip(k).filter_map(|s| s.mark.take()).collect();
            if let Some(mark) = marks.into_iter().next() {
                self.host.undo(mark, &condition).map_err(Stop::Unhandled)?;
            }
        }
        self.host.push_area();
        self.handling.push(condition);
        // Its action is in scope of the handlers outside its compound
        // statement alone.
        let hidden = self.scopes.split_off(k);
        let atomic = hidden.iter().filter(|s| s.mark.is_some()).count();
        self.hidden_atomic = self.hidden_atomic.saturating_add(atomic);
        let ran = self.list(std::slice::from_ref(action));
        self.hidden_atomic = self.hidden_atomic.saturating_sub(atomic);
        self.scopes.extend(hidden);
        self.handling.pop();
        self.host.pop_area();
        match (ran?, handler.kind) {
            ((Flow::Next, _), HandlerType::Exit | HandlerType::Undo) => Ok(Some(Flow::Leave(block.label))),
            ((Flow::Next, _), HandlerType::Continue) => Ok(Some(Flow::Next)),
            ((flow, _), _) => Ok(Some(flow)),
        }
    }

    /// The statement `node` run, the diagnostics area emptied as it
    /// begins and its condition placed there as it ends. Of a program
    /// nothing of which reads the area (`unread`), the warnings heard are
    /// emptied as it begins, as the area's emptying empties them, and the
    /// area is left as it is.
    fn statement(&mut self, node: &'a Node) -> Ran {
        if self.unread {
            self.session.unheard();
            return self.dispatch(&node.code);
        }
        if !node.reads_area {
            self.session.emptied((node.command, commands::UNRECOGNIZED));
        }
        let ran = self.dispatch(&node.code);
        if !node.reads_area {
            let outcome = match &ran {
                Ok((_, done)) => Ok(*done),
                Err(Stop::Raised(e) | Stop::Unhandled(e)) => Err(e.clone()),
            };
            let _ = self.session.diagnosed(outcome, |done| done.sqlstate());
        }
        ran
    }

    /// The statement `code` run.
    fn dispatch(&mut self, code: &'a Code) -> Ran {
        let next = |r: Result<Completion>| r.map(|c| (Flow::Next, c)).map_err(Stop::Raised);
        match code {
            Code::Block(b) => self.block(b),
            Code::Assign(a) => next(self.assignment(a)),
            Code::If(b) => self.branches(b, false),
            Code::Case(b) => self.branches(b, true),
            Code::Loop(l) => self.looped(l),
            Code::For(f) => self.for_each(f),
            Code::Leave(label) => Ok((Flow::Leave(*label), Completion::Success)),
            Code::Iterate(label) => Ok((Flow::Iterate(*label), Completion::Success)),
            Code::Return(v) => {
                let value = v.as_ref().map_or(Ok(crate::value::Value::Null), |v| self.value(v)).map_err(Stop::Raised)?;
                Ok((Flow::Return(value), Completion::Success))
            }
            Code::Signal(s) => Err(Stop::Raised(self.signal(s)?)),
            Code::Into(i) => next(self.atomic(|run| run.select_into(i))),
            Code::Cursor(c) => next(self.atomic(|run| run.cursor(c))),
            Code::Statement(s) => next(self.contained(s)),
        }
    }

    /// The statement `s` the session runs: as an atomic statement, unless
    /// it is not one [ISO/IEC 9075-2:2023 §4.41.5].
    fn contained(&mut self, s: &'a super::code::Statement) -> Result<Completion> {
        let stmt = s.contained.statement();
        // CALL is not atomic, nor is a savepoint's statement.
        if matches!(stmt, SqlStatement::Call(_) | SqlStatement::Savepoint(_)) {
            return self.host.run(&s.contained, self.frame);
        }
        // Nor are COMMIT and ROLLBACK, which end the transaction a mark
        // would be of.
        if matches!(stmt, SqlStatement::Commit | SqlStatement::Rollback) {
            return self.atomic_context().and_then(|()| self.host.run(&s.contained, self.frame));
        }
        self.atomic(|run| run.changed(s))
    }

    /// The statement `s` run: straight on its table if it is a change of
    /// a shape its kept plans allow, else as the session runs it, after
    /// which a change learns whether it may run straight from then on.
    fn changed(&mut self, s: &'a super::code::Statement) -> Result<Completion> {
        if let Some(change) = &s.change
            && let Some(done) = change.run(self, &s.contained)?
        {
            return Ok(done);
        }
        let ran = self.host.run(&s.contained, self.frame);
        if let (Ok(_), Some(change)) = (&ran, &s.change) {
            change.learn(self, &s.contained)?;
        }
        ran
    }

    /// [ISO/IEC 9075-2:2023 §17.7 GR2, §17.8 GR2] That no atomic execution
    /// context is active, for COMMIT or ROLLBACK.
    fn atomic_context(&self) -> Result<()> {
        if self.hidden_atomic == 0 && self.scopes.iter().all(|s| s.mark.is_none()) {
            return Ok(());
        }
        let msg = "[ISO/IEC 9075-2:2023 §17.7 GR2, §17.8 GR2] COMMIT or ROLLBACK while an ATOMIC compound statement is executed";
        Err(SqlError::new(ErrorCode::TransactionState, msg).with_state(SqlState::INVALID_TRANSACTION_TERMINATION))
    }

    /// `f` run as an atomic statement: undone if it fails [ISO/IEC
    /// 9075-2:2023 §4.41.5].
    pub(super) fn atomic(&mut self, f: impl FnOnce(&mut Self) -> Result<Completion>) -> Result<Completion> {
        let mark = self.host.mark()?;
        let ran = f(self);
        match (ran, mark) {
            (Ok(c), Some(m)) => {
                self.host.release(m);
                Ok(c)
            }
            (Err(e), Some(m)) => Err(self.host.undo(m, &e).err().unwrap_or(e)),
            (ran, None) => ran,
        }
    }
}
