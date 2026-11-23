//! \[DESIGN\] The values of a compiled program (RULES.md D839): a leaf
//! that is one row of values reading no table is evaluated where it is,
//! over the frame's slots; a query leaf that plans to one point read of a
//! whole unique key is read as that point read; any other leaf is run by
//! the session as the interpreter runs it. And the targets a value is
//! stored in: a variable or parameter as a whole is its slot, found once.

use std::sync::{Arc, OnceLock};

use crate::ast::{ParameterSpecification, TargetKind};
use crate::catalog::QualifiedName;
use crate::error::{At, Result};
use crate::logical::{Expr, Leaf, LogicalPlan, Target};
use crate::params::RoutineScope;
use crate::physical::Kept;
use crate::value::DataType;

/// A value, condition or query of a compiled program.
#[derive(Debug)]
pub(super) enum Valued {
    /// One row of values, each of its operands alone, over the frame.
    Row(Box<RowOf>),
    /// A query, read as one point read once it plans to one.
    Query(Box<Queried>),
    /// Any other leaf, run by the session.
    General(Leaf),
}

/// A row of values evaluated where it is: the leaf's outer expressions,
/// over the frame, then its row over them.
#[derive(Debug)]
pub(super) struct RowOf {
    pub(super) outer: Vec<Expr>,
    pub(super) row: Vec<Expr>,
    pub(super) at: usize,
}

/// A query leaf, and its point read, planned as it is first run.
#[derive(Debug)]
pub(super) struct Queried {
    pub(super) leaf: Leaf,
    /// The columns of each table it reads, which it locks as the session
    /// does before it reads.
    pub(super) reads: Vec<(QualifiedName, u64)>,
    pub(super) point: OnceLock<Option<Arc<Kept>>>,
}

impl Valued {
    /// The leaf `leaf` compiled: a row of values that reads nothing is
    /// evaluated where it is; any other leaf runs as the session runs it.
    pub(super) fn of(leaf: &Leaf) -> Valued {
        let query = &leaf.query;
        let LogicalPlan::Values { rows, .. } = &query.plan else { return Valued::General(leaf.clone()) };
        let [row] = rows.as_slice() else { return Valued::General(leaf.clone()) };
        let own = row.iter().chain(&query.outer).all(Expr::reads_nothing);
        if !own || !query.plan.tables().is_empty() {
            return Valued::General(leaf.clone());
        }
        Valued::Row(Box::new(RowOf { outer: query.outer.clone(), row: row.clone(), at: leaf.at }))
    }

    /// The query leaf `leaf` of a SELECT INTO compiled: read as a point
    /// read once it plans to one.
    pub(super) fn of_query(leaf: &Leaf) -> Valued {
        if let LogicalPlan::Values { .. } = &leaf.query.plan {
            return Valued::of(leaf);
        }
        let reads = leaf.query.plan.columns_read();
        Valued::Query(Box::new(Queried { leaf: leaf.clone(), reads, point: OnceLock::new() }))
    }
}

/// Where each value of a row is stored.
#[derive(Debug)]
pub(super) struct Stores(pub(super) Vec<Store>);

/// Where one value is stored.
#[derive(Debug)]
pub(super) enum Store {
    /// [ISO/IEC 9075-2:2023 §9.2] An SQL variable or parameter as a whole:
    /// its slot and declared type, and where the target is written.
    Slot { at: usize, data_type: DataType, written: usize },
    /// Any other target, assigned as the interpreter assigns it.
    General,
}

impl Stores {
    /// Of an assignment statement's targets `targets`, named in `scope`.
    pub(super) fn of_targets(targets: &[Target], scope: &RoutineScope) -> Stores {
        Stores(targets.iter().map(|t| if t.fields.is_empty() { store(&t.spec, scope) } else { Store::General }).collect())
    }

    /// Of SELECT INTO's targets `specs`, named in `scope`.
    pub(super) fn of_specs(specs: &[ParameterSpecification], scope: &RoutineScope) -> Stores {
        Stores(specs.iter().map(|s| store(s, scope)).collect())
    }

    /// Whether every target is a slot.
    pub(super) fn all_slots(&self) -> bool {
        self.0.iter().all(|s| matches!(s, Store::Slot { .. }))
    }
}

/// Where the target `spec`, named in `scope`, stores its value.
fn store(spec: &ParameterSpecification, scope: &RoutineScope) -> Store {
    let whole = spec.kind == TargetKind::Routine && spec.element.is_none() && spec.indicator.is_none();
    match scope.parameter(spec.qualifier.as_deref(), &spec.name).filter(|_| whole) {
        Some((at, data_type)) => Store::Slot { at, data_type, written: spec.at },
        None => Store::General,
    }
}

/// [ISO/IEC 9075-2:2023 §9.2] `value` stored in the slot `store`, as the
/// SQL variable or parameter it is takes it.
///
/// # Errors
///
/// Store assignment's, at where the target is written.
pub(super) fn stored(value: crate::value::Value, (data_type, written): (&DataType, usize)) -> Result<crate::value::Value> {
    crate::value::assign(value, data_type, "ISO/IEC 9075-2:2023 §9.2").or_at(written)
}
