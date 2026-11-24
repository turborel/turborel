//! [ISO/IEC 9075-4:2006 draft Clause 14] The steps of a program executed:
//! compound statements, the control statements, and the statements a host
//! runs, each condition one raises offered to the handlers in scope where
//! it was raised (`conditions.rs`).
//!
//! A statement list runs its statements in turn. A statement that raises
//! a condition has it offered there, so a CONTINUE handler's action runs
//! and the statement after it follows [§14.2 GR3b i]; an EXIT or UNDO
//! handler's leaves the handler's compound statement as LEAVE leaves it. A
//! statement whose exception no handler handles ends each statement it is
//! in, unhandled; one that is atomic, a statement that may change data or
//! an ATOMIC compound statement, is undone first [ISO/IEC 9075-2:2023
//! §4.41.5; Part 4 §4.10.4].

use super::{Host, called, conditions, leaves};
use crate::ast::{FetchOrientation, HandlerType, SqlStatement};
use crate::db::{Completion, assign_targets};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::logical::{Block, Branches, Contained, Cursor, CursorAction, CursorStep, ForEach, Initial, Looped, Program, Repeating, Step};
use crate::params::{Params, local_key};
use crate::value::{DataType, Value, assign};

/// How a program ended.
#[derive(Debug)]
pub(crate) enum Ended {
    /// Its statement completed: with the last completion condition no
    /// handler handled, if one was raised.
    Completed(Completion),
    /// [ISO/IEC 9075-2:2023 §16.2 GR1] A function's RETURN, of the value.
    Returned(Value),
}

/// Where control goes once a statement has run.
pub(super) enum Flow {
    /// To the statement after it.
    Next,
    /// [§14.9] Out of the statement of the label numbered so.
    Leave(usize),
    /// [§14.8] To the next iteration of the statement of the label
    /// numbered so.
    Iterate(usize),
    /// Out of the function, its value returned.
    Return(Value),
}

/// An exception, as it leaves a statement.
pub(super) enum Stop {
    /// Raised by the statement itself: to be offered to the handlers in
    /// scope where it is.
    Raised(SqlError),
    /// Raised within, and handled by no handler in scope there.
    Unhandled(SqlError),
}

/// What running a statement gives.
type Ran = std::result::Result<(Flow, Completion), Stop>;

/// A compound statement being executed, and its mark, if it is ATOMIC.
pub(super) struct Scope<'p, M> {
    pub(super) block: &'p Block,
    pub(super) mark: Option<M>,
}

/// A program being run.
pub(super) struct Run<'p, 'h, H: Host> {
    pub(super) program: &'p Program,
    pub(super) host: &'h mut H,
    pub(super) frame: &'h mut Params,
    /// The compound statements being executed, the innermost last.
    pub(super) scopes: Vec<Scope<'p, H::Mark>>,
    /// The conditions the handlers being executed handle, the innermost
    /// last: RESIGNAL resignals the last [§17.3].
    pub(super) handling: Vec<SqlError>,
    /// How many ATOMIC compound statements, out of scope of the handlers
    /// being executed, are being executed all the same.
    hidden_atomic: usize,
    /// \[EXTENSION\] Its frame of the stack of the debugger attached, once
    /// one is (`debug.rs`).
    pub(super) tracked: Option<Box<crate::debugger::Tracked>>,
}

/// [ISO/IEC 9075-4:2006 draft Clause 14] Runs `program` over `frame` in
/// `host`.
///
/// # Errors
///
/// An exception no handler handles: of a condition declared for no
/// SQLSTATE value, SQLSTATE 45000 [§17.3 GR6b iii].
pub(crate) fn run_program<H: Host>(program: &Program, frame: &mut Params, host: &mut H) -> Result<Ended> {
    frame.set_execution(host.execution());
    let mut run = Run { program, host, frame, scopes: Vec::new(), handling: Vec::new(), hidden_atomic: 0, tracked: None };
    match run.list(std::slice::from_ref(&program.body)) {
        Ok((Flow::Return(v), _)) => Ok(Ended::Returned(v)),
        Ok((Flow::Next | Flow::Leave(_) | Flow::Iterate(_), completion)) => Ok(Ended::Completed(completion)),
        Err(Stop::Raised(e) | Stop::Unhandled(e)) => Err(conditions::unhandled(e)),
    }
}

