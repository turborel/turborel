//! \[DESIGN\] (RULES.md D653) The rows a hash join's runtime filter leaves
//! of its probing operand (`exec/table/runtime.rs`, RULES.md D369). The
//! filter is made of the built rows' keys and drops each probing row whose
//! key none of them holds, before the join hashes it, so a join whose
//! built operand keeps few of its key's values hashes and probes only the
//! share of the probing rows whose keys those values are. Of each class of
//! the join's equalities, that share is the built operand's distinct
//! values over the probing operand's, the built distinct at most its
//! rows, as Zhu, Potti, Saurabh and Patel (PVLDB 2017) count what a
//! build's Bloom filter keeps of a probe; the least share of the join's
//! classes is taken. Each probing row still pays `FILTER_ROW` to be
//! judged by the filter, so a join whose build keeps every value is
//! dearer by that for nothing, as the executor's filter that stops
//! judging (`KEEP_MOST`) bounds it. So join orders put a selective build
//! where its filter reaches the scan it drops rows of.

use crate::db::State;

/// Whether hash joins make runtime filters, as EXECUTOR.RUNTIME_FILTERS
/// says (`exec/batch/runtime.rs`).
pub(crate) fn wanted(db: &State) -> bool {
    if let crate::value::Value::Char(s) = db.settings.get(crate::settings::EXECUTOR_RUNTIME_FILTERS) {
        s != crate::settings::OFF
    } else {
        true
    }
}

/// \[DESIGN\] What a probing row costs to be judged by a runtime filter, in
/// rows of a scan: a hash of its key and a test of a block of bits, as a
/// conjunct judged by batch costs (`bench/analytics/costs.rs`).
pub(in crate::physical) const FILTER_ROW: f64 = 0.45;
/// \[DESIGN\] The share of the probing rows past which the executor stops
/// judging them by the filter, nine in ten (`exec/table/runtime.rs`).
const KEEP_MOST: f64 = 0.9;

/// The share of a join's probing rows its runtime filter keeps, of the
/// classes of its equalities each with the distinct values of its key of
/// the built operand and of the probing one, the built operand giving
/// `built` rows: 1 if it judges no class.
pub(in crate::physical) fn kept(classes: &[(f64, f64)], built: f64) -> f64 {
    classes
        .iter()
        .map(|&(of_built, of_probing)| (of_built.min(built).max(1.0_f64) / of_probing.max(1.0_f64)).min(1.0_f64))
        .fold(1.0_f64, f64::min)
}

/// The cost of hashing and probing with `probing` rows, at `per_row`
/// each, of which the runtime filter keeps the share `kept`: each row
/// judged by the filter, and those it keeps hashed. None filtered where
/// it keeps more than `KEEP_MOST`, as the executor stops such a filter.
pub(in crate::physical) fn probed(probing: f64, per_row: f64, kept: f64) -> f64 {
    if kept > KEEP_MOST {
        return probing * per_row;
    }
    probing * (FILTER_ROW + per_row * kept)
}
