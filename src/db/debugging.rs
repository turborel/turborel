//! \[EXTENSION\] A session's side of the routine debugger (RULES.md D1005
//! to D1012, `debugger.rs`): a session attaches to another by its
//! `DebugTarget`, sets and clears breakpoints in the routines and triggers
//! it may debug, tells the target, paused, how to go on, and waits for
//! what it hears of it.
//!
//! The plan this was built by is kept outside the repository, in
//! turborel-work/rv-debugger-plan/PLAN.md.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::Session;
use crate::catalog::{Catalog, QualifiedName, owner_of, sole};
use crate::debugger::{Action, Breakpoint, Event, Frame, Kind, Link, Paused, Reason, refused};
use crate::error::{Code, Result, SqlError};
use crate::txn::Stop;

/// A session a debugger may attach to: what another thread holds of it,
/// as it holds its `Stop` to cancel its statements (`Session::canceller`).
#[derive(Debug, Clone)]
pub struct DebugTarget(Arc<Stop>);

impl DebugTarget {
    /// The session whose `Stop` is `stop`, as a server holds it of each
    /// connection.
    pub fn of(stop: Arc<Stop>) -> DebugTarget {
        DebugTarget(stop)
    }

    /// The statement the session runs cancelled, as `Stop::cancel` does,
    /// paused or not.
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

/// A debugger attached to a target session: detached as it is dropped,
/// the target, if paused, going on.
#[derive(Debug)]
pub struct Debugger {
    link: Arc<Link>,
    target: Arc<Stop>,
}

impl Session {
    /// \[EXTENSION\] What a debugger attaches to this session by.
    pub fn debug_target(&self) -> DebugTarget {
        DebugTarget(Arc::clone(&self.stop))
    }

    /// \[EXTENSION\] A debugger of the session `target`, run as this
    /// session's current user: what frames it sees into, and where it may
    /// set breakpoints, are as that user's privileges are (RULES.md D1008).
    ///
    /// # Errors
    ///
    /// SQLSTATE TBD01 if `target` is this session, or has a debugger
    /// attached already.
    pub fn attach(&self, target: &DebugTarget) -> Result<Debugger> {
        if Arc::ptr_eq(&target.0, &self.stop) {
            return Err(refused("a session does not debug itself"));
        }
        let link = Arc::new(Link::new(&self.auth()));
        if !target.0.debugged().attach(Arc::clone(&link)) {
            return Err(refused("the session has a debugger attached already"));
        }
        Ok(Debugger { link, target: Arc::clone(&target.0) })
    }

    /// The routine of `kind` or the trigger `name`, its schema this
    /// session's default if not given: its specific name, or the trigger's,
    /// and its text, if this session's user may debug it.
    fn debugged_text(&self, kind: Kind, name: &QualifiedName) -> Result<(QualifiedName, String)> {
        let st = self.current_state()?;
        let catalog: &dyn Catalog = &*st.catalog;
        let auth = if name.auth.is_empty() { self.default_schema() } else { name.auth.clone() };
        let name = QualifiedName { auth, ident: name.ident.clone() };
        let user = self.auth().clone();
        let who = sole(catalog, &user);
        let no_debug = || SqlError::new(Code::Privilege, format!("[EXTENSION] {user} holds no DEBUG on {name}"));
        if kind == Kind::Trigger {
            let t = catalog.trigger(&name).ok_or_else(|| no_such(kind, &name))?;
            return if who.includes(owner_of(catalog, &name.auth)) { Ok((name, t.text.clone())) } else { Err(no_debug()) };
        }
        let routine = catalog.routine(&name).or_else(|| {
            let named = catalog.routines_named(&name);
            let of_kind: Vec<_> = named.into_iter().filter(|r| r.function() == (kind == Kind::Function)).collect();
            if let [one] = of_kind.as_slice() { Some(*one) } else { None }
        });
        let r = routine.filter(|r| r.function() == (kind == Kind::Function)).ok_or_else(|| no_such(kind, &name))?;
        if !r.debug_of(who, owner_of(catalog, &r.name.auth)).0 {
            return Err(no_debug());
        }
        Ok((r.specific.clone(), r.text.clone()))
    }
}

/// The refusal of a breakpoint in no routine or trigger (SQLCODE -103).
fn no_such(kind: Kind, name: &QualifiedName) -> SqlError {
    let what = match kind {
        Kind::Procedure => "procedure",
        Kind::Function => "function",
        Kind::Trigger => "trigger",
    };
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] there is no single {what} {name} to set a breakpoint in"))
}

/// The characters of line `line` of `text`, from 1: from where it begins
/// to where the next begins; none past its last line.
pub(crate) fn line_span(text: &str, line: u32) -> Option<(usize, usize)> {
    let mut begins = std::iter::once(0).chain(text.chars().enumerate().filter(|&(_, c)| c == '\n').map(|(k, _)| k.saturating_add(1)));
    let first = begins.nth(usize::try_from(line.checked_sub(1)?).ok()?)?;
    Some((first, begins.next().unwrap_or(usize::MAX)))
}

