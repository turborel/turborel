//! \[DESIGN\] The clock and time zone of a statement's datetimes [ISO/IEC
//! 9075:1992 §4.5.1, §6.8]: the clock the database reads (`clock.rs`), and
//! the instant and displacement a statement is bound with.

use std::sync::Arc;

use super::{Database, State};
use crate::clock::Clock;
use crate::error::Result;
use crate::settings::{Own, TIME_ZONE};
use crate::value::Value;
use crate::value::datetime::{Now, parse_displacement, utc_of_unix};

impl Database {
    /// \[DESIGN\] Sets the clock CURRENT_DATE, CURRENT_TIME and
    /// CURRENT_TIMESTAMP read, for the statements that begin after it.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a lock is poisoned.
    pub fn set_clock(&self, clock: Arc<dyn Clock>) -> Result<()> {
        self.change(|st| {
            st.clock = clock;
            Ok(())
        })
    }
}

impl State {
    /// [ISO/IEC 9075:1992 §6.8 GR3, §4.5.1] What a statement begun now,
    /// under the settings `own` sets, is bound with: the clock's instant,
    /// and the time zone displacement TIME_ZONE gives.
    pub(crate) fn now(&self, own: &Own) -> Now {
        let zone = if let Value::Char(s) = self.settings.effective(TIME_ZONE, own).0 { parse_displacement(s).unwrap_or(0) } else { 0 };
        Now { utc: utc_of_unix(self.clock.now()), zone }
    }
}