impl<'p, H: Host> Run<'p, '_, H> {
    /// [§14.1 GR3] The statements `steps` run in turn, each condition one
    /// raises offered where it was raised: the flow of the first that does
    /// not go on to the next, and the last completion condition no handler
    /// handled.
    pub(super) fn list(&mut self, steps: &'p [Step]) -> Ran {
        let mut completed = Completion::Success;
        for step in steps {
            let flow = match self.statement(step) {
                Ok((flow, Completion::Success)) => flow,
                Ok((flow, completion)) => {
                    if let Some(handled) = self.offer(conditions::completion(completion))? {
                        handled
                    } else {
                        completed = completion;
                        flow
                    }
                }
                Err(Stop::Raised(e)) => match (self.offer(e.clone())?, conditions::completed(&e)) {
                    (Some(handled), _) => handled,
                    // [§17.3 GR6a iii NOTE 28] an unhandled completion
                    // condition: on without altering the flow of control.
                    (None, Some(completion)) => {
                        self.host.noted(&e);
                        completed = completion;
                        Flow::Next
                    }
                    (None, None) => return Err(Stop::Unhandled(e)),
                },
                Err(Stop::Unhandled(e)) => return Err(Stop::Unhandled(e)),
            };
            if !matches!(flow, Flow::Next) {
                return Ok((flow, completed));
            }
        }
        Ok((Flow::Next, completed))
    }

    /// The handler in scope for `condition` activated, if there is one,
    /// and the flow after it [§14.2 GR3]: none if there is none.
    fn offer(&mut self, condition: SqlError) -> std::result::Result<Option<Flow>, Stop> {
        let Some((k, handler)) = conditions::appropriate(&self.scopes, &condition) else { return Ok(None) };
        if handler.kind == HandlerType::Undo {
            // [§14.2 GR3b iii 2] the compound statement's changes undone.
            let marks: Vec<H::Mark> = self.scopes.iter_mut().skip(k).filter_map(|s| s.mark.take()).collect();
            if let Some(mark) = marks.into_iter().next() {
                self.host.undo(mark, &condition).map_err(Stop::Unhandled)?;
            }
        }
        let label = self.scopes.get(k).map(|s| s.block.label);
        self.host.push_area();
        self.handling.push(condition);
        // Its action is in scope of the handlers outside its compound
        // statement alone.
        let hidden = self.scopes.split_off(k);
        let atomic = hidden.iter().filter(|s| s.mark.is_some()).count();
        self.hidden_atomic = self.hidden_atomic.saturating_add(atomic);
        let ran = self.list(std::slice::from_ref(&handler.action));
        self.hidden_atomic = self.hidden_atomic.saturating_sub(atomic);
        self.scopes.extend(hidden);
        self.handling.pop();
        self.host.pop_area();
        match (ran?, handler.kind, label) {
            ((Flow::Next, _), HandlerType::Exit | HandlerType::Undo, Some(label)) => Ok(Some(Flow::Leave(label))),
            ((Flow::Next, _), HandlerType::Continue, _) | ((Flow::Next, _), HandlerType::Exit | HandlerType::Undo, None) => {
                Ok(Some(Flow::Next))
            }
            ((flow, _), _, _) => Ok(Some(flow)),
        }
    }

    /// The statement `step` run, the diagnostics area emptied as it begins
    /// and its condition placed there as it ends.
    fn statement(&mut self, step: &'p Step) -> Ran {
        // [EXTENSION] RULES.md D1015: one relaxed load of a static with no debugger.
        if crate::txn::debuggers_attached() {
            self.debugged(step)?;
        }
        self.host.begin(step);
        let ran = self.dispatch(step);
        let outcome = match &ran {
            Ok((_, completion)) => Ok(*completion),
            Err(Stop::Raised(e) | Stop::Unhandled(e)) => Err(e.clone()),
        };
        self.host.end(step, &outcome);
        ran
    }

    /// The statement `step` run.
    fn dispatch(&mut self, step: &'p Step) -> Ran {
        let next = |r: Result<Completion>| r.map(|c| (Flow::Next, c)).map_err(Stop::Raised);
        match step {
            Step::Block(b) => self.block(b),
            Step::Assign(a) => next(leaves::assignment(self.host, a, self.frame)),
            Step::If(b) => self.branches(b, false),
            Step::Case(b) => self.branches(b, true),
            Step::Loop(l) => self.looped(l),
            Step::For(f) => self.for_each(f),
            Step::Leave(label) => Ok((Flow::Leave(*label), Completion::Success)),
            Step::Iterate(label) => Ok((Flow::Iterate(*label), Completion::Success)),
            Step::Return(leaf) => {
                let value = leaf.as_ref().map_or(Ok(Value::Null), |l| leaves::value(self.host, l, self.frame)).map_err(Stop::Raised)?;
                Ok((Flow::Return(value), Completion::Success))
            }
            Step::Signal(s) => Err(Stop::Raised(self.signal(s)?)),
            Step::Into(i) => next(self.atomic(|run| leaves::select_into(run.host, i, run.frame))),
            Step::Cursor(c) => next(self.atomic(|run| run.cursor(c))),
            Step::Statement(c) => next(self.contained(c)),
            Step::Call(c) => next(called::call(self.host, c, self.frame)),
        }
    }

    /// The statement `c` the host runs: as an atomic statement, unless it
    /// is not one [ISO/IEC 9075-2:2023 §4.41.5].
    fn contained(&mut self, c: &Contained) -> Result<Completion> {
        // CALL is not atomic: each statement of the procedure is undone
        // alone. Nor is a savepoint's statement, ROLLBACK TO going back
        // past where a mark would be [§17.8 GR3d].
        if matches!(c.statement(), SqlStatement::Call(_) | SqlStatement::Savepoint(_)) {
            return self.host.run(c, self.frame);
        }
        // Nor are COMMIT and ROLLBACK, which end the transaction a mark
        // would be of.
        if matches!(c.statement(), SqlStatement::Commit | SqlStatement::Rollback) {
            return self.atomic_context().and_then(|()| self.host.run(c, self.frame));
        }
        self.atomic(|run| run.host.run(c, run.frame))
    }

    /// [ISO/IEC 9075-2:2023 §17.7 GR2, §17.8 GR2] That no atomic execution
    /// context is active, for COMMIT or ROLLBACK: none is in an ATOMIC
    /// compound statement [ISO/IEC 9075-4:2006 draft §14.1 SR6], but the
    /// action of a handler of a compound statement around one runs while
    /// it does.
    fn atomic_context(&self) -> Result<()> {
        if self.hidden_atomic == 0 && self.scopes.iter().all(|s| s.mark.is_none()) {
            return Ok(());
        }
        let msg = "[ISO/IEC 9075-2:2023 §17.7 GR2, §17.8 GR2] COMMIT or ROLLBACK while an ATOMIC compound statement is executed";
        Err(SqlError::new(Code::TransactionState, msg).with_state(SqlState::INVALID_TRANSACTION_TERMINATION))
    }

    /// `f` run as an atomic statement: undone if it fails [ISO/IEC
    /// 9075-2:2023 §4.41.5].
    fn atomic(&mut self, f: impl FnOnce(&mut Self) -> Result<Completion>) -> Result<Completion> {
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

    /// [§14.1] The compound statement `b`: ATOMIC, at a savepoint level of
    /// its own, established as it begins and destroyed as it ends [GR1,
    /// GR6].
    fn block(&mut self, b: &'p Block) -> Ran {
        if b.atomic {
            self.host.open_level(true);
        }
        let ran = self.block_within(b);
        if b.atomic {
            self.host.close_level();
        }
        ran
    }

    /// [§14.1] The compound statement `b`: its variables created, of their
    /// initial values [§14.4 GR1], its cursors declared, its statements
    /// run in scope of its handlers; its cursors destroyed after [GR4,
    /// GR5]. ATOMIC, it is undone if an exception ends it [GR1].
    fn block_within(&mut self, b: &'p Block) -> Ran {
        let mark = if b.atomic { self.host.mark().map_err(Stop::Raised)? } else { None };
        if let Err(e) = self.enter(b) {
            let e = match mark {
                Some(m) => self.host.undo(m, &e).err().unwrap_or(e),
                None => e,
            };
            return Err(Stop::Raised(e));
        }
        self.scopes.push(Scope { block: b, mark });
        let ran = self.list(&b.statements);
        let kept = self.scopes.pop().and_then(|s| s.mark);
        for c in &b.cursors {
            if let Ok(cursor) = self.cursor_of(*c) {
                self.host.destroy(&self.key(cursor));
            }
        }
        let ran = match (ran, kept) {
            (Err(Stop::Unhandled(e)), Some(m)) => Err(Stop::Unhandled(self.host.undo(m, &e).err().unwrap_or(e))),
            (ran, Some(m)) => {
                self.host.release(m);
                ran
            }
            (ran, None) => ran,
        };
        match ran? {
            (Flow::Leave(label), completion) if label == b.label => Ok((Flow::Next, completion)),
            done => Ok(done),
        }
    }

    /// [§14.4 GR1, §12.1] The variables of `b` created and its cursors
    /// declared.
    fn enter(&mut self, b: &'p Block) -> Result<()> {
        for v in &b.variables {
            let value = match &v.initial {
                Initial::Null => Value::Null,
                Initial::Empty => {
                    let none = std::sync::Arc::from(Vec::new());
                    if matches!(v.data_type.represented(), DataType::Multiset(_)) { Value::Multiset(none) } else { Value::Array(none) }
                }
                Initial::Value(leaf) => {
                    assign(leaves::value(self.host, leaf, self.frame)?, &v.data_type, "ISO/IEC 9075-4:2006 draft §14.4 GR1")?
                }
            };
            self.frame.set_slot(v.slot, value)?;
        }
        for c in &b.cursors {
            let cursor = self.cursor_of(*c)?;
            self.host.declare(&self.key(cursor), cursor)?;
        }
        Ok(())
    }

    /// The local cursor numbered `number`.
    fn cursor_of(&self, number: usize) -> Result<&'p Cursor> {
        self.program.cursors.get(number).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] the program has no cursor {number}")))
    }

    /// The name of `cursor` in this execution of the program.
    pub(super) fn key(&self, cursor: &Cursor) -> String {
        local_key(&cursor.key, self.frame.execution())
    }

    /// [§14.6, §14.7] IF, or a CASE statement if `case`: the statements of
    /// the first branch whose condition is true, else ELSE's; of CASE with
    /// none, SQLSTATE 20000 [§14.6 GR1b, GR2b].
    fn branches(&mut self, b: &'p Branches, case: bool) -> Ran {
        for (condition, steps) in &b.arms {
            if leaves::truth(self.host, condition, self.frame).map_err(Stop::Raised)? {
                return self.list(steps);
            }
        }
        match &b.otherwise {
            Some(steps) => self.list(steps),
            None if case => {
                let msg = "[ISO/IEC 9075-4:2006 draft §14.6 GR1b, GR2b] no WHEN of the CASE statement is true, and it has no ELSE";
                Err(Stop::Raised(SqlError::new(Code::Data, msg).with_state(SqlState::CASE_NOT_FOUND).or_at(b.at)))
            }
            None => Ok((Flow::Next, Completion::Success)),
        }
    }

    /// [§14.10 to §14.12] LOOP, WHILE or REPEAT `l`: WHILE's condition read
    /// before each iteration, REPEAT's after, ITERATE's too [§14.8 GR1].
    fn looped(&mut self, l: &'p Looped) -> Ran {
        let mut completed = Completion::Success;
        loop {
            self.host.interrupted().map_err(Stop::Unhandled)?;
            if let Repeating::While(c) = &l.kind
                && !leaves::truth(self.host, c, self.frame).map_err(Stop::Raised)?
            {
                return Ok((Flow::Next, completed));
            }
            match self.list(&l.statements)? {
                (Flow::Leave(label), c) if label == l.label => return Ok((Flow::Next, c)),
                (Flow::Next, c) => completed = c,
                (Flow::Iterate(label), _) if label == l.label => {}
                done => return Ok(done),
            }
            if let Repeating::Until(c) = &l.kind
                && leaves::truth(self.host, c, self.frame).map_err(Stop::Raised)?
            {
                return Ok((Flow::Next, completed));
            }
        }
    }

    /// [§14.13] FOR `f`: its cursor declared and opened, and for each row it
    /// fetches, its columns assigned to the variables and its statements
    /// run; the cursor destroyed after.
    fn for_each(&mut self, f: &'p ForEach) -> Ran {
        let cursor = self.cursor_of(f.cursor).map_err(Stop::Raised)?;
        let key = self.key(cursor);
        self.host.declare(&key, cursor).map_err(Stop::Raised)?;
        let ran = self.host.open(&key, cursor, self.frame).map_err(Stop::Raised).and_then(|()| self.rows_of(f, &key));
        self.host.destroy(&key);
        ran
    }

    /// The iterations of FOR `f`, its cursor `key` open.
    fn rows_of(&mut self, f: &'p ForEach, key: &str) -> Ran {
        let mut completed = Completion::Success;
        loop {
            self.host.interrupted().map_err(Stop::Unhandled)?;
            let Some(row) = self.host.fetch(key, &FetchOrientation::Next, self.frame).map_err(Stop::Raised)? else {
                return Ok((Flow::Next, completed));
            };
            for (slot, value) in f.slots.iter().zip(row) {
                self.frame.set_slot(*slot, value).map_err(Stop::Raised)?;
            }
            match self.list(&f.statements)? {
                (Flow::Leave(label), c) if label == f.label => return Ok((Flow::Next, c)),
                (Flow::Next, c) => completed = c,
                (Flow::Iterate(label), _) if label == f.label => {}
                done => return Ok(done),
            }
        }
    }

    /// [§12.2 to §12.4] OPEN, FETCH or CLOSE of a local cursor: FETCH past
    /// its rows no data, its targets not assigned.
    fn cursor(&mut self, c: &CursorStep) -> Result<Completion> {
        let cursor = self.cursor_of(c.cursor)?;
        let key = self.key(cursor);
        match &c.action {
            CursorAction::Open => self.host.open(&key, cursor, self.frame).map(|()| Completion::Success),
            CursorAction::Close => self.host.close(&key).map(|()| Completion::Success),
            CursorAction::Fetch(orientation, targets, scope, cast) => {
                let Some(mut row) = self.host.fetch(&key, orientation, self.frame)? else { return Ok(Completion::NoData) };
                if let Some(cast) = cast {
                    let outer = row.iter().map(|v| crate::logical::Expr::Const(v.clone())).collect();
                    let query = std::sync::Arc::new(crate::logical::Subquery::new(cast.query.plan.clone(), outer));
                    let filled = crate::logical::Leaf { query, at: cast.at };
                    row = self.host.rows(&filled, self.frame, Some(1))?.into_iter().next().unwrap_or(row);
                }
                self.frame.set_routine(std::sync::Arc::clone(scope));
                assign_targets(&row, targets, self.frame, "X3.135-1986 §8.6 GR8", "X3.135-1986 §8.6 GR10")
            }
        }
    }
}
