//! \[DESIGN\] The commands of a Snowball program that test or change the
//! string (RULES.md D320): matching a string or grouping at the cursor,
//! and replacing, inserting and copying characters, the cursor, limits
//! and slice moved as the runtime's `replace_s`, `slice_from` and
//! `insert` move them (`python/snowballstemmer/basestemmer.py`).

use std::borrow::Cow;

use super::{Halt, Machine, Signal};
use crate::fulltext::text::snowball::ast::Text;

/// `place` where it was before characters `removed` of a replacement
/// were taken out and `added` put in.
fn moved(place: usize, removed: usize, added: usize) -> Result<usize, Halt> {
    place.checked_sub(removed).and_then(|p| p.checked_add(added)).ok_or(Halt)
}

impl<'p> Machine<'p> {
    /// The characters `text` stands for.
    fn chars(&self, text: &'p Text) -> Cow<'p, [char]> {
        match text {
            Text::Literal(chars) => Cow::Borrowed(chars),
            Text::Variable(variable) => Cow::Owned(self.strings.get(*variable).cloned().unwrap_or_default()),
        }
    }

    /// A literal string or string variable matched at the cursor, the
    /// cursor moved past it.
    pub(super) fn matches(&mut self, text: &'p Text) -> bool {
        let chars = self.chars(text);
        let env = &mut self.env;
        let n = chars.len();
        let found = if self.backward {
            env.c.checked_sub(n).filter(|start| *start >= env.lb).filter(|start| env.s.get(*start..env.c) == Some(chars.as_ref()))
        } else {
            env.c.checked_add(n).filter(|end| *end <= env.l).filter(|end| env.s.get(env.c..*end) == Some(chars.as_ref()))
        };
        found.is_some_and(|to| {
            env.c = to;
            true
        })
    }

    /// The character at the cursor matched against a grouping, or with
    /// `non` against its complement, the cursor moved past it.
    pub(super) fn grouping(&mut self, grouping: usize, non: bool) -> bool {
        let program = self.program;
        let Some(chars) = program.groupings.get(grouping) else { return false };
        let env = &mut self.env;
        let at = if self.backward {
            env.c.checked_sub(1).filter(|at| *at >= env.lb)
        } else {
            Some(env.c).filter(|at| *at < env.l)
        };
        let Some(c) = at.and_then(|at| env.s.get(at)) else { return false };
        if chars.binary_search(c).is_ok() == non {
            return false;
        }
        env.c = if self.backward { env.c.saturating_sub(1) } else { env.c.saturating_add(1) };
        true
    }

    /// The characters from `bra` to `ket` replaced by `chars`, the limit
    /// and cursor moved: the cursor after them moved with them, one inside
    /// them to their start.
    fn replace(&mut self, bra: usize, ket: usize, chars: &[char]) -> Result<(), Halt> {
        let env = &mut self.env;
        if bra > ket || ket > env.s.len() {
            return Err(Halt);
        }
        let removed = ket.saturating_sub(bra);
        env.s.splice(bra..ket, chars.iter().copied());
        env.l = moved(env.l, removed, chars.len())?;
        if env.c >= ket {
            env.c = moved(env.c, removed, chars.len())?;
        } else if env.c > bra {
            env.c = bra;
        }
        Ok(())
    }

    /// Whether the slice is in order and within the limit.
    fn slice_valid(&self) -> bool {
        let env = &self.env;
        env.bra <= env.ket && env.ket <= env.l && env.l <= env.s.len()
    }

    /// `<- S`: the slice replaced by S, the slice then S.
    pub(super) fn slice_from(&mut self, text: &'p Text) -> Signal {
        if !self.slice_valid() {
            return Err(Halt);
        }
        let chars = self.chars(text);
        let (bra, ket) = (self.env.bra, self.env.ket);
        self.replace(bra, ket, &chars)?;
        self.env.ket = bra.checked_add(chars.len()).ok_or(Halt)?;
        Ok(true)
    }

    /// `-> s`: the slice copied to `s`.
    pub(super) fn slice_to(&mut self, variable: usize) -> Signal {
        if !self.slice_valid() {
            return Err(Halt);
        }
        let slice = self.env.s.get(self.env.bra..self.env.ket).ok_or(Halt)?.to_vec();
        *self.strings.get_mut(variable).ok_or(Halt)? = slice;
        Ok(true)
    }

    /// `=> s`: the string to the limit copied to `s`.
    pub(super) fn assign_to(&mut self, variable: usize) -> Signal {
        let start = self.env.s.get(..self.env.l).ok_or(Halt)?.to_vec();
        *self.strings.get_mut(variable).ok_or(Halt)? = start;
        Ok(true)
    }

    /// The characters from `bra` to `ket` replaced by `chars`, the slice
    /// moved with what comes after it.
    fn insert_at(&mut self, bra: usize, ket: usize, chars: &[char]) -> Result<(), Halt> {
        self.replace(bra, ket, chars)?;
        let removed = ket.saturating_sub(bra);
        if bra <= self.env.bra {
            self.env.bra = moved(self.env.bra, removed, chars.len())?;
        }
        if bra <= self.env.ket {
            self.env.ket = moved(self.env.ket, removed, chars.len())?;
        }
        Ok(())
    }

    /// `insert S`, or `attach S`: S put in at the cursor, the cursor left
    /// before it if `keep`, else moved past it.
    pub(super) fn insert(&mut self, text: &'p Text, keep: bool) -> Signal {
        let chars = self.chars(text);
        let c = self.env.c;
        self.insert_at(c, c, &chars)?;
        if keep {
            self.env.c = c;
        }
        Ok(true)
    }

    /// `= S`: the string from the cursor to the limit, or backward from
    /// the backward limit to the cursor, replaced by S.
    pub(super) fn assign(&mut self, text: &'p Text) -> Signal {
        let chars = self.chars(text);
        let c = self.env.c;
        if self.backward {
            let lb = self.env.lb;
            self.insert_at(lb, c, &chars)?;
        } else {
            let l = self.env.l;
            self.insert_at(c, l, &chars)?;
            self.env.c = c;
        }
        Ok(true)
    }
}
