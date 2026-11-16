//! \[DESIGN\] Index lookups for WHERE function calls such as `f(column, ?)`
//! (RULES.md D34). These helpers describe the lookup without calling the
//! function. The executor checks the full WHERE condition on fetched rows.

use super::Candidate;
use crate::catalog::{ColumnDesc, IndexDesc};
use crate::index::{IndexMethod, Predicate, Probe};
use crate::logical::{Cond, Expr, Slot, filled_expr};
use crate::physical::{Place, ProbeSlot};
use crate::value::{DataType, Value};

/// Whether this is a function lookup, requiring the full WHERE check.
pub(super) fn named(probes: &[Probe]) -> bool {
    matches!(probes.first(), Some(Probe::Predicate { .. }))
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
            let Cond::Value(Expr::Call(function, args)) = c else { return None };
            if c.can_raise() {
                return None;
            }
            let (column, argument, other) = operands(args)?;
            let column = ix.columns.iter().position(|&key_column| key_column == column)?;
            let (value, slot) = constant(other, values)?;
            let predicate = Box::new(Predicate { function: function.clone(), column, argument });
            let probe = Probe::Predicate { predicate, value };
            let slots = slot.into_iter().map(|slot| ProbeSlot { place: Place::PredicateValue, slot }).collect();
            // D34: even an exact index leaves the full WHERE check in place.
            Some(Candidate { slots, ..Candidate::one(probe, (vec![at], 0, all, true)) })
        })
        .collect()
}

/// Returns the table column number, its position in the call and the other
/// argument. Order matters: `f(column, value)` need not equal `f(value, column)`.
fn operands(args: &[Expr]) -> Option<(usize, usize, &Expr)> {
    match args {
        [Expr::Column(column), other] => Some((*column, 0, other)),
        [other, Expr::Column(column)] => Some((*column, 1, other)),
        _ => None,
    }
}

/// Gets the constant or current parameter value, retaining its slot for
/// later executions. Plan filling reports any failed parameter conversion.
fn constant(expr: &Expr, values: Option<&[Value]>) -> Option<(Value, Option<Slot>)> {
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
        Probe::Compare { op, .. } => method.supports(*op),
        Probe::Prefix { equal, then } => method.supports_prefix(types.len(), equal.len(), then.as_ref().map(|(op, _)| *op)),
        Probe::Range { equal, .. } => method.supports_range(types.len(), equal.len()),
        Probe::Predicate { predicate, .. } => method.supports_predicate(predicate, types),
    }
}

/// Whether omitting null keys could hide a matching row. A function call
/// rejects nulls in its argument column, but may leave other key columns null.
pub(super) fn omits_rows(ix: &IndexDesc, columns: &[&ColumnDesc], probe: Option<&Probe>) -> bool {
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
        Probe::All | Probe::Predicate { .. } => 0,
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
            predicate: Box::new(Predicate { function: function.clone(), column: at, argument: 0 }),
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
