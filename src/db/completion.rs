//! How a statement that executed successfully completed. Split from db.rs
//! (RULES.md R-SIZE-4).

use crate::error::SqlState;

/// [X3.135-1986 §7.3 GR3a] The SQLCODE of a statement that found no data.
pub const SQLCODE_NO_DATA: i32 = 100;

/// How a statement that executed successfully completed: SQLCODE 0, or 100
/// when there was no data [X3.135-1986 §7.3 GR3a]; or, of SQL-92, with a
/// warning.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Completion {
    /// SQLCODE 0.
    #[default]
    Success,
    /// SQLCODE 100: no row was found to fetch, select, update or delete.
    NoData,
    /// \[EXTENSION\] [ISO/IEC 9075:1992 §22.1, §22.2] A completion
    /// condition of class 01, warning: SQLCODE 0, as Table 24 gives none
    /// other to a completion that is not "no data"; its SQLSTATE this.
    Warning(SqlState),
}

impl Completion {
    /// The SQLCODE value [X3.135-1986 §7.3 GR3a].
    pub fn sqlcode(self) -> i32 {
        match self {
            Completion::Success | Completion::Warning(_) => 0,
            Completion::NoData => SQLCODE_NO_DATA,
        }
    }

    /// \[EXTENSION\] The SQLSTATE value [ISO/IEC 9075:1992 §22.1].
    pub fn sqlstate(self) -> SqlState {
        match self {
            Completion::Success => SqlState::SUCCESSFUL_COMPLETION,
            Completion::NoData => SqlState::NO_DATA,
            Completion::Warning(state) => state,
        }
    }
}
