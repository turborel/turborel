//! \[DESIGN\] The clock the datetime value functions read [ISO/IEC
//! 9075:1992 §6.8]: a plug-in of the database (`Database::set_clock`), so
//! a test gives one that stands still (RULES.md R-TEST-5) and a host one
//! of its own; by default the system's.

use std::time::{SystemTime, UNIX_EPOCH};

/// \[DESIGN\] The name of a clock that gives none of its own
/// (`Clock::name`): the group of no built-in setting, so what it declares
/// is never taken for the database's own (RULES.md D36).
pub const CLOCK_PLUGIN: &str = "clock_plugin";

/// A source of the current instant.
pub trait Clock: Send + Sync {
    /// Its name, for its settings: by default `CLOCK_PLUGIN`, which no
    /// built-in setting is under (RULES.md D36).
    fn name(&self) -> &'static str {
        CLOCK_PLUGIN
    }
    /// \[DESIGN\] Itself as it declares settings and is told them
    /// (`Configurable`, RULES.md D36), if it has any: declared as the
    /// database is given it (`Database::set_clock`). Default: none.
    fn configurable(&self) -> Option<&dyn crate::settings::Configurable> {
        None
    }
    /// Microseconds from 1970-01-01 00:00:00 UTC to now; before it,
    /// negative.
    fn now(&self) -> i64;
}

/// The operating system's clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        let micros = |d: std::time::Duration| i64::try_from(d.as_micros()).unwrap_or(i64::MAX);
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(after) => micros(after),
            Err(before) => micros(before.duration()).saturating_neg(),
        }
    }
}

/// A clock that stands still at `0`, microseconds from 1970-01-01
/// 00:00:00 UTC, or wherever it is set.
#[derive(Debug, Default)]
pub struct FixedClock(pub std::sync::atomic::AtomicI64);

impl FixedClock {
    /// A clock standing at `micros`.
    pub fn at(micros: i64) -> FixedClock {
        FixedClock(std::sync::atomic::AtomicI64::new(micros))
    }

    /// Sets it to `micros`.
    pub fn set(&self, micros: i64) {
        self.0.store(micros, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Clock for FixedClock {
    fn now(&self) -> i64 {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

// Unit tests: the system's clock and one standing still (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D54: the system's clock reads an instant
    /// after this code was written; a fixed one stands where it is set.
    #[test]
    fn clocks_read_instants() {
        assert!(SystemClock.now() > 1_700_000_000_000_000, "after November 2023");
        let fixed = FixedClock::at(5);
        assert_eq!(fixed.now(), 5);
        fixed.set(-7);
        assert_eq!(fixed.now(), -7);
        assert_eq!(FixedClock::default().now(), 0);
    }
}
