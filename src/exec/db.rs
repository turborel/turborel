//! \[DESIGN\] The database as an execution sees it: its state, and the
//! values of its statement's slots (`Expr::Dynamic`) if the plan it runs
//! is a kept one that reads them where they are (RULES.md D45).
//!
//! Every operator is opened with it, and hands it to those it opens and to
//! the runner of its subqueries, so a plan opened, or a subquery run,
//! after its statement ended, as a cursor's rows are pulled, reads the
//! values its own execution began with.

use std::borrow::Cow;
use std::ops::Deref;
use std::sync::Arc;

use crate::db::State;
use crate::error::Result;
use crate::index::Probe;
use crate::physical::{ProbeSlot, placed};
use crate::value::Value;

/// The values of a statement's slots, by place, shared by what its
/// execution opens.
pub type Slots = Arc<[Value]>;

/// The state a plan is executed in, and its statement's slot values.
#[derive(Clone)]
pub struct Db {
    state: Arc<State>,
    slots: Option<Slots>,
}

impl Db {
    /// `state`, for a plan with no slots.
    pub fn of(state: &Arc<State>) -> Db {
        Db { state: Arc::clone(state), slots: None }
    }

    /// `state`, for a kept plan whose slots hold `slots` (RULES.md D610).
    pub fn slotted(state: &Arc<State>, slots: Slots) -> Db {
        Db { state: Arc::clone(state), slots: Some(slots) }
    }

    /// Its statement's slots, executed in `state`, a copy of its own made
    /// for a part of the statement.
    #[must_use]
    pub fn in_state(&self, state: Arc<State>) -> Db {
        Db { state, slots: self.slots.clone() }
    }

    /// The state it is executed in.
    pub fn state(&self) -> &Arc<State> {
        &self.state
    }

    /// `probe`, each of `slots` placed with its slot's value: as it is if
    /// it has none (RULES.md D610).
    ///
    /// # Errors
    ///
    /// As `physical::placed`; SQLCODE -910 if it has no slot values.
    pub fn probe<'p>(&self, probe: &'p Probe, slots: &[ProbeSlot]) -> Result<Cow<'p, Probe>> {
        placed(probe, slots, self.slots.as_deref().unwrap_or_default())
    }

    /// The values of its statement's slots, if it has any.
    pub fn slots(&self) -> Option<&Slots> {
        self.slots.as_ref()
    }
}

impl Deref for Db {
    type Target = Arc<State>;

    fn deref(&self) -> &Arc<State> {
        &self.state
    }
}

impl From<Arc<State>> for Db {
    fn from(state: Arc<State>) -> Db {
        Db { state, slots: None }
    }
}

/// What a plan is opened in: an execution's `Db`, as it is, or a state
/// alone, for a plan with no slots.
pub trait Opening {
    /// The `Db` a plan is opened in.
    fn opening(&self) -> Cow<'_, Db>;
}

impl Opening for Db {
    fn opening(&self) -> Cow<'_, Db> {
        Cow::Borrowed(self)
    }
}

impl Opening for Arc<State> {
    fn opening(&self) -> Cow<'_, Db> {
        Cow::Owned(Db::of(self))
    }
}
