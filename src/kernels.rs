//! \[DESIGN\] The kernels an executor's batches run on (RULES.md D354): the
//! loops of every primitive over a vector, a selection of rows, a sum, a
//! gather, written once each, as Boncz, Zukowski and Nes, "MonetDB/X100"
//! (2005), §4, write a primitive per type, operator and operand shape, and
//! passing selection vectors between them in place of the rows copied.
//!
//! `Kernels` is an extension point (`db.rs`): the database holds one, the
//! built-in `Portable` unless a plug-in's is set (`Database::set_kernels`),
//! which may run the same primitives on wider instructions. Every method
//! has its portable body as its default, so a set of kernels that predates
//! a method still runs it, and every one must give what the portable body
//! gives, of any input: the contract tests in `turborel-test-support`
//! (`kernels.rs`) hold each implementation to it.
//!
//! A selection's rows are ascending, each below its vector's length; a
//! null's slot holds 0, and its row is left out of every selection.

pub mod portable;

use crate::storage::Mask;
use crate::value::CompOp;

/// The rows a kernel reads: the nulls to leave out, if any is null, and
/// the rows of a selection, or all.
pub type Given<'a> = (Option<&'a Mask>, Option<&'a [u32]>);

/// \[DESIGN\] Primitives over vectors, each a loop a batch's operator runs
/// in place of one a row (RULES.md D354). Each default is the portable
/// body (`kernels/portable`); an implementation gives the same results.
pub trait Kernels: Send + Sync {
    /// Its name, as diagnostics show it.
    fn name(&self) -> &'static str;

    /// Appends to `out`, ascending, the rows of `given`'s selection, or of
    /// all `values`, not null, whose value `v` satisfies `v op c` of
    /// `pred = (op, c)`.
    fn select_i64(&self, values: &[i64], pred: (CompOp, i64), given: Given<'_>, out: &mut Vec<u32>) {
        portable::select::select_i64(values, pred, given, out);
    }

    /// As `select_i64`, of approximate numbers, compared as `f64::partial_cmp`
    /// orders them, as a row's judgment compares two (`value::compare`).
    fn select_f64(&self, values: &[f64], pred: (CompOp, f64), given: Given<'_>, out: &mut Vec<u32>) {
        portable::select::select_f64(values, pred, given, out);
    }

    /// Appends to `out` the rows of `given`, not null, whose bit is set in
    /// `bits`, words of a `Mask`.
    fn select_mask(&self, bits: &[u64], given: Given<'_>, out: &mut Vec<u32>) {
        portable::select::select_mask(bits, given, out);
    }

    /// Appends to `out` the rows of `given`, not null, whose dictionary
    /// code's bit is set in `keep`, words of a `Mask` of the entries.
    fn select_codes(&self, codes: &[u32], keep: &[u64], given: Given<'_>, out: &mut Vec<u32>) {
        portable::select::select_codes(codes, keep, given, out);
    }

    /// The exact sum of `values` at `sel`, or of all; a null's slot holds
    /// 0, so it adds nothing.
    fn sum_i64(&self, values: &[i64], sel: Option<&[u32]>) -> i128 {
        portable::sum::sum_i64(values, sel)
    }

    /// The least and greatest of `values` of `given`, not null; none of no
    /// row.
    fn min_max_i64(&self, values: &[i64], given: Given<'_>) -> Option<(i64, i64)> {
        portable::sum::min_max_i64(values, given)
    }

    /// Appends to `out` the values at `sel`, in order; 0 for a row past the
    /// end.
    fn gather_i64(&self, values: &[i64], sel: &[u32], out: &mut Vec<i64>) {
        portable::gather::gather_i64(values, sel, out);
    }
}

/// \[DESIGN\] The built-in kernels: every portable body, as written.
#[derive(Debug, Clone, Copy, Default)]
pub struct Portable;

/// The name of the built-in kernels.
pub const PORTABLE: &str = "PORTABLE";

impl Kernels for Portable {
    fn name(&self) -> &'static str {
        PORTABLE
    }
}
