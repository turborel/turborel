//! \[EXTENSION\] The debugger attached to a session, hung on its `Stop`,
//! the one handle of a session another thread may hold (RULES.md D1005).
//!
//! A program looks at `link` as each of its statements begins. With no
//! debugger attached that is one relaxed load of a flag and one branch,
//! and nothing else: the lock behind it is taken only while one is.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::debugger::Link;
use crate::sync::lock_builtin;

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
        if !self.on.load(Ordering::Relaxed) {
            return None;
        }
        lock_builtin(&self.link).clone()
    }

    /// Whether a debugger is attached.
    pub(crate) fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// `link` attached; false if another is.
    pub(crate) fn attach(&self, link: Arc<Link>) -> bool {
        let mut held = lock_builtin(&self.link);
        if held.is_some() {
            return false;
        }
        *held = Some(link);
        self.on.store(true, Ordering::Release);
        true
    }

    /// `link` detached, if it is the one attached.
    pub(crate) fn detach(&self, link: &Arc<Link>) {
        let mut held = lock_builtin(&self.link);
        if held.as_ref().is_some_and(|h| Arc::ptr_eq(h, link)) {
            *held = None;
            self.on.store(false, Ordering::Release);
        }
        link.detach();
    }

    /// The session ended: its debugger told, and detached.
    pub(crate) fn end(&self) {
        let taken = lock_builtin(&self.link).take();
        self.on.store(false, Ordering::Release);
        if let Some(link) = taken {
            link.end();
        }
    }
}
