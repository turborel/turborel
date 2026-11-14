//! \[DESIGN\] Physical plans kept between the executions of a prepared
//! statement (RULES.md D45). A probe that fixes every column of a unique
//! index finds one row at most, so it is chosen without pricing another
//! path (`physical/access.rs`), by constants and by the statement's `?`s
//! alike: a null stands in a probe for each `?` until an execution fills
//! it. A plan of such a probe alone, under a filter or a projection, is
//! the plan every execution would make, whatever its values, so it is made
//! once, with its slots, and kept; each execution fills the slots, as
//! `logical/dynamic.rs` fills a logical plan's. Every other plan is chosen
//! by its values' statistics: a planner given an execution's values may
//! still make it with the slots, each `?` compared with a column standing
//! in its probe for the value (`ProbeSlot`), so another execution can fill
//! it, and the statement keeps such a plan once its executions keep making
//! one of the same shape (`kept/shape.rs`).

use super::Plan;
use super::access::{ix_is_point, whole_key};
use super::probe::{Compared, placed_as_is};
use crate::db::State;
use crate::error::{Code, Result, fail};
use crate::index::Probe;
use crate::logical::{AggCall, Cond, Expr, Slot, filled_cond, filled_expr};
use crate::value::{CompOp, DataType, Value};

mod shape;

pub(crate) use shape::same_shape;

/// A `?` of a probe: the slot, and the place among the probe's values its
/// value takes.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeSlot {
    /// The place among the probe's values.
    pub place: Place,
    /// The slot, whose type the value is cast to.
    pub slot: Slot,
}

/// Where a `?`'s value goes in a probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// The value compared with the index's column of this place, from 0:
    /// a `Probe::Compare`'s, one of the equal values of a `Probe::Prefix`
    /// or a `Probe::Range`, or the comparison after a prefix's.
    Key(usize),
    /// The lower bound of a `Probe::Range`.
    Low,
    /// The upper bound of a `Probe::Range`.
    High,
}

/// The probe fixing every column of an index on `columns`, of `types`, by
/// the equalities among `conds`: of a constant, one of `compared`, placed
/// as the index places it; or of a `?`, a null standing in for its value,
/// its slot noted. With the conjuncts it answers. None if a column has
/// neither.
pub(super) fn point_probe(
    conds: &[Cond],
    compared: &[Compared],
    (columns, types): (&[usize], &[DataType]),
) -> Option<(Probe, Vec<ProbeSlot>, Vec<usize>)> {
    let (mut equal, mut slots, mut used) = (Vec::new(), Vec::new(), Vec::new());
    for (at, (&column, data_type)) in columns.iter().zip(types).enumerate() {
        if let Some((value, answered)) = constant_equal(compared, column, data_type) {
            slots.extend(answered.slot_at(Place::Key(at)));
            equal.push(value);
            used.push(answered.at);
        } else if let Some((slot, answered)) = slot_equal(conds, column, data_type) {
            equal.push(Value::Null);
            slots.push(ProbeSlot { place: Place::Key(at), slot });
            used.push(answered);
        } else {
            return None;
        }
    }
    // An index of one column is asked by a comparison, which every method
    // answers that supports equality.
    let probe = match (equal.as_slice(), columns.len()) {
        ([only], 1) => Probe::Compare { op: CompOp::Eq, value: only.clone() },
        _ => Probe::Prefix { equal, then: None },
    };
    Some((probe, slots, used))
}

/// The value a constant among `compared` holds `column`, of `data_type`,
/// equal to, placed as an index on it places it, and its comparison: of a
/// `?`'s value, only as it is (`Compared::placed`).
fn constant_equal<'c>(compared: &'c [Compared], column: usize, data_type: &DataType) -> Option<(Value, &'c Compared)> {
    compared.iter().filter(|c| c.column == column && c.exact && c.op == CompOp::Eq).find_map(|c| {
        let placed = c.placed(data_type)?;
        match placed.as_slice() {
            [(CompOp::Eq, value)] => Some((value.clone(), c)),
            // A value between two of the column's equals none of them.
            _ => None,
        }
    })
}

