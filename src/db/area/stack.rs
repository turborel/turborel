//! [ISO/IEC 9075-2:2023 §23.2] The diagnostics area stack: a handler
//! pushes it as it is activated and pops it as its action completes
//! [ISO/IEC 9075-4:2006 draft §14.2 GR3b], so the condition it handles is
//! the stacked diagnostics area's, which GET STACKED DIAGNOSTICS reads
//! while the action's statements change the first (Feature P007).

use super::Area;
use crate::db::Session;
use crate::error::{Code, Result, SqlState, fail_with};

impl Area {
    /// [§23.2 GR2] PUSH: a copy of the first diagnostics area placed below
    /// it, the second.
    pub(in crate::db) fn push(&mut self) {
        let below = std::mem::take(&mut self.stacked);
        let copy = Area { stacked: Vec::new(), ..self.clone() };
        self.stacked = below;
        self.stacked.push(copy);
    }

    /// [§23.2 GR3] POP: the first diagnostics area taken away, the second
    /// the first; none if none was pushed.
    pub(in crate::db) fn pop(&mut self) {
        let mut below = std::mem::take(&mut self.stacked);
        if let Some(second) = below.pop() {
            *self = Area { stacked: below, ..second };
        } else {
            self.stacked = below;
        }
    }

    /// [ISO/IEC 9075-2:2023 §23.1 GR1] The diagnostics area GET
    /// DIAGNOSTICS reads: the stacked one if `stacked`, else the first.
    ///
    /// # Errors
    ///
    /// SQLSTATE 0Z002 of STACKED where no handler is active, as no area is
    /// stacked [§23.1 GR1b].
    pub(in crate::db) fn read(&self, stacked: bool) -> Result<Area> {
        if !stacked {
            return Ok(Area { stacked: Vec::new(), ..self.clone() });
        }
        match self.stacked.last() {
            Some(area) => Ok(area.clone()),
            None => fail_with(
                Code::Data,
                SqlState::STACKED_WITHOUT_HANDLER,
                "[ISO/IEC 9075-2:2023 §23.1 GR1b] GET STACKED DIAGNOSTICS where no handler is active".to_owned(),
            ),
        }
    }
}

impl Session {
    /// [§23.2 GR2] The session's diagnostics area stack pushed.
    pub(in crate::db) fn push_area(&self) {
        self.inner.borrow_mut().area.push();
    }

    /// [§23.2 GR3] The session's diagnostics area stack popped.
    pub(in crate::db) fn pop_area(&self) {
        self.inner.borrow_mut().area.pop();
    }

    /// The session's diagnostics area GET DIAGNOSTICS reads (`Area::read`).
    ///
    /// # Errors
    ///
    /// SQLSTATE 0Z002 of STACKED where no handler is active.
    pub(super) fn read_area(&self, stacked: bool) -> Result<Area> {
        self.inner.borrow().area.read(stacked)
    }
}
