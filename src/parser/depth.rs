//! \[DESIGN\] The depth of the syntax tree being built, bounded by
//! `MAX_NESTING_DEPTH` so that no statement's parse overflows the stack
//! (RULES.md R-CF-6).

use super::{MAX_NESTING_DEPTH, Parser};
use crate::error::{Code, Result, SqlError};

impl Parser {
    /// Parses one level deeper in the syntax tree. The depth is restored
    /// afterwards, also on failure, because `boolean_primary` backtracks.
    /// Written without combinators, whose frames each level would add to
    /// the stack (RULES.md R-CF-6).
    pub(super) fn nested<T>(&mut self, parse: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let saved = self.depth;
        // A failure deepens nothing, so the depth needs no restoring.
        self.deepen()?;
        let result = parse(self);
        self.depth = saved;
        result
    }

    /// Counts one more level of the tree being built; fails past `MAX_NESTING_DEPTH`.
    pub(super) fn deepen(&mut self) -> Result<()> {
        if self.depth >= MAX_NESTING_DEPTH {
            return Err(SqlError::at(
                Code::Unsupported,
                self.here(),
                format!("[DESIGN] the statement nests deeper than this implementation's limit of {MAX_NESTING_DEPTH} levels"),
            ));
        }
        self.depth = self.depth.saturating_add(1);
        Ok(())
    }
}