/// The slot a conjunct of `conds` holds `column`, of `data_type`, equal
/// to, if its type's values are placed in an index on the column as they
/// are, and its conjunct.
fn slot_equal(conds: &[Cond], column: usize, data_type: &DataType) -> Option<(Slot, usize)> {
    conds.iter().enumerate().find_map(|(at, c)| {
        let slot = match c {
            Cond::Compare(Expr::Column(col), CompOp::Eq, Expr::Dynamic(slot))
            | Cond::Compare(Expr::Dynamic(slot), CompOp::Eq, Expr::Column(col))
                if *col == column =>
            {
                slot
            }
            Cond::Compare(..)
            | Cond::Distinct(..)
            | Cond::Const(_)
            | Cond::Or(..)
            | Cond::And(..)
            | Cond::Not(_)
            | Cond::Is(..)
            | Cond::IsNull(..)
            | Cond::In(..)
            | Cond::Like(..)
            | Cond::Exists(_)
            | Cond::Unique(..)
            | Cond::Quantified(..)
            | Cond::Match { .. }
            | Cond::Value(_) => return None,
        };
        placed_as_is(&slot.data_type, data_type).then_some((slot.clone(), at))
    })
}

/// Whether `plan` is the plan every execution of its statement would make,
/// whatever its values: one probe fixing every column of a unique index,
/// under a filter or a projection or both, or none. The index is read from
/// `st`, the state it was planned in.
pub(crate) fn keepable(plan: &Plan, st: &State) -> bool {
    match plan {
        Plan::Project { input, .. } | Plan::Filter { input, .. } => keepable(input, st),
        Plan::IndexScan { index, probe, .. } => {
            st.catalog.index(index).is_some_and(|ix| ix_is_point(ix) && whole_key(probe, ix.columns.len()))
        }
        Plan::SeqScan { .. }
        | Plan::Product { .. }
        | Plan::HashJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::Aggregate { .. }
        | Plan::Distinct { .. }
        | Plan::SetOp { .. }
        | Plan::System { .. }
        | Plan::Values { .. }
        | Plan::Append { .. }
        | Plan::Sort { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::WorkTable { .. } => false,
    }
}

/// `plan`, a kept plan (`keepable`), its slots filled with `values`: those
/// of its projection's expressions and its filter's condition, and those
/// of its probe.
///
/// # Errors
///
/// As `logical::filled`; SQLCODE -910 for an operator no kept plan holds.
pub(crate) fn filled_plan(plan: Plan, values: &[Value]) -> Result<Plan> {
    match plan {
        Plan::Project { exprs, detail, input } => {
            let exprs = exprs.into_iter().map(|e| filled_expr(e, values)).collect::<Result<_>>()?;
            Ok(Plan::Project { exprs, detail, input: Box::new(filled_plan(*input, values)?) })
        }
        Plan::Filter { cond, detail, est, input } => {
            let cond = filled_cond(cond, values)?;
            Ok(Plan::Filter { cond, detail, est, input: Box::new(filled_plan(*input, values)?) })
        }
        Plan::IndexScan { .. } => filled_scan(plan, values),
        Plan::SeqScan { .. } => Ok(plan),
        Plan::Aggregate { groups, collations, aggs, seeds, detail, input } => {
            let filled_agg = |a: AggCall| {
                let arg = a.arg.map(|e| filled_expr(e, values)).transpose()?;
                Ok(AggCall { arg, filter: a.filter.map(|c| filled_cond(c, values)).transpose()?, ..a })
            };
            let aggs = aggs.into_iter().map(filled_agg).collect::<Result<_>>()?;
            Ok(Plan::Aggregate { groups, collations, aggs, seeds, detail, input: Box::new(filled_plan(*input, values)?) })
        }
        Plan::Distinct { input } => Ok(Plan::Distinct { input: Box::new(filled_plan(*input, values)?) }),
        Plan::Sort { keys, detail, input } => Ok(Plan::Sort { keys, detail, input: Box::new(filled_plan(*input, values)?) }),
        Plan::Append { left, right } => {
            let left = Box::new(filled_plan(*left, values)?);
            Ok(Plan::Append { left, right: Box::new(filled_plan(*right, values)?) })
        }
        Plan::Product { .. }
        | Plan::HashJoin { .. }
        | Plan::IndexJoin { .. }
        | Plan::OuterJoin { .. }
        | Plan::SetOp { .. }
        | Plan::System { .. }
        | Plan::Values { .. }
        | Plan::Recursive { .. }
        | Plan::Unnest(..)
        | Plan::Window { .. }
        | Plan::WorkTable { .. } => fail(Code::Api, "[DESIGN] a kept plan holds an operator no kept plan holds"),
    }
}

/// `plan`, an index scan, each `?` of its probe filled with its value.
fn filled_scan(mut plan: Plan, values: &[Value]) -> Result<Plan> {
    if let Plan::IndexScan { probe, slots, .. } = &mut plan {
        for s in slots.iter() {
            place(probe, s, values)?;
        }
    }
    Ok(plan)
}

/// Puts the value of `s`'s slot, cast to its type as a logical plan's
/// slots are, in its place in `probe`.
///
/// # Errors
///
/// As `logical::filled`; SQLCODE -910 if the probe has no such place.
fn place(probe: &mut Probe, s: &ProbeSlot, values: &[Value]) -> Result<()> {
    let value = match filled_expr(Expr::Dynamic(s.slot.clone()), values)? {
        Expr::Const(value) => value,
        Expr::Column(_)
        | Expr::Dynamic(_)
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
        | Expr::NextValue(_) => return fail(Code::Api, "[DESIGN] a slot filled is a constant"),
    };
    let at = match (probe, s.place) {
        (Probe::Compare { value: first, .. }, Place::Key(n)) => (n == 0).then_some(first),
        (Probe::Prefix { equal, then }, Place::Key(n)) => {
            if n == equal.len() {
                then.as_mut().map(|(_, bound)| bound)
            } else {
                equal.get_mut(n)
            }
        }
        (Probe::Range { equal, .. }, Place::Key(n)) => equal.get_mut(n),
        (Probe::Range { low, .. }, Place::Low) => Some(&mut low.1),
        (Probe::Range { high, .. }, Place::High) => Some(&mut high.1),
        (Probe::All, _) | (Probe::Compare { .. } | Probe::Prefix { .. }, Place::Low | Place::High) => None,
    };
    match at {
        Some(at) => {
            *at = value;
            Ok(())
        }
        None => fail(Code::Api, "[DESIGN] a kept probe has no place for a <dynamic parameter specification>"),
    }
}

// Unit tests: plans made with no table, and the conditions of keeping one
// (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use super::*;
    use crate::catalog::{Catalog, ColumnDesc, IndexDesc, MemoryCatalog, QualifiedName, TableDesc};
    use crate::engine::rows::ROW_STORE;
    use crate::index::BTREE_METHOD;
    use crate::logical::Truth;
    use crate::value::collation::{CASE_INSENSITIVE, CollationRef, Pad, builtin};
    use crate::value::{Decimal, Spelled};

    const INT: DataType = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Numeric };

    fn int(n: i128) -> Value {
        Value::Exact(Decimal::new(n, 0))
    }

    fn slot(index: usize) -> Slot {
        Slot { index, data_type: INT, at: 3 }
    }

    fn table() -> QualifiedName {
        QualifiedName { auth: "A".into(), ident: "T".into() }
    }

    /// Whether `a` and `b` are written the same, of what compares no
    /// other way.
    fn same<T: Debug>(a: &T, b: &T) -> bool {
        format!("{a:?}") == format!("{b:?}")
    }

    /// A state whose catalog has the table `A.T` (K, L INTEGER) and on it
    /// the index `I` on (K, L), unique if `unique`, and `J` on K, by a
    /// collation.
    fn state(unique: bool) -> State {
        let mut catalog = MemoryCatalog::default();
        let column = |n: &str| ColumnDesc {
            name: n.into(),
            data_type: INT,
            not_null: true,
            default: None,
            domain: None,
            collation: None,
            generated: None,
        };
        let desc = TableDesc {
            name: table(),
            columns: vec![column("K"), column("L")],
            constraints: vec![],
            engine: ROW_STORE.into(),
            options: Vec::new(),
            generation: 0,
            temporary: None,
        };
        catalog.create_schema("A", "A", vec![desc]);
        let index = |n: &str, columns: Vec<usize>, collations: Vec<Option<CollationRef>>| IndexDesc {
            name: n.into(),
            table: table(),
            columns,
            method: BTREE_METHOD.into(),
            unique,
            options: Vec::new(),
            collations,
            nulls: false,
        };
        catalog.add_index(index("I", vec![0, 1], Vec::new()));
        let folded =
            builtin().into_iter().find(|c| c.name() == CASE_INSENSITIVE).map(|collation| CollationRef { collation, pad: Pad::Space });
        catalog.add_index(index("J", vec![0], vec![folded]));
        State::with_builtins(Box::new(catalog))
    }

    /// An index scan of `index` by `probe`, with `slots`.
    fn scan(index: &str, probe: Probe, slots: Vec<ProbeSlot>) -> Plan {
        Plan::IndexScan {
            table: table(),
            index: index.into(),
            method: BTREE_METHOD.into(),
            probe,
            slots,
            reverse: false,
            columns: vec![0, 1],
            detail: String::new(),
            est: 1.0_f64,
            cost: 1.0_f64,
            covering: false,
        }
    }

    /// A probe of the two values of the key of `I`.
    fn whole() -> Probe {
        Probe::Prefix { equal: vec![int(1), int(2)], then: None }
    }

    /// Verifies: [DESIGN] RULES.md D45: a plan is kept if it is one probe
    /// fixing every column of a unique index, under a filter or a
    /// projection or neither. MC/DC pairs, from such a probe kept: the
    /// index not unique; the index collated; the probe not of its whole
    /// key: each alone forbids it. A scan, a sort over the probe, or an
    /// index the catalog lacks is never kept.
    #[test]
    fn a_probe_of_a_whole_unique_key_alone_is_kept() {
        let st = state(true);
        assert!(keepable(&scan("I", whole(), Vec::new()), &st));
        let filtered = Plan::Filter {
            cond: Cond::Const(Truth::True),
            detail: String::new(),
            est: None,
            input: Box::new(scan("I", whole(), Vec::new())),
        };
        let projected = Plan::Project { exprs: vec![Expr::Column(0)], detail: String::new(), input: Box::new(filtered) };
        assert!(keepable(&projected, &st));
        assert!(!keepable(&scan("I", whole(), Vec::new()), &state(false)), "not unique");
        assert!(!keepable(&scan("J", Probe::Compare { op: CompOp::Eq, value: int(1) }, Vec::new()), &st), "collated");
        assert!(!keepable(&scan("I", Probe::Prefix { equal: vec![int(1)], then: None }, Vec::new()), &st), "a prefix of the key");
        assert!(!keepable(&scan("I", Probe::All, Vec::new()), &st), "every key");
        let seq = Plan::SeqScan { table: table(), columns: vec![0], rows: 0, cost: 0.0_f64, order: Vec::new(), backward: false };
        assert!(!keepable(&seq, &st));
        let sorted = Plan::Sort { keys: Vec::new(), detail: String::new(), input: Box::new(scan("I", whole(), Vec::new())) };
        assert!(!keepable(&sorted, &st));
        assert!(!keepable(&scan("X", whole(), Vec::new()), &st), "no such index");
    }

    /// Verifies: [DESIGN] RULES.md D45: a probe fixing a key by `?`s and a
    /// constant is made of the equalities of the key's columns, a `?`
    /// written on either side, a null standing in for each `?`; none if a
    /// column is not held equal, if it is held equal to a `?` of a type
    /// the index places otherwise than as it is, or by another comparison.
    #[test]
    fn a_point_probe_is_made_of_equalities_with_slots_and_constants() {
        let equal = |column: usize, e: Expr| Cond::Compare(Expr::Column(column), CompOp::Eq, e);
        let conds = vec![equal(1, Expr::Dynamic(slot(0))), Cond::Compare(Expr::Dynamic(slot(1)), CompOp::Eq, Expr::Column(0))];
        let both = point_probe(&conds, &[], (&[0, 1], &[INT, INT])).unwrap();
        assert!(same(&both.0, &Probe::Prefix { equal: vec![Value::Null, Value::Null], then: None }), "{both:?}");
        assert_eq!(both.1, vec![ProbeSlot { place: Place::Key(0), slot: slot(1) }, ProbeSlot { place: Place::Key(1), slot: slot(0) }]);
        assert_eq!(both.2, vec![1, 0]);
        let constant = Compared { collation: None, at: 1, exact: true, column: 0, op: CompOp::Eq, value: int(7), slot: None };
        let mixed = point_probe(&conds, std::slice::from_ref(&constant), (&[0, 1], &[INT, INT])).unwrap();
        assert!(same(&mixed.0, &Probe::Prefix { equal: vec![int(7), Value::Null], then: None }), "{mixed:?}");
        assert_eq!((mixed.1, mixed.2), (vec![ProbeSlot { place: Place::Key(1), slot: slot(0) }], vec![1, 0]));
        let one = point_probe(&conds, &[], (&[1], &[INT])).unwrap();
        assert!(same(&one.0, &Probe::Compare { op: CompOp::Eq, value: Value::Null }), "{one:?}");
        assert_eq!(one.1, vec![ProbeSlot { place: Place::Key(0), slot: slot(0) }]);
        assert!(point_probe(&conds[..1], &[], (&[0, 1], &[INT, INT])).is_none(), "K not held equal");
        let text = DataType::varying(4);
        assert!(point_probe(&conds, &[], (&[0, 1], &[INT, text])).is_none(), "a number in a text column");
        let less = Cond::Compare(Expr::Column(0), CompOp::Lt, Expr::Dynamic(slot(0)));
        assert!(point_probe(&[less], &[], (&[0], &[INT])).is_none(), "not an equality");
        let between =
            Compared { collation: None, at: 0, exact: true, column: 0, op: CompOp::Eq, value: Value::Approx(0.5_f64), slot: None };
        assert!(point_probe(&[], std::slice::from_ref(&between), (&[0], &[INT])).is_none(), "a number between the column's");
    }

    /// Verifies: [DESIGN] RULES.md D45: filling a kept plan puts each value,
    /// cast to its slot's type, in its place in the probe, and in the
    /// filter and projection over it, a range's equal values and bounds
    /// and a prefix's comparison among them; a value its slot's type takes
    /// none of fails as CAST fails; a `?` with no place in its probe, and a
    /// plan of an operator no kept plan holds, a join's, are refused.
    #[test]
    fn a_kept_plan_is_filled_with_each_executions_values() {
        let slots = vec![ProbeSlot { place: Place::Key(1), slot: slot(0) }];
        let standing = Probe::Prefix { equal: vec![int(7), Value::Null], then: None };
        let judged = Cond::Compare(Expr::Column(1), CompOp::Lt, Expr::Dynamic(slot(1)));
        let filtered = Plan::Filter { cond: judged, detail: String::new(), est: None, input: Box::new(scan("I", standing, slots)) };
        let plan = Plan::Project { exprs: vec![Expr::Dynamic(slot(1))], detail: String::new(), input: Box::new(filtered) };
        let filled = filled_plan(plan, &[Value::Char(" 12 ".into()), int(3)]).unwrap();
        let Plan::Project { exprs, input, .. } = filled else { panic!("{filled:?}") };
        assert!(same(&exprs, &vec![Expr::Const(int(3))]), "{exprs:?}");
        let Plan::Filter { cond, input, .. } = *input else { panic!("{input:?}") };
        assert!(same(&cond, &Cond::Compare(Expr::Column(1), CompOp::Lt, Expr::Const(int(3)))), "{cond:?}");
        let Plan::IndexScan { probe, .. } = *input else { panic!("{input:?}") };
        assert!(same(&probe, &Probe::Prefix { equal: vec![int(7), int(12)], then: None }), "the string cast to INTEGER: {probe:?}");
        let one = scan("I", Probe::Compare { op: CompOp::Eq, value: Value::Null }, vec![ProbeSlot { place: Place::Key(0), slot: slot(0) }]);
        let Plan::IndexScan { probe: first, .. } = filled_plan(one, &[int(5)]).unwrap() else { panic!("not a scan") };
        assert!(same(&first, &Probe::Compare { op: CompOp::Eq, value: int(5) }), "{first:?}");
        let failed =
            filled_plan(scan("I", Probe::All, vec![ProbeSlot { place: Place::Key(0), slot: slot(0) }]), &[Value::Char("X".into())])
                .unwrap_err();
        assert_eq!(failed.code, Code::Data, "CAST fails first");
        let misplaced = filled_plan(scan("I", Probe::All, vec![ProbeSlot { place: Place::Key(0), slot: slot(0) }]), &[int(1)]).unwrap_err();
        assert_eq!(misplaced.code, Code::Api);
        let ranged =
            Probe::Range { equal: vec![Value::Null], low: Box::new((CompOp::Ge, Value::Null)), high: Box::new((CompOp::Lt, Value::Null)) };
        let bounds = [Place::Key(0), Place::Low, Place::High].map(|place| ProbeSlot { place, slot: slot(0) });
        let Plan::IndexScan { probe: range, .. } = filled_plan(scan("I", ranged, bounds.to_vec()), &[int(4)]).unwrap() else {
            panic!("not a scan")
        };
        let filled_range = Probe::Range { equal: vec![int(4)], low: Box::new((CompOp::Ge, int(4))), high: Box::new((CompOp::Lt, int(4))) };
        assert!(same(&range, &filled_range), "{range:?}");
        let then = Probe::Prefix { equal: vec![int(1)], then: Some((CompOp::Gt, Value::Null)) };
        let Plan::IndexScan { probe: after, .. } =
            filled_plan(scan("I", then, vec![ProbeSlot { place: Place::Key(1), slot: slot(0) }]), &[int(6)]).unwrap()
        else {
            panic!("not a scan")
        };
        assert!(same(&after, &Probe::Prefix { equal: vec![int(1)], then: Some((CompOp::Gt, int(6))) }), "{after:?}");
        let low_of_prefix =
            scan("I", Probe::Prefix { equal: vec![int(1)], then: None }, vec![ProbeSlot { place: Place::Low, slot: slot(0) }]);
        assert_eq!(filled_plan(low_of_prefix, &[int(1)]).unwrap_err().code, Code::Api, "a bound of no range");
        let sorted = Plan::Sort { keys: Vec::new(), detail: String::new(), input: Box::new(scan("I", Probe::All, Vec::new())) };
        assert!(filled_plan(Plan::Distinct { input: Box::new(sorted) }, &[]).is_ok());
        let other = Plan::Product { left: Box::new(scan("I", Probe::All, Vec::new())), right: Box::new(scan("I", Probe::All, Vec::new())) };
        assert_eq!(filled_plan(other, &[]).unwrap_err().code, Code::Api);
    }
}
