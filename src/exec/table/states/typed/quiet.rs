//! \[DESIGN\] (RULES.md D1256) Typed states finished for their errors
//! alone. A sort's first rows picked before their groups' rows are made
//! leave most groups unfinished, and a result that raises must raise all
//! the same, as the plan run alone finishes every group. An exact total
//! raises only where it is very large, which its units tell with no
//! value made, so a column of totals is passed over in one loop and only
//! a total that could raise is finished to know.

use super::{ColumnarKind, Exact, Totals, Typed};
use crate::error::Result;
use crate::value::DataType;

/// \[DESIGN\] (RULES.md D1256) The digits under which an exact total
/// raises nothing as it is finished: one fewer than the most an exact
/// number has, so a total of that many fits whatever its last digit.
const QUIET_DIGITS: u32 = crate::value::MAX_EXACT_PRECISION.saturating_sub(1);

/// The radix a total's digits are counted in.
const TEN: u128 = 10;

/// \[DESIGN\] (RULES.md D1308) The greatest total of either sign a
/// narrow column holds, 2^63, the least integer of 64 bits without its
/// sign.
const NARROW_MOST: u128 = 1 << 63;

impl Typed {
    /// \[DESIGN\] (RULES.md D1256) Finishes each of the first `len`
    /// groups' results for the error one may raise, keeping none. A count
    /// raises nothing, nor does the least or greatest string. An exact
    /// total is finished only where it is large enough that it could
    /// (`Exact::checked`).
    ///
    /// # Errors
    ///
    /// As the accumulator's `finish`, of the first group that fails.
    pub(in crate::exec::table::states) fn checked(&self, len: usize) -> Result<()> {
        match self {
            // A count raises nothing, and a best string is given as it is.
            Typed::Count(_) | Typed::Best(_) => Ok(()),
            Typed::Exact(exact) => exact.checked(len),
            Typed::Approx(approx) => (0..len).try_for_each(|g| approx.finish(g).map(drop)),
        }
    }
}

impl Exact {
    /// \[DESIGN\] (RULES.md D1256) Finishes the results of the first
    /// `len` groups that may raise, keeping none. A total under
    /// `quiet_below` raises nothing whatever its count, so it is passed
    /// over, and any other is finished to know. A narrow column no total
    /// of which can reach the bound is passed over whole, and each total
    /// of any other is compared as it is held (RULES.md D1308).
    fn checked(&self, len: usize) -> Result<()> {
        let bound = self.quiet_below();
        let loud = |g: usize, total: u128| if total >= bound { self.finish(g).map(drop) } else { Ok(()) };
        match &self.totals {
            Totals::Narrow(totals) => {
                if bound > NARROW_MOST {
                    return Ok(());
                }
                totals.iter().take(len).enumerate().try_for_each(|(g, units)| loud(g, u128::from(units.unsigned_abs())))
            }
            Totals::Wide(totals) => totals.iter().take(len).enumerate().try_for_each(|(g, units)| loud(g, units.unsigned_abs())),
        }
    }

    /// The total, of either sign, under which finishing a group raises
    /// nothing. An extreme is given as it is. A sum raises only past the
    /// most digits an exact number has [X3.135-1986 §5.9 GR6a], and a
    /// mean only if its total, written at the scale of its result, is
    /// past them, so a total of `QUIET_DIGITS` digits less that shift is
    /// under both. A mean whose result is no exact number has no such
    /// bound, and every total of it is finished.
    fn quiet_below(&self) -> u128 {
        let digits = match (self.kind, &self.result) {
            (ColumnarKind::Min | ColumnarKind::Max, _) => return u128::MAX,
            (ColumnarKind::Sum | ColumnarKind::Count, _) => Some(QUIET_DIGITS),
            (ColumnarKind::Avg, DataType::Exact { scale, .. }) => QUIET_DIGITS.checked_sub(scale.saturating_sub(self.scale.unwrap_or(0))),
            (ColumnarKind::Avg, _) => None,
        };
        digits.and_then(|d| TEN.checked_pow(d)).unwrap_or(0)
    }
}
