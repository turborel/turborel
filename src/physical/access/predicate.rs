//! \[DESIGN\] Index lookups for WHERE function calls such as `f(column, ?)`
//! (RULES.md D34). These helpers describe the lookup without calling the
//! function. The executor checks the full WHERE condition on fetched rows.

use super::Candidate;
use crate::catalog::{ColumnDesc, IndexDesc};
use crate::functions::ScalarRef;
use crate::index::{IndexMethod, Predicate, Probe};
use crate::logical::{Cond, Expr, Slot, filled_expr};
use crate::physical::{Place, ProbeSlot};
use crate::value::{CompOp, DataType, Value, compare};
use std::cmp::Ordering;

/// Whether this is a function lookup, or an XML lookup, requiring the full
/// WHERE check (RULES.md D34, D106).
pub(super) fn named(probes: &[Probe]) -> bool {
    match probes.first() {
        Some(Probe::Xml(asked)) => !asked.answered,
        Some(probe) => probe.opaque(),
        None => false,
    }
}

/// Finds function calls with one indexed column and a constant or parameter.
/// Collated indexes are excluded because their keys replace the original
/// column values with sort keys, which cannot be passed to the function.
pub(super) fn candidates(conds: &[Cond], ix: &IndexDesc, values: Option<&[Value]>, all: f64) -> Vec<Candidate> {
    if ix.is_collated() {
        return Vec::new();
    }
    conds
        .iter()
        .enumerate()
        .filter_map(|(at, c)| {
            let (function, args) = predicate_call(c)?;
            let args = probed(function, args)?;
            if raises(function, args) {
                return None;
            }
            let (column, argument, other) = operands(args)?;
            let column = ix.columns.iter().position(|&key_column| key_column == column)?;
            let (value, slot) = constant(other, values)?;
            let predicate = Box::new(Predicate { function: function.clone(), column, argument: Some(argument) });
            let probe = Probe::Predicate { predicate, value };
            let slots = slot.into_iter().map(|slot| ProbeSlot { place: Place::PredicateValue, slot }).collect();
            // D34: even an exact index leaves the full WHERE check in place.
            Some(Candidate { slots, ..Candidate::one(probe, (vec![at], 0, all, true)) })
        })
        .collect()
}

/// The call a condition is a lookup of: a call of a BOOLEAN result, or one
/// compared equal to the value its function declares it is where it stands
/// for a predicate (`ScalarFunction::predicate_truth`, RULES.md D87), as
/// `CONTAINS(doc, p) = 1`, either way round.
pub(crate) fn predicate_call(c: &Cond) -> Option<(&ScalarRef, &[Expr])> {
    if let Cond::Value(Expr::Call(function, args)) = c {
        return Some((function, args));
    }
    let Cond::Compare(left, CompOp::Eq, right) = c else { return None };
    let ((Expr::Call(function, args), Expr::Const(value)) | (Expr::Const(value), Expr::Call(function, args))) = (left, right) else {
        return None;
    };
    let truth = function.0.predicate_truth()?;
    (compare(value, &truth) == Some(Ordering::Equal)).then_some((function, args.as_slice()))
}

/// The arguments of `args` an index reads (`ScalarFunction::probe_arguments`).
fn probed<'a>(function: &ScalarRef, args: &'a [Expr]) -> Option<&'a [Expr]> {
    function.0.probe_arguments().map_or(Some(args), |n| args.get(..n))
}

/// Whether a call of `function` of `args` may raise in a way an index lookup
/// would change (RULES.md D15): of an argument, or of a function that is
/// neither an index predicate nor one whose errors the WHERE check raises
/// as a scan would (D34, D87). Its constant compared with is never raising.
fn raises(function: &ScalarRef, args: &[Expr]) -> bool {
    let f = &function.0;
    let promised = f.index_predicate() || f.rechecked_errors();
    !promised || !f.deterministic() || args.iter().any(Expr::can_raise)
}

/// Whether `c` is a lookup the WHERE check rechecks (D87): a call of a
/// function whose errors are of its arguments, alike for every row, of
/// arguments none of which raises. An index given an argument that raises
/// finds every row it holds of a value, and a null one raises nowhere, so
/// the WHERE check raises where a scan would and the conjunct need not
/// keep the table from being probed (D15).
pub(crate) fn rechecked_lookup(c: &Cond) -> bool {
    let Some((function, args)) = predicate_call(c) else { return false };
    let f = &function.0;
    f.rechecked_errors() && f.deterministic() && args.iter().all(|a| !a.can_raise())
}

/// The conjuncts of `conds` an index probe may answer. RULES.md D15: a
/// probe reads only the rows its conjuncts select, so the other conjuncts
/// would not be evaluated on the rest. When one of them can raise an error
/// it must see every row, so no index is probed then. A rechecked lookup
/// raises where the scan would (D87).
pub(super) fn answerable(conds: &[Cond]) -> &[Cond] {
    if conds.iter().any(|c| c.can_raise() && !rechecked_lookup(c)) { &[] } else { conds }
}

/// Returns the table column number, its position in the call and the other
/// argument. Order matters: `f(column, value)` need not equal `f(value, column)`.
pub(super) fn operands(args: &[Expr]) -> Option<(usize, usize, &Expr)> {
    match args {
        [Expr::Column(column), other] => Some((*column, 0, other)),
        [other, Expr::Column(column)] => Some((*column, 1, other)),
        _ => None,
    }
}

