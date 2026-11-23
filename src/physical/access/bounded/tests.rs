//! Unit tests of which index candidates the scan of their conjuncts bounds
//! (RULES.md D839).

use super::*;
use crate::catalog::QualifiedName;
use crate::index::{BTREE_METHOD, Probe};
use crate::value::{CompOp, Decimal, Value};

/// An index on the first two columns of T, unique if `unique`.
fn index(unique: bool) -> IndexDesc {
    IndexDesc {
        name: "T_K".to_owned(),
        table: QualifiedName { auth: "S".into(), ident: "T".into() },
        columns: vec![0, 1],
        method: BTREE_METHOD.to_owned(),
        unique,
        options: Vec::new(),
        collations: Vec::new(),
        nulls: false,
        expressions: Vec::new(),
    }
}

/// A candidate of `probe` holding `fixed` columns equal, `loose` if it only
/// narrows the rows.
fn candidate(probe: Probe, fixed: usize, loose: bool) -> Candidate {
    Candidate::one(probe, (vec![0], fixed, 10.0_f64, loose))
}

fn one() -> Value {
    Value::Exact(Decimal::new(1, 0))
}

/// The probe of the first column equal to 1.
fn prefix() -> Probe {
    Probe::Prefix { equal: vec![one()], then: None }
}

/// Verifies: [DESIGN] RULES.md D839: a range holding a prefix of a unique
/// key equal, its conjuncts answered exactly, is bounded by the scan of
/// them, and so is one of an index that is not unique but holds every
/// column read. MC/DC pairs, each against the first: no column held equal,
/// a loose range, an index neither unique nor covering, a probe that is
/// not of keys.
#[test]
fn a_prefix_range_of_a_unique_or_covering_index_is_bounded() {
    assert!(bounded(&index(true), &candidate(prefix(), 1, false), false));
    assert!(bounded(&index(false), &candidate(prefix(), 1, false), true), "covering");
    let range = Probe::Compare { op: CompOp::Gt, value: one() };
    assert!(!bounded(&index(true), &candidate(range, 0, false), false), "no column held equal");
    assert!(!bounded(&index(true), &candidate(prefix(), 1, true), false), "loose");
    assert!(!bounded(&index(false), &candidate(prefix(), 1, false), false), "neither unique nor covering");
    assert!(!bounded(&index(true), &candidate(Probe::All, 1, false), false), "not of keys");
}

/// A path of `total` cost reading `est` rows, bounded by the scan of its
/// conjuncts if `bounded`.
fn path(total: f64, est: f64, bounded: bool) -> Path {
    Path {
        total,
        cost: total,
        est,
        ordered: false,
        reverse: false,
        index: None,
        order: Vec::new(),
        note: String::new(),
        windowed: None,
        bounded,
    }
}

/// Verifies: [DESIGN] RULES.md D839: a bounded range priced as the scan is
/// taken where it is estimated to read fewer rows than the scan and fewer
/// than the table's rows over the margin. MC/DC pairs, each against the
/// first: not bounded, dearer than the scan, as many rows as the scan, the
/// margin's share of the table exactly.
#[test]
fn a_bounded_range_is_taken_at_a_tie_where_it_reads_fewer_rows() {
    let scan = path(800.0_f64, 1000.0_f64, false);
    let of = (8.0_f64, 10_000.0_f64);
    assert!(fewer_alike(&path(800.0_f64, 900.0_f64, true), &scan, of));
    assert!(!fewer_alike(&path(800.0_f64, 900.0_f64, false), &scan, of), "not bounded");
    assert!(!fewer_alike(&path(800.5_f64, 900.0_f64, true), &scan, of), "dearer");
    assert!(!fewer_alike(&path(800.0_f64, 1000.0_f64, true), &scan, of), "as many rows");
    assert!(!fewer_alike(&path(800.0_f64, 900.0_f64, true), &scan, (8.0_f64, 7_200.0_f64)), "the margin's share exactly");
}
