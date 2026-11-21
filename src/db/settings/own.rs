//! \[DESIGN\] A session's own values of the settings (RULES.md D36), and
//! the state its calls run against with them over the database's: made
//! once, and kept while neither the published state nor its values
//! change, so a session that set one pays no copy of the state per
//! statement.

use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Weak};

use super::super::{Session, State};
use crate::settings::Own;

/// Its own values, and the state overlaid with them, if made since they
/// last changed: with the published state it was made of, held weakly, so
/// a state published since is told apart however its memory is reused.
#[derive(Default)]
pub(in crate::db) struct OwnValues {
    values: Own,
    overlaid: Option<(Weak<State>, Arc<State>)>,
}

impl Deref for OwnValues {
    type Target = Own;
    fn deref(&self) -> &Own {
        &self.values
    }
}

/// A change of its values forgets the state overlaid with them.
impl DerefMut for OwnValues {
    fn deref_mut(&mut self) -> &mut Own {
        self.overlaid = None;
        &mut self.values
    }
}

/// `st` with the settings `own` over its own.
fn overlay(st: &State, own: &Own) -> Arc<State> {
    let mut copy = State::clone(st);
    copy.settings = st.settings.overlaid(own);
    Arc::new(copy)
}

impl Session {
    /// `st`, its settings this session's own over the database's, so its
    /// statements read them where they read the database's (RULES.md D36):
    /// a copy, made only if it set one, never published, and kept for its
    /// next statements while `st` and its values stay.
    pub(in crate::db) fn with_own(&self, st: Arc<State>) -> Arc<State> {
        let Ok(mut own) = self.own.try_borrow_mut() else { return overlay(&st, &self.own.borrow()) };
        if own.is_empty() {
            return st;
        }
        if let Some((base, made)) = &own.overlaid
            && std::ptr::eq(base.as_ptr(), Arc::as_ptr(&st))
        {
            return Arc::clone(made);
        }
        let made = overlay(&st, &own);
        own.overlaid = Some((Arc::downgrade(&st), Arc::clone(&made)));
        made
    }
}

#[cfg(test)]
mod tests;