/// Gets the constant or current parameter value, retaining its slot for
/// later executions. Plan filling reports any failed parameter conversion.
pub(super) fn constant(expr: &Expr, values: Option<&[Value]>) -> Option<(Value, Option<Slot>)> {
    match expr {
        Expr::Const(value) => Some((value.clone(), None)),
        Expr::Dynamic(slot) => {
            let Expr::Const(value) = filled_expr(expr.clone(), values?).ok()? else { return None };
            Some((value, Some(slot.clone())))
        }
        Expr::Column(_)
        | Expr::Neg(_)
        | Expr::Arith(..)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Cast(..)
        | Expr::Case(..)
        | Expr::Subquery(..)
        | Expr::SortKey(..)
        | Expr::Domain(..)
        | Expr::Warned(..)
        | Expr::Truth(_)
        | Expr::NextValue(_)
        | Expr::Invoked(_) => None,
    }
}

/// Whether the method supports this lookup on the given key types.
/// Function support must cover every accepted other argument, including
/// values supplied by later executions of a prepared statement (D34).
pub(super) fn supports(method: &dyn IndexMethod, probe: &Probe, (types, ordering): (&[DataType], bool)) -> bool {
    match probe {
        Probe::All => method.ordered() && ordering,
        Probe::Columns(each) => method.supports_columns(types.len(), &each.iter().map(|c| (c.column, c.op)).collect::<Vec<_>>()),
        Probe::Compare { op, .. } => method.supports_compare(*op, types),
        Probe::Prefix { equal, then } => method.supports_prefix(types.len(), equal.len(), then.as_ref().map(|(op, _)| *op)),
        Probe::Range { equal, .. } => method.supports_range(types.len(), equal.len()),
        Probe::Predicate { predicate, .. } => method.supports_predicate(predicate, types),
        // An index that prices no XML lookup is asked none (`physical/access/xml.rs`).
        Probe::Xml(_) => true,
    }
}

/// Whether omitting null keys could hide a matching row. A function call
/// rejects nulls in its argument column, but may leave other key columns null.
pub(super) fn omits_rows(ix: &IndexDesc, columns: &[&ColumnDesc], probe: Option<&Probe>) -> bool {
    // RULES.md D106: a query of a null context item is not true, nor
    // otherwise than null, so its rows are none an XML lookup must give.
    if matches!(probe, Some(Probe::Xml(_))) {
        return false;
    }
    if matches!(probe, Some(Probe::Predicate { .. })) {
        return !ix.nulls && untested_nulls(columns, probe);
    }
    columns.get(probe.map_or(0, tested)..).unwrap_or_default().iter().any(|col| !col.not_null)
}

/// Whether a function lookup may keep nulls in another key column. Such
/// rows need an index that holds null keys; their order may still require
/// a SQL sort (D34).
pub(super) fn untested_nulls(columns: &[&ColumnDesc], probe: Option<&Probe>) -> bool {
    let Some(Probe::Predicate { predicate, .. }) = probe else { return false };
    columns.iter().enumerate().any(|(at, col)| at != predicate.column && !col.not_null)
}

/// How many initial key columns a comparison probe excludes nulls from.
fn tested(probe: &Probe) -> usize {
    match probe {
        Probe::All | Probe::Predicate { .. } | Probe::Xml(_) => 0,
        Probe::Columns(each) => (0..each.len()).take_while(|&at| each.iter().any(|c| c.column == at)).count(),
        Probe::Compare { .. } => 1,
        Probe::Prefix { equal, then } => equal.len().saturating_add(usize::from(then.is_some())),
        Probe::Range { equal, .. } => equal.len().saturating_add(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::QualifiedName;
    use crate::functions::Registry;

    /// Verifies: [DESIGN] D34: a function lookup cannot use an index that
    /// might omit a matching row. MC/DC pairs start with a nullable key
    /// column outside the call: storing null keys, testing that column or
    /// making it NOT NULL each independently permits the lookup. The last
    /// two also prevent surviving nulls, allowing the index to supply order.
    #[test]
    fn nullable_untested_columns_require_null_entries() {
        let mut ix = IndexDesc {
            name: "I".into(),
            table: QualifiedName { auth: "A".into(), ident: "T".into() },
            columns: vec![0, 1],
            method: "test".into(),
            unique: false,
            options: Vec::new(),
            collations: Vec::new(),
            nulls: false,
            expressions: Vec::new(),
        };
        let mut column = ColumnDesc {
            name: "K".into(),
            data_type: DataType::Boolean,
            not_null: false,
            default: None,
            domain: None,
            collation: None,
            generated: None,
        };
        let tested_column = ColumnDesc { not_null: true, ..column.clone() };
        let function = Registry::with_set_functions().scalar("UPPER").unwrap();
        let probe = |at| Probe::Predicate {
            predicate: Box::new(Predicate { function: function.clone(), column: at, argument: Some(0) }),
            value: Value::Null,
        };
        assert!(omits_rows(&ix, &[&tested_column, &column], Some(&probe(0))));
        assert!(untested_nulls(&[&tested_column, &column], Some(&probe(0))));
        ix.nulls = true;
        assert!(!omits_rows(&ix, &[&tested_column, &column], Some(&probe(0))), "index holds nulls");
        ix.nulls = false;
        assert!(!omits_rows(&ix, &[&tested_column, &column], Some(&probe(1))), "column tested");
        assert!(!untested_nulls(&[&tested_column, &column], Some(&probe(1))));
        column.not_null = true;
        assert!(!omits_rows(&ix, &[&tested_column, &column], Some(&probe(0))), "NOT NULL");
        assert!(!untested_nulls(&[&tested_column, &column], Some(&probe(0))));
    }
}
