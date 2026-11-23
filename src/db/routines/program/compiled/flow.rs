//! \[DESIGN\] The control statements of a compiled program run (RULES.md
//! D839): compound statements, IF and CASE, the iterated statements, FOR,
//! and the statements of local cursors, as the interpreter runs them
//! [ISO/IEC 9075-4:2006 draft Clause 14].

use std::sync::Arc;

use super::code::{Begun, Block, Branches, ForEach, Looped, Repeat};
use super::run::{Exec, Flow, Ran, Scope, Stop};
use crate::db::Completion;
use crate::error::{Code, Result, SqlError, SqlState};
use crate::exec::Host;
use crate::logical::{Cursor, CursorAction, CursorStep};
use crate::params::local_key;
use crate::value::{Value, assign};

impl<'a> Exec<'a> {
    /// [§14.1] The compound statement `b`: ATOMIC, at a savepoint level of
    /// its own, established as it begins and destroyed as it ends [GR1,
    /// GR6].
    pub(super) fn block(&mut self, b: &'a Block) -> Ran {
        if b.atomic {
            self.host.open_level(true);
        }
        let ran = self.block_within(b);
        if b.atomic {
            self.host.close_level();
        }
        ran
    }

    /// [§14.1] The compound statement `b`: its variables created [§14.4
    /// GR1], its cursors declared, its statements run in scope of its
    /// handlers; its cursors destroyed after [GR4, GR5]. ATOMIC, it is
    /// undone if an exception ends it [GR1].
    fn block_within(&mut self, b: &'a Block) -> Ran {
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
                let key = self.key(cursor);
                self.host.destroy(&key);
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
        Ok(ended(ran?, b.label))
    }

    /// [§14.4 GR1, §12.1] The variables of `b` created and its cursors
    /// declared.
    fn enter(&mut self, b: &'a Block) -> Result<()> {
        for v in &b.variables {
            let value = match &v.initial {
                Begun::Null => Value::Null,
                Begun::Empty(multiset) => {
                    let none = Arc::from(Vec::new());
                    if *multiset { Value::Multiset(none) } else { Value::Array(none) }
                }
                Begun::Value(initial) => assign(self.value(initial)?, &v.data_type, "ISO/IEC 9075-4:2006 draft §14.4 GR1")?,
            };
            self.frame.set_slot(v.slot, value)?;
        }
        for c in &b.cursors {
            let cursor = self.cursor_of(*c)?;
            let key = self.key(cursor);
            self.host.declare(&key, cursor)?;
        }
        Ok(())
    }

