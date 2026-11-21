//! \[EXTENSION\] Lookups of a json index (RULES.md D34): a conjunct that is
//! JSON_EXISTS of the indexed column, JSON_VALUE of it compared equal to a
//! constant or `?`, or JSON_CONTAINS of it and one, each of whose other
//! arguments is a constant or `?`. Its PASSING arguments' values, then the
//! value compared or contained, make the probe's ROW, a slot for each `?`.
//!
//! Such a call raises of a document only where the index finds the row
//! anyway: it raises nothing of a document it reads with NULL ON ERROR,
//! and a document the index cannot read it finds by every probe. So with
//! the WHERE check of each row found, a probe raises what a scan would
//! (D15), and such a conjunct is no reason not to probe the index.

use super::{Candidate, predicate};
use crate::catalog::IndexDesc;
use crate::functions::{JsonOperator, ScalarRef};
use crate::index::json::one_text_column;
use crate::index::{JSON_METHOD, Predicate, Probe};
use crate::logical::{Cond, Expr};
use crate::physical::{Place, ProbeSlot};
use crate::value::{CompOp, DataType, Value};

/// The call `c` is, the arguments it passes but the column, and the value
/// it compares with, of a JSON_VALUE compared equal.
fn json_call(c: &Cond) -> Option<(&ScalarRef, &[Expr], Option<&Expr>)> {
    if let Cond::Value(Expr::Call(function, args)) = c {
        return Some((function, args, None));
    }
    let Cond::Compare(left, CompOp::Eq, right) = c else { return None };
    let ((Expr::Call(function, args), other) | (other, Expr::Call(function, args))) = (left, right) else { return None };
    let probe = function.0.json_probe()?;
    (probe.operator == JsonOperator::Value).then_some((function, args.as_slice(), Some(other)))
}

/// Whether `expr` is a constant or `?`.
fn constant(expr: &Expr) -> bool {
    matches!(expr, Expr::Const(_) | Expr::Dynamic(_))
}

/// The column `c` looks up and its call's other arguments, with the value
/// it compares with: of a call whose errors are only of documents the
/// index finds anyway (module comment).
fn lookup(c: &Cond) -> Option<(usize, &ScalarRef, Vec<&Expr>)> {
    let (function, args, compared) = json_call(c)?;
    let probe = function.0.json_probe()?;
    let (Expr::Column(column), rest) = args.split_first()? else { return None };
    let passed = rest.len() == probe.passing.len();
    let (safe, fields): (bool, Vec<&Expr>) = match (probe.operator, compared) {
        (JsonOperator::Exists, None) => (passed && probe.lookup_safe(), rest.iter().collect()),
        (JsonOperator::Value, Some(other)) => (passed && probe.lookup_safe() && probe.empty_null, rest.iter().chain([other]).collect()),
        (JsonOperator::Contains, None) => (rest.len() == 1, rest.iter().collect()),
        (JsonOperator::Exists | JsonOperator::Contains, Some(_)) | (JsonOperator::Value, None) | (JsonOperator::Query, Some(_) | None) => {
            (false, Vec::new())
        }
    };
    (safe && fields.iter().all(|e| constant(e))).then_some((*column, function, fields))
}

/// Whether `c` is a lookup of index `ix` whose errors its probe raises as
/// a scan would (module comment): of a json index on its column.
pub(super) fn rechecked(c: &Cond, ix: &IndexDesc) -> bool {
    ix.method == JSON_METHOD && lookup(c).is_some_and(|(column, _, _)| ix.columns == [column])
}

/// The lookups of json index `ix` answering conjuncts of `conds`: none
/// if a conjunct can raise where a probe would not (RULES.md D15), unless
/// a probe raises what a scan would of it, as of a call it rechecks.
pub(super) fn candidates(conds: &[Cond], ix: &IndexDesc, types: &[DataType], values: Option<&[Value]>, all: f64) -> Vec<Candidate> {
    let unanswerable = conds.iter().any(|c| c.can_raise() && !predicate::rechecked_lookup(c) && !rechecked(c, ix));
    if ix.method != JSON_METHOD || !one_text_column(types) || unanswerable {
        return Vec::new();
    }
    conds
        .iter()
        .enumerate()
        .filter_map(|(at, c)| {
            let (column, function, fields) = lookup(c)?;
            if ix.columns != [column] {
                return None;
            }
            let mut row = Vec::with_capacity(fields.len());
            let mut slots = Vec::new();
            for (n, field) in fields.into_iter().enumerate() {
                let (value, slot) = predicate::constant(field, values)?;
                row.push(value);
                slots.extend(slot.map(|slot| ProbeSlot::of(Place::PredicateField(n), slot)));
            }
            let predicate = Box::new(Predicate { function: function.clone(), column: 0, argument: None });
            let probe = Probe::Predicate { predicate, value: Value::Row(row.into()) };
            // D34: every row found is checked by the whole WHERE condition.
            Some(Candidate { slots, ..Candidate::one(probe, (vec![at], 0, all, true)) })
        })
        .collect()
}
