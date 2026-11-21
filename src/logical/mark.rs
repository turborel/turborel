//! \[DESIGN\] The mark join (RULES.md D640), after Neumann, Leis and Kemper,
//! "The Complete Story of Joins (in HyPer)" (BTW 2017), §3.3: each row of
//! its left operand once, in its order, with one more column, the mark,
//! true, false or null for unknown.
//!
//! The mark of a left row is the OR [ISO/IEC 9075:1992 §8.12 GR2] of the
//! truth value of `compared`, or of true where there is none, over the
//! right rows that make `cond` true with it; false of none. Of EXISTS
//! that is whether such a row is there [X3.135-1986 §5.17 GR2]; of `x op
//! SOME s`, of `compared` the comparison of `x` with the subquery's row,
//! it is the quantified comparison's value [X3.135-1986 §5.16 GR2], and
//! `x op ALL s` is NOT of the mark of the comparison negated, since ALL is
//! true where no row makes it false and unknown where none does but one
//! makes it unknown.

use super::{Cond, LogicalPlan};

/// The display name of a mark join's mark column.
pub const MARK_COLUMN: &str = "MARK";

/// \[DESIGN\] A mark join (see the module's documentation).
#[derive(Debug, Clone)]
pub struct MarkJoin {
    /// The condition a pair's right row must make true to count, over the
    /// left's columns then the right's.
    pub cond: Cond,
    /// The condition whose truth values the mark ORs, over the pair's
    /// columns; none of EXISTS, whose rows count as true.
    pub compared: Option<Cond>,
    /// The rows marked.
    pub left: LogicalPlan,
    /// The rows they are matched against.
    pub right: LogicalPlan,
}

impl MarkJoin {
    /// Its conditions, `cond` then `compared`.
    pub fn conds(&self) -> impl Iterator<Item = &Cond> {
        std::iter::once(&self.cond).chain(self.compared.as_ref())
    }

    /// It, each of its conditions replaced by `f` of it.
    #[must_use]
    pub fn map_conds(self, f: &mut dyn FnMut(Cond) -> Cond) -> MarkJoin {
        let MarkJoin { cond, compared, left, right } = self;
        MarkJoin { cond: f(cond), compared: compared.map(&mut *f), left, right }
    }
}
