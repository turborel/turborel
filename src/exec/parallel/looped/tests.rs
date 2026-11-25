//! Unit tests of which index joins probe on a part's threads (RULES.md
//! D1114): the degree of the parts their left operands hold, and the left
//! operand taken out below every join that takes part.

use super::*;
use crate::catalog::QualifiedName;
use crate::logical::{Cond, Expr, Slot};
use crate::physical::{Parallel, Unmatched};
use crate::value::{CompOp, DataType, Spelled};

/// A part run in parallel of `degree`, of no rows.
fn part(degree: usize) -> Plan {
    Plan::Parallel(Box::new(Parallel { degree, input: values("part") }))
}

/// No rows, named `detail`.
fn values(detail: &str) -> Plan {
    Plan::Values { rows: Vec::new(), detail: detail.to_owned() }
}

/// The text of `plan`, rows given by value.
fn detail_of(plan: &Plan) -> &str {
    let Plan::Values { detail, .. } = plan else { panic!("{plan:?}") };
    detail
}

/// An index join of `left` by its column 0, its rest `cond`, of rows kept
/// as `outer` says.
fn index_join(outer: Preserved, cond: Option<Cond>, left: Plan) -> Plan {
    Plan::IndexJoin {
        outer,
        spatial: None,
        left: Box::new(left),
        table: QualifiedName { auth: "A".to_owned(), ident: "T".to_owned() },
        index: "T_K".into(),
        method: "btree".into(),
        probe: JoinProbe::Prefix(vec![0].into_boxed_slice()),
        cond,
        width: 1,
        columns: vec![0].into_boxed_slice(),
        detail: "".into(),
        est: 1.0_f64,
        cost: 1.0_f64,
        located: false,
    }
}

/// A condition a worker evaluates alone, or, not `calm`, one of a
/// parameter's value, which it does not.
fn cond(calm: bool) -> Cond {
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    let other = if calm { Expr::Column(1) } else { Expr::Dynamic(Slot { index: 0, data_type: integer, at: 0 }) };
    Cond::Compare(Expr::Column(0), CompOp::Ne, other)
}

/// Verifies: [DESIGN] (RULES.md D1114) an inner, LEFT, semi or antijoin
/// by an index probes on the threads of the greatest part its left
/// operand holds, below every index join of the chain that takes part;
/// not of a left operand holding none, of a FULL join, or of a condition
/// a worker cannot evaluate alone, which leaves the joins below to their
/// own chain.
#[test]
fn an_index_join_over_a_part_probes_on_its_threads() {
    for outer in [Preserved::Inner, Preserved::Outer, Preserved::Semi, Preserved::Anti] {
        assert_eq!(loop_degree(&mut index_join(outer, Some(cond(true)), part(6))), 6);
    }
    let both = Preserved::Both(Box::new(Unmatched { filter: None, width: 1 }));
    assert_eq!(loop_degree(&mut index_join(both, None, part(6))), 1, "a FULL join");
    assert_eq!(loop_degree(&mut index_join(Preserved::Inner, Some(cond(false)), part(6))), 1, "of a parameter");
    assert_eq!(loop_degree(&mut index_join(Preserved::Inner, None, values("rows"))), 1, "of no part");
    let union = Plan::Append { left: Box::new(part(3)), right: Box::new(part(5)), est: 0.0_f64 };
    let mut chain = index_join(Preserved::Semi, None, index_join(Preserved::Inner, None, union));
    assert_eq!(loop_degree(&mut chain), 5, "the greatest part below the chain");
    let mut uncalm = index_join(Preserved::Semi, None, index_join(Preserved::Inner, Some(cond(false)), part(4)));
    assert_eq!(loop_degree(&mut uncalm), 4, "the join below is the left operand");
    assert!(matches!(leaf(&mut uncalm, 0), Plan::IndexJoin { .. }), "taken out whole");
}

/// Verifies: [DESIGN] (RULES.md D1114) the left operand taken out of a
/// chain is that of its lowest join that takes part, an empty set of rows
/// left in its place, the joins above kept.
#[test]
fn the_left_operand_of_the_lowest_join_is_taken_out() {
    let mut chain = index_join(Preserved::Semi, None, index_join(Preserved::Inner, None, values("left")));
    assert_eq!(detail_of(&leaf(&mut chain, 0)), "left");
    let Plan::IndexJoin { left, .. } = &chain else { panic!("{chain:?}") };
    let Plan::IndexJoin { left: lowest, .. } = &**left else { panic!("{chain:?}") };
    assert_eq!(detail_of(lowest), "", "an empty set of rows in its place");
    assert_eq!(detail_of(&leaf(&mut values("alone"), 0)), "", "of no join, none");
}
