//! \[DESIGN\] Named predicate candidates and the capabilities that make
//! an index path complete (RULES.md D34). No function is evaluated while
//! planning; a candidate keeps its whole condition for the executor.

use super::Candidate;
use crate::catalog::{ColumnDesc, IndexDesc};
use crate::index::{IndexMethod, Predicate, Probe};
use crate::logical::{Cond, Expr, Slot, filled_expr};
use crate::physical::{Place, ProbeSlot};
use crate::value::{DataType, Value};

/// Whether the path needs the whole predicate rechecked on fetched rows.
pub(super) fn named(probes: &[Probe]) -> bool {
    matches!(probes.first(), Some(Probe::Predicate { .. }))
}

/// The calls of a column and a constant or slot among `conds`, over the
/// index's raw values. A sort key is no argument of the bound function.
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
            // `access` retains this conjunct even when the index is exact.
            Some(Candidate { slots, ..Candidate::one(probe, (vec![at], 0, all, true)) })
        })
        .collect()
}

/// The column's two places, and the other argument. Argument order is
/// preserved; a method declares each position it supports.
fn operands(args: &[Expr]) -> Option<(usize, usize, &Expr)> {
    match args {
        [Expr::Column(column), other] => Some((*column, 0, other)),
        [other, Expr::Column(column)] => Some((*column, 1, other)),
        _ => None,
    }
}

/// The same value the bound call receives, with its slot if one supplies
/// it. Failed slot conversions are reported by ordinary plan filling.
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
        | Expr::NextValue(_) => None,
    }
}

/// Whether the method answers the probe on these types. Support for a
/// predicate promises every value of its other argument, not one sample.
pub(super) fn supports(method: &dyn IndexMethod, probe: &Probe, (types, ordering): (&[DataType], bool)) -> bool {
    match probe {
        Probe::All => method.ordered() && ordering,
        Probe::Compare { op, .. } => method.supports(*op),
        Probe::Prefix { equal, then } => method.supports_prefix(types.len(), equal.len(), then.as_ref().map(|(op, _)| *op)),
        Probe::Range { equal, .. } => method.supports_range(types.len(), equal.len()),
        Probe::Predicate { predicate, .. } => method.supports_predicate(predicate, types),
    }
}

/// Whether a true row could have no index entry because another column
/// of its key is null. The predicate itself rejects its column's nulls.
pub(super) fn omits_rows(ix: &IndexDesc, columns: &[&ColumnDesc], probe: Option<&Probe>) -> bool {
    if matches!(probe, Some(Probe::Predicate { .. })) {
        return !ix.nulls && untested_nulls(columns, probe);
    }
    columns.get(probe.map_or(0, tested)..).unwrap_or_default().iter().any(|col| !col.not_null)
}

/// Whether a named probe may keep a key with a null. Such keys need an
/// index that holds nulls, and give no SQL null order without a sort (D34).
pub(super) fn untested_nulls(columns: &[&ColumnDesc], probe: Option<&Probe>) -> bool {
    let Some(Probe::Predicate { predicate, .. }) = probe else { return false };
    columns.iter().enumerate().any(|(at, col)| at != predicate.column && !col.not_null)
}

/// How many initial key columns a comparison probe excludes nulls from.
fn tested(probe: &Probe) -> usize {
    match probe {
        Probe::All | Probe::Predicate { .. } => 0,
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

    /// Verifies: [DESIGN] D34: a named predicate cannot use an index that
    /// omits a possible true row. MC/DC pairs: from an untested nullable
    /// column, change only the index holding nulls, the column being
    /// tested or the column being NOT NULL; each alone permits the path.
    /// The latter two pairs also decide whether nulls can survive, so the
    /// named probe may claim key order without a SQL sort.
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