    /// The local cursor numbered `number`, as the kept program holds it.
    pub(super) fn cursor_of(&self, number: usize) -> Result<&'a Cursor> {
        let kept: &'a super::super::super::kept::KeptProgram = self.kept;
        kept.program.cursors.get(number).ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] the program has no cursor {number}")))
    }

    /// The name of `cursor` in this execution of the program.
    pub(super) fn key(&self, cursor: &Cursor) -> String {
        local_key(&cursor.key, self.frame.execution())
    }

    /// [§14.6, §14.7] IF, or a CASE statement if `case`: the statements of
    /// the first branch whose condition is true, else ELSE's; of CASE with
    /// none, SQLSTATE 20000 [§14.6 GR1b, GR2b].
    pub(super) fn branches(&mut self, b: &'a Branches, case: bool) -> Ran {
        for (condition, nodes) in &b.arms {
            if self.truth(condition).map_err(Stop::Raised)? {
                return self.list(nodes);
            }
        }
        if let Some(nodes) = &b.otherwise {
            return self.list(nodes);
        }
        if !case {
            return Ok((Flow::Next, Completion::Success));
        }
        let msg = "[ISO/IEC 9075-4:2006 draft §14.6 GR1b, GR2b] no WHEN of the CASE statement is true, and it has no ELSE";
        Err(Stop::Raised(SqlError::new(Code::Data, msg).with_state(SqlState::CASE_NOT_FOUND).or_at(b.at)))
    }

    /// [§14.10 to §14.12] LOOP, WHILE or REPEAT `l`: WHILE's condition read
    /// before each iteration, REPEAT's after, ITERATE's too [§14.8 GR1].
    pub(super) fn looped(&mut self, l: &'a Looped) -> Ran {
        let mut completed = Completion::Success;
        loop {
            self.host.interrupted().map_err(Stop::Unhandled)?;
            if let Repeat::While(c) = &l.kind
                && !self.truth(c).map_err(Stop::Raised)?
            {
                return Ok((Flow::Next, completed));
            }
            match iterated(self.list(&l.statements)?, l.label) {
                Ok(Some(c)) => completed = c,
                Ok(None) => {}
                Err(done) => return Ok(done),
            }
            if let Repeat::Until(c) = &l.kind
                && self.truth(c).map_err(Stop::Raised)?
            {
                return Ok((Flow::Next, completed));
            }
        }
    }

    /// [§14.13] FOR `f`: its cursor declared and opened, and for each row
    /// it fetches, its columns assigned to the variables and its
    /// statements run; the cursor destroyed after.
    pub(super) fn for_each(&mut self, f: &'a ForEach) -> Ran {
        let cursor = self.cursor_of(f.cursor).map_err(Stop::Raised)?;
        let key = self.key(cursor);
        self.host.declare(&key, cursor).map_err(Stop::Raised)?;
        let ran = self.host.open(&key, cursor, self.frame).map_err(Stop::Raised).and_then(|()| self.rows_of(f, &key));
        self.host.destroy(&key);
        ran
    }

    /// The iterations of FOR `f`, its cursor `key` open.
    fn rows_of(&mut self, f: &'a ForEach, key: &str) -> Ran {
        let mut completed = Completion::Success;
        loop {
            self.host.interrupted().map_err(Stop::Unhandled)?;
            let Some(row) = self.host.fetch(key, &f.next, self.frame).map_err(Stop::Raised)? else {
                return Ok((Flow::Next, completed));
            };
            for (slot, value) in f.slots.iter().zip(row) {
                self.frame.set_slot(*slot, value).map_err(Stop::Raised)?;
            }
            match iterated(self.list(&f.statements)?, f.label) {
                Ok(Some(c)) => completed = c,
                Ok(None) => {}
                Err(done) => return Ok(done),
            }
        }
    }

    /// [§12.2 to §12.4] OPEN, FETCH or CLOSE of a local cursor: FETCH past
    /// its rows no data, its targets not assigned.
    pub(super) fn cursor(&mut self, c: &CursorStep) -> Result<Completion> {
        let cursor = self.cursor_of(c.cursor)?;
        let key = self.key(cursor);
        match &c.action {
            CursorAction::Open => self.host.open(&key, cursor, self.frame).map(|()| Completion::Success),
            CursorAction::Close => self.host.close(&key).map(|()| Completion::Success),
            CursorAction::Fetch(orientation, targets, scope, cast) => {
                let Some(mut row) = self.host.fetch(&key, orientation, self.frame)? else { return Ok(Completion::NoData) };
                if let Some(cast) = cast {
                    let outer = row.iter().map(|v| crate::logical::Expr::Const(v.clone())).collect();
                    let query = Arc::new(crate::logical::Subquery::new(cast.query.plan.clone(), outer));
                    let filled = crate::logical::Leaf { query, at: cast.at };
                    row = self.host.rows(&filled, self.frame, Some(1))?.into_iter().next().unwrap_or(row);
                }
                self.frame.set_routine(Arc::clone(scope));
                crate::db::assign_targets(&row, targets, self.frame, "X3.135-1986 §8.6 GR8", "X3.135-1986 §8.6 GR10")
            }
        }
    }
}

/// What a statement labelled `label` gives, its statements having given
/// `ran`: LEAVE of it the next statement's flow.
fn ended(ran: (Flow, Completion), label: usize) -> (Flow, Completion) {
    match ran {
        (Flow::Leave(left), done) => (if left == label { Flow::Next } else { Flow::Leave(left) }, done),
        done @ (Flow::Next | Flow::Iterate(_) | Flow::Return(_), _) => done,
    }
}

/// Of an iteration of the statement labelled `label`, whose statements
/// gave `ran`: the completion to go on with, of the next iteration; none
/// if ITERATE of it began the next one; else, as an error, what ends the
/// iterated statement.
fn iterated(ran: (Flow, Completion), label: usize) -> std::result::Result<Option<Completion>, (Flow, Completion)> {
    match ran {
        (Flow::Next, c) => Ok(Some(c)),
        (Flow::Iterate(l), done) => {
            if l == label {
                Ok(None)
            } else {
                Err((Flow::Iterate(l), done))
            }
        }
        (Flow::Leave(l), done) => Err(if l == label { (Flow::Next, done) } else { (Flow::Leave(l), done) }),
        done @ (Flow::Return(_), _) => Err(done),
    }
}
