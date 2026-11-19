//! \[DESIGN\] The filter over a table access: the conjuncts the access
//! does not answer itself, and the rows they are estimated to keep, a
//! spatial probe's estimate standing for the windowed spatial conjuncts
//! (RULES.md D137).

use super::super::estimate::Known;
use super::super::shown::Shown;
use super::super::{Built, CostModel, Plan, TableRead};
use super::geometric;
use crate::logical::{Cond, and_all, judged_by_sets, render_cond};

/// A table without storage scanned anyway under a filter on `conds`, of
/// no rows, with the guesses `cost`.
pub(super) fn unstored(read: TableRead<'_>, conds: Vec<Cond>, cost: &CostModel) -> Built {
    let plan = Plan::SeqScan {
        table: read.table.clone(),
        columns: read.needed.to_vec(),
        rows: 0,
        cost: 0.0_f64,
        order: Vec::new(),
        backward: false,
        most: None,
        located: read.located,
    };
    let known = Known { stats: None, cost };
    filtered(plan, conds, &Filtered { shown: read.shown, known, ordered: false, read: 0.0, most: 0.0, note: "", windowed: None })
}

/// What a filter over a table access is planned with.
pub(super) struct Filtered<'a> {
    /// How the filter is described (`physical/shown.rs`).
    pub(super) shown: Shown<'a>,
    /// The table's statistics, if its engine gives any, and the guesses.
    pub(super) known: Known<'a>,
    /// Whether the access yields rows in the order asked for.
    pub(super) ordered: bool,
    /// The row count used to estimate the fraction a filter keeps: the
    /// whole table for a scan or function lookup, else comparison matches.
    pub(super) read: f64,
    /// The most rows the filter may keep: those the access reads, a scan
    /// only those of the zones or chunks its conditions may match
    /// (`TableStorage::scan_share`).
    pub(super) most: f64,
    /// What EXPLAIN shows of a spatial probe priced against the scan
    /// (RULES.md D137), if any, after the condition.
    pub(super) note: &'a str,
    /// The share of the rows read the windowed spatial conjuncts keep, if
    /// a spatial probe was priced.
    pub(super) windowed: Option<f64>,
}

/// A table access `plan` under a filter for the conjuncts `conds` it does not
/// answer itself, estimated to keep the fraction of the rows that the
/// table's statistics give, no more than the access reads: a scan that
/// skips the rows its conditions cannot match keeps the same rows of the
/// fewer it reads, not a fraction of them again. Its long IN lists are
/// judged by sets (`logical::judged_by_sets`). Of no conjunct left, an
/// index scan deciding its conjunct of each entry (RULES.md D85) shows
/// the spatial note itself.
pub(super) fn filtered(plan: Plan, conds: Vec<Cond>, with: &Filtered<'_>) -> Built {
    let kept = geometric::kept(&conds, with.known, with.windowed);
    let plan = match and_all(conds) {
        Some(c) => {
            let est = Some((with.read * kept).min(with.most));
            let c = judged_by_sets(c);
            let shown = with.shown.text(|names| render_cond(&c, names));
            let detail = if with.note.is_empty() { shown } else { format!("{shown}  {}", with.note) };
            Plan::Filter { detail, cond: c, est, input: Box::new(plan) }
        }
        None => noted(plan, with.note),
    };
    Built { plan, ordered: with.ordered }
}

/// `plan` shown with `note`, what EXPLAIN shows of the spatial probe
/// priced, after its detail, if it is an index scan and there is a note.
fn noted(mut plan: Plan, note: &str) -> Plan {
    if let Plan::IndexScan { detail, .. } = &mut plan
        && !note.is_empty()
    {
        detail.push_str("  ");
        detail.push_str(note);
    }
    plan
}