impl Debugger {
    /// \[EXTENSION\] A breakpoint at line `line`, from 1, of the text of
    /// the routine of `kind` named `name`, its specific name or, if it is
    /// the one routine of its kind of that name, its name, or of the
    /// trigger `name`: the target pauses at each statement that begins on
    /// that line, where `condition`, if given, a `<search condition>` over
    /// the frame's parameters and variables, is true (RULES.md D1009). The
    /// name's schema, if empty, is `session`'s default; its user, the one
    /// the debugger was attached as, may debug the routine.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no single routine of that name, or no
    /// such line; -104 if the user holds no DEBUG on it.
    pub fn break_at(
        &self,
        session: &Session,
        (kind, name): (Kind, &QualifiedName),
        line: u32,
        condition: Option<&str>,
    ) -> Result<Breakpoint> {
        let (specific, text) = session.debugged_text(kind, name)?;
        let span =
            line_span(&text, line).ok_or_else(|| SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {specific} has no line {line}")))?;
        let number = self.link.break_at(specific.clone(), (line, span), condition.map(str::to_owned))?;
        Ok(Breakpoint { number, routine: specific, line, span, condition: condition.map(str::to_owned) })
    }

    /// \[EXTENSION\] The breakpoint `number` cleared.
    ///
    /// # Errors
    ///
    /// SQLSTATE TBD01 if there is none of that number.
    pub fn clear(&self, number: u32) -> Result<()> {
        self.link.clear(number)
    }

    /// \[EXTENSION\] Its breakpoints, in the order they were set.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn breakpoints(&self) -> Result<Vec<Breakpoint>> {
        self.link.breakpoints()
    }

    /// \[EXTENSION\] The paused target told to go on as `action` says: to
    /// pause at its next statement, into a routine it invokes or not, or
    /// at the next of the frame it returns to, or to run to a breakpoint.
    ///
    /// # Errors
    ///
    /// SQLSTATE TBD01 if the target is not paused.
    pub fn resume(&self, action: Action) -> Result<()> {
        self.link.resume(action)
    }

    /// \[EXTENSION\] The running target told to pause at the next
    /// statement it runs of a routine the debugger may debug.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn interrupt(&self) -> Result<()> {
        self.link.interrupt()
    }

    /// \[EXTENSION\] The target's pause, with its call stack, if it is
    /// paused.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn paused(&self) -> Result<Option<Paused>> {
        self.link.paused()
    }

    /// \[EXTENSION\] The next event the debugger has not read of its
    /// target: waited for at most `most`, none if none came. A pause at a
    /// breakpoint whose condition is not true of the paused frame, its
    /// condition evaluated in `session` (RULES.md D1009), is no event: the
    /// target is told to go on, and the wait goes on.
    ///
    /// # Errors
    ///
    /// A condition's evaluation's; SQLCODE -910 if a lock is poisoned.
    pub fn wait(&self, session: &Session, most: Duration) -> Result<Option<Event>> {
        let began = Instant::now();
        loop {
            let Some(event) = self.link.next_event(most.saturating_sub(began.elapsed()))? else { return Ok(None) };
            let Event::Paused(Paused { reason: Reason::Breakpoint(number), frames }) = &event else { return Ok(Some(event)) };
            let breakpoints = self.link.breakpoints()?;
            let condition = breakpoints.iter().find(|b| b.number == *number).and_then(|b| b.condition.as_deref());
            let (Some(condition), Some(frame)) = (condition, frames.last()) else { return Ok(Some(event)) };
            if holds(session, frame, condition)? {
                return Ok(Some(event));
            }
            self.link.resume(Action::Continue)?;
        }
    }

    /// \[EXTENSION\] Whether the target session has ended.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn ended(&self) -> Result<bool> {
        self.link.ended()
    }

    /// \[EXTENSION\] Detached: its breakpoints gone, the target, if
    /// paused, going on as if never debugged.
    pub fn detach(self) {
        drop(self);
    }
}

/// \[EXTENSION\] Whether `condition` is true of the parameters and
/// variables of `frame`: evaluated in `session` as the search condition of
/// a query of one row, the frame's values, its columns named as they are
/// (RULES.md D1009).
fn holds(session: &Session, frame: &Frame, condition: &str) -> Result<bool> {
    let named: Vec<_> = frame.parameters.iter().chain(&frame.variables).collect();
    let (casts, columns): (Vec<_>, Vec<_>) = if named.is_empty() {
        (vec!["0".to_owned()], vec!["\"(NONE)\"".to_owned()])
    } else {
        named.iter().map(|n| (format!("CAST(? AS {})", n.data_type), format!("\"{}\"", n.name.replace('"', "\"\"")))).unzip()
    };
    let query = format!("SELECT 1 FROM (VALUES ({})) AS FRAME ({}) WHERE {condition}", casts.join(", "), columns.join(", "));
    let prepared = session.prepare(&query)?;
    let values: Vec<_> = named.iter().map(|n| n.value.clone()).collect();
    let mut rows = session.open_prepared(&prepared, &values)?;
    Ok(rows.fetch()?.is_some())
}

impl Drop for Debugger {
    fn drop(&mut self) {
        self.target.debugged().detach(&self.link);
    }
}
