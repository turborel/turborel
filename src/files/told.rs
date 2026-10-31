//! \[DESIGN\] What the file manager tells the database's sink (RULES.md
//! D56): a failure no caller is left to tell, as a file nothing needs not
//! removed (`FileSystem::discard`); and FILES.MAX_OPEN too low for the
//! work, more files opened again in a minute than it lets be open, told
//! at most once a minute.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use super::Registry;
use crate::diagnostics::{Diagnostics, Event, NoDiagnostics};
use crate::error::SqlError;

/// \[DESIGN\] The span reopens are counted over, and the least between two
/// warnings. Fixed, not a setting (RULES.md D36): how often the
/// diagnostics hear of one trouble, not a pace of work; FILES.MAX_OPEN is
/// what an operator tunes.
const WINDOW: Duration = Duration::from_secs(60);

/// Where the registry tells, and its count of reopens.
pub(super) struct Teller {
    events: RwLock<Arc<dyn Diagnostics>>,
    window: Mutex<Window>,
}

impl Default for Teller {
    fn default() -> Teller {
        Teller { events: RwLock::new(Arc::new(NoDiagnostics)), window: Mutex::default() }
    }
}

/// The reopens of the span begun at `began`, and when one was last warned.
#[derive(Default)]
struct Window {
    began: Option<Instant>,
    reopened: u64,
    warned: Option<Instant>,
}

impl Registry {
    /// Tells `events` what it tells from now on.
    pub fn telling(&self, events: Arc<dyn Diagnostics>) {
        *self.teller.events.write().unwrap_or_else(PoisonError::into_inner) = events;
    }

    /// Tells `event`, no lock held.
    pub(super) fn tell(&self, event: &Event) {
        let events = Arc::clone(&self.teller.events.read().unwrap_or_else(PoisonError::into_inner));
        events.event(event);
    }

    /// Tells failure `e`, which no caller is left to tell.
    pub(super) fn tell_failed(&self, e: &SqlError) {
        self.tell(&Event::Failed(e.clone()));
    }

    /// Counts a file opened again, at `now`: past the limit in the span,
    /// warned once a span.
    pub(super) fn reopened(&self, now: Instant) {
        let limit = u64::try_from(self.limit.load(Ordering::Acquire)).unwrap_or(u64::MAX);
        let mut window = self.teller.window.lock().unwrap_or_else(PoisonError::into_inner);
        if window.began.is_none_or(|began| now.duration_since(began) >= WINDOW) {
            *window = Window { began: Some(now), reopened: 0, warned: window.warned };
        }
        window.reopened = window.reopened.saturating_add(1);
        let due = limit > 0 && window.reopened > limit && window.warned.is_none_or(|w| now.duration_since(w) >= WINDOW);
        if !due {
            return;
        }
        window.warned = Some(now);
        let reopened = window.reopened;
        drop(window);
        self.tell(&Event::FilesReopened { reopened, limit });
    }
}
