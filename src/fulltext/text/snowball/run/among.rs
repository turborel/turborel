//! \[DESIGN\] `substring` and `among` of a Snowball program (RULES.md
//! D320): the longest of an among's strings at the cursor whose routine,
//! if it names one, succeeds after it, as the runtime's `find_among` and
//! `find_among_b` find it (`python/snowballstemmer/basestemmer.py`); then
//! the command that string chooses.
//!
//! The strings at the cursor are each a prefix of the longest, backward a
//! suffix, so trying them longest first is trying the runtime's chain of
//! shorter strings. Each length is looked up once in the among's index.

use super::{Halt, Machine, Signal};

impl<'p> Machine<'p> {
    /// The command chosen by the longest string of the among `among` at
    /// the cursor whose routine succeeds, if any matches; the cursor
    /// moved past the string.
    fn find(&mut self, among: usize) -> Result<Option<Option<usize>>, Halt> {
        let program = self.program;
        let among = program.amongs.get(among).ok_or(Halt)?;
        let c = self.env.c;
        for &length in &among.lengths {
            let to = if self.backward {
                c.checked_sub(length).filter(|start| *start >= self.env.lb)
            } else {
                c.checked_add(length).filter(|end| *end <= self.env.l)
            };
            let Some(to) = to else { continue };
            let key = if self.backward { self.env.s.get(to..c) } else { self.env.s.get(c..to) };
            let Some(entry) = key.and_then(|key| among.index.get(key)).and_then(|place| among.entries.get(*place)) else { continue };
            self.env.c = to;
            if let Some(routine) = entry.routine {
                if !self.call(routine)? {
                    continue;
                }
                self.env.c = to;
            }
            return Ok(Some(entry.action));
        }
        Ok(None)
    }

    /// `substring`: the among's string found, failing if none is.
    pub(super) fn substring(&mut self, among: usize) -> Signal {
        let found = self.find(among)?;
        let slot = self.matched.get_mut(among).ok_or(Halt)?;
        *slot = found;
        Ok(found.is_some())
    }

    /// `among`: the command its string at the cursor chooses, found now or
    /// by its `substring`; failing if no string matches.
    pub(super) fn among(&mut self, among: usize, substring: bool) -> Signal {
        let found = if substring { Some(self.matched.get(among).copied().flatten().ok_or(Halt)?) } else { self.find(among)? };
        let program = self.program;
        let Some(action) = found else { return Ok(false) };
        let Some(action) = action else { return Ok(true) };
        let command = program.amongs.get(among).and_then(|a| a.actions.get(action)).ok_or(Halt)?;
        self.run(command)
    }
}
