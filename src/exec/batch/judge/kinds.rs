//! \[DESIGN\] RULES.md D31, D354: conjuncts judged over vectors of
//! approximate numbers, datetimes and truth values, no value made of each,
//! by the kernels: approximate numbers compared with an approximate
//! constant as two numbers compare by algebraic value [X3.135-1986 §5.11
//! GR4], `select_f64`, an exact constant as exactly as a row's judgment
//! compares it (`value::compare`); datetimes of one type by their instants
//! in UTC [ISO/IEC 9075:1992 §8.2 GR5], `select_i64`; truth values,
//! FALSE before TRUE [ISO/IEC 9075-2:2023 §4.6.1], `select_mask`. A null
//! is true of no comparison [X3.135-1986 §5.11 GR2], of IS NULL only
//! [§5.15]. And an OR of conjuncts judged over vectors, each of the same
//! rows, a row kept if one keeps it [§5.20 GR4].

use std::cmp::Ordering;

use super::Judged;
use super::ints::holds;
use crate::exec::Subqueries;
use crate::kernels::Kernels;
use crate::storage::mask::index;
use crate::storage::sel::rows_of;
use crate::storage::{Batch, Bools, Datetimes, Floats, Mask};
use crate::value::{Value, compare};

/// The rows of `over`, or of all `len`, that are null, or, if `not`, not
/// null, as `nulls` marks them.
pub(super) fn nulls_kept(nulls: Option<&Mask>, (len, over): (usize, Option<&[u32]>), not: bool, kernels: &dyn Kernels) -> Vec<u32> {
    let mut out = Vec::new();
    match (nulls, not) {
        (Some(nulls), false) => kernels.select_mask(nulls.words(), (None, over), &mut out),
        (Some(nulls), true) => kernels.select_mask(all_set(len).and_not(nulls).words(), (None, over), &mut out),
        (None, false) => {}
        (None, true) => out.extend(over.map_or_else(|| rows_of(0..len), <[u32]>::to_vec)),
    }
    out
}

/// `len` bits, every one set.
fn all_set(len: usize) -> Mask {
    let mut mask = Mask::none(len);
    (0..len).for_each(|n| mask.set(n));
    mask
}

/// The rows of `over`, or of all, that `judged` keeps of `floats`, if it is
/// judged of approximate numbers.
pub(super) fn floats(judged: &Judged, floats: &Floats, over: Option<&[u32]>, kernels: &dyn Kernels) -> Option<Vec<u32>> {
    let rows = || over.map_or_else(|| rows_of(0..floats.numbers.len()), <[u32]>::to_vec);
    let keep = |truth: &dyn Fn(f64) -> bool| Some(rows().into_iter().filter(|&n| floats.get(index(n)).is_some_and(truth)).collect());
    match judged {
        // [X3.135-1986 §5.15 GR2-GR4] never unknown.
        Judged::IsNull(_, not) => Some(nulls_kept(floats.nulls.as_ref(), (floats.numbers.len(), over), *not, kernels)),
        Judged::Compare(_, op, Value::Approx(c)) => {
            let mut out = Vec::new();
            kernels.select_f64(&floats.numbers, (*op, *c), (floats.nulls.as_ref(), over), &mut out);
            Some(out)
        }
        Judged::Compare(_, op, constant) => keep(&|f| holds(*op, compare(&Value::Approx(f), constant))),
        // [X3.135-1986 §5.13 GR2, §5.18] an OR of equalities: true if one
        // is; NOT of it true only if every one is false; a long list's by its set.
        Judged::In(_, set, not) => {
            if set.hashed() {
                let want = super::wanted(*not);
                return keep(&|f| set.truth(&Value::Approx(f)) == want);
            }
            let list = set.values();
            let ordering = |f: f64, c: &Value| compare(&Value::Approx(f), c);
            if *not {
                keep(&|f| list.iter().all(|c| ordering(f, c).is_some_and(Ordering::is_ne)))
            } else {
                keep(&|f| list.iter().any(|c| ordering(f, c) == Some(Ordering::Equal)))
            }
        }
        Judged::Like(..) | Judged::Compared(..) | Judged::LikeOf(..) | Judged::Any(_) => None,
    }
}

/// The rows of `over`, or of all, that `judged` keeps of `datetimes`, if
/// it is judged of their instants: a comparison with a datetime of the
/// same fields, or IS NULL.
pub(super) fn datetimes(judged: &Judged, datetimes: &Datetimes, over: Option<&[u32]>, kernels: &dyn Kernels) -> Option<Vec<u32>> {
    match judged {
        Judged::IsNull(_, not) => Some(nulls_kept(datetimes.nulls.as_ref(), (datetimes.len(), over), *not, kernels)),
        Judged::Compare(_, op, Value::Datetime(c)) => {
            if c.moment != datetimes.moment {
                return None;
            }
            let mut out = Vec::new();
            kernels.select_i64(&datetimes.utc, (*op, c.utc), (datetimes.nulls.as_ref(), over), &mut out);
            Some(out)
        }
        Judged::Compare(..) | Judged::In(..) | Judged::Like(..) | Judged::Compared(..) | Judged::LikeOf(..) | Judged::Any(_) => None,
    }
}

/// The rows of `over`, or of all, that `judged` keeps of `bools`, if it is
/// judged of truth values: a comparison with one, or IS NULL.
pub(super) fn bools(judged: &Judged, bools: &Bools, over: Option<&[u32]>, kernels: &dyn Kernels) -> Option<Vec<u32>> {
    match judged {
        Judged::IsNull(_, not) => Some(nulls_kept(bools.nulls.as_ref(), (bools.len(), over), *not, kernels)),
        Judged::Compare(_, op, Value::Bool(c)) => {
            let set = all_set(bools.len());
            // Of every truth value, whether `op` holds of it against `c`.
            let of_true = holds(*op, Some(true.cmp(c)));
            let of_false = holds(*op, Some(false.cmp(c)));
            let true_of = match (of_true, of_false) {
                (true, true) => set,
                (true, false) => bools.bits.clone(),
                (false, true) => set.and_not(&bools.bits),
                (false, false) => Mask::none(bools.len()),
            };
            let mut out = Vec::new();
            kernels.select_mask(true_of.words(), (bools.nulls.as_ref(), over), &mut out);
            Some(out)
        }
        Judged::Compare(..) | Judged::In(..) | Judged::Like(..) | Judged::Compared(..) | Judged::LikeOf(..) | Judged::Any(_) => None,
    }
}

/// The rows of `over`, or of all, that one of `disjuncts` keeps, each
/// judged of them all; none if one cannot be judged over vectors.
pub(super) fn any(
    disjuncts: &[Judged],
    batch: &Batch,
    over: Option<&[u32]>,
    (subqueries, kernels): (&dyn Subqueries, &dyn Kernels),
) -> Option<Vec<u32>> {
    let mut held = Mask::none(batch.rows);
    for disjunct in disjuncts {
        let true_of = disjunct.kept(batch, over, subqueries, kernels)?;
        for &r in &true_of {
            held.set(index(r));
        }
    }
    let mut out = Vec::new();
    kernels.select_mask(held.words(), (None, over), &mut out);
    Some(out)
}
