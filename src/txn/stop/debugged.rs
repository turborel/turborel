//! \[EXTENSION\] The debugger attached to a session, hung on its `Stop`,
//! the one handle of a session another thread may hold (RULES.md D1005).
//!
//! A program looks at `attached` as each of its statements begins: how
//! many debuggers the process has attached, a static. With none that is
//! one relaxed load and one branch, and nothing else, no session's state
//! reached and nothing made (RULES.md D1015). Else it looks at its own
//! session's flag, and the lock behind it is taken only while one is.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::debugger::Link;
use crate::sync::lock_builtin;

/// \[DESIGN\] How many debuggers are attached to sessions of the
/// process (RULES.md D1015).
static ATTACHED: AtomicUsize = AtomicUsize::new(0);

/// Whether any session of the process has a debugger attached: if not,
/// none is debugged, and a statement looks no further.
#[inline]
pub(crate) fn attached() -> bool {
    ATTACHED.load(Ordering::Relaxed) != 0
}

/// The debugger attached to a session, if one is.
#[derive(Debug, Default)]
pub(crate) struct Debugged {
    /// Whether one is: what a statement looks at first.
    on: AtomicBool,
    link: Mutex<Option<Arc<Link>>>,
}

impl Debugged {
    /// The link of the debugger attached, if one is.
    pub(crate) fn link(&self) -> Option<Arc<Link>> {
        if !self.on() {
            return None;
        }
        lock_builtin(&self.link).clone()
    }

    /// Whether a debugger is attached.
    #[inline]
    pub(crate) fn on(&self) -> bool {
        attached() && self.on.load(Ordering::Relaxed)
    }

    /// `link` attached; false if another is.
    pub(crate) fn attach(&self, link: Arc<Link>) -> bool {
        let mut held = lock_builtin(&self.link);
        if held.is_some() {
            return false;
        }
        *held = Some(link);
        ATTACHED.fetch_add(1, Ordering::Release);
        self.on.store(true, Ordering::Release);
        true
    }

    /// `link` detached, if it is the one attached.
    pub(crate) fn detach(&self, link: &Arc<Link>) {
        let mut held = lock_builtin(&self.link);
        if held.as_ref().is_some_and(|h| Arc::ptr_eq(h, link)) {
            *held = None;
            self.on.store(false, Ordering::Release);
            ATTACHED.fetch_sub(1, Ordering::Release);
        }
        link.detach();
    }

    /// The session ended: its debugger told, and detached.
    pub(crate) fn end(&self) {
        let taken = lock_builtin(&self.link).take();
        self.on.store(false, Ordering::Release);
        if let Some(link) = taken {
            ATTACHED.fetch_sub(1, Ordering::Release);
            link.end();
        }
    }
}
