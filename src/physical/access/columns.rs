//! \[DESIGN\] Independent comparisons for index methods that can
//! constrain several key columns without an equality prefix. Existing
//! SQL comparisons supply the bounds; this adds no SQL syntax.

use super::{Candidate, probes};
use crate::index::{ColumnComparison, IndexMethod, Probe};
use crate::physical::estimate::{self, Known};
use crate::physical::kept::Place;
use crate::physical::probe::{Alternatives, Compared};
use crate::value::{CompOp, DataType};

pub(super) fn candidates(
    (columns, types): (&[usize], &[DataType]),
    (compared, alternatives): (&[Compared], &[Alternatives]),
    (all, known): (f64, Known<'_>),
    method: &dyn IndexMethod,
) -> Vec<Candidate> {
    let mut out = probes(columns, types, (compared, alternatives), all, known);
    if let Some(candidate) = independent((columns, types), compared, (all, known), method) {
        out.push(candidate);
    }
    out
}

fn independent(
    (columns, types): (&[usize], &[DataType]),
    compared: &[Compared],
    (all, known): (f64, Known<'_>),
    method: &dyn IndexMethod,
) -> Option<Candidate> {
    let mut each = Vec::new();
    let mut candidate = Candidate::one(Probe::All, (Vec::new(), 0, all, false));
    for (at, (column, data_type)) in columns.iter().zip(types).enumerate() {
        for c in compared.iter().filter(|c| c.column == *column && c.op != CompOp::Ne) {
            let Some(placed) = c.placed(data_type) else { continue };
            for (op, value) in placed {
                candidate.slots.extend(c.slot_at(Place::Column(each.len())));
                each.push(ColumnComparison { column: at, op, value });
            }
            if c.exact {
                candidate.used.push(c.at);
            }
            candidate.loose |= !c.exact;
            candidate.guess *= estimate::compare((c.column, c.op, &c.value), known);
        }
    }
    let operators: Vec<_> = each.iter().map(|c| (c.column, c.op)).collect();
    if each.is_empty() || !method.supports_columns(columns.len(), &operators) {
        return None;
    }
    candidate.probes = vec![Probe::Columns(each)];
    Some(candidate)
}
