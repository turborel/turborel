use super::*;
use crate::ast::ColumnSpecification;

/// A grouping column reference to the column `name`.
fn column(name: &str) -> GroupingColumn {
    GroupingColumn { column: ColumnSpecification { qualifier: None, column: name.to_owned(), catalog: None, at: 0 }, collate: None }
}

/// The ordinary grouping set of the columns `names`.
fn set(names: &[&str]) -> Vec<GroupingColumn> {
    names.iter().map(|n| column(n)).collect()
}

/// A GROUP BY ALL of `elements`.
fn by(elements: Vec<GroupingElement>) -> GroupBy {
    GroupBy { distinct: false, elements }
}

/// The grouping sets of `g`, each its columns' names joined, or the
/// primitive columns' prefixed by "primitive".
fn names(g: &GroupBy) -> Vec<String> {
    let joined = |s: &Set<'_>| s.iter().map(|c| c.column.column.as_str()).collect::<Vec<_>>().join(",");
    match expand(g).unwrap() {
        Expansion::Primitive(s) => vec![format!("primitive {}", joined(&s))],
        Expansion::Sets(sets) => sets.iter().map(joined).collect(),
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 SR9, SR10] a simple
/// GROUP BY is primitive, its parentheses and empty grouping sets erased;
/// one ROLLUP makes it not simple.
#[test]
fn simple_group_by_loses_its_parentheses() {
    let g = by(vec![GroupingElement::Ordinary(set(&["A", "B"])), GroupingElement::Empty, GroupingElement::Ordinary(set(&["C"]))]);
    assert_eq!(names(&g), ["primitive A,B,C"]);
    assert_eq!(names(&by(vec![GroupingElement::Empty, GroupingElement::Empty])), ["primitive "]);
    let rolled = by(vec![GroupingElement::Ordinary(set(&["C"])), GroupingElement::Rollup(vec![set(&["A"])])]);
    assert_eq!(names(&rolled), ["C,A", "C"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 SR13] ROLLUP ((A, B),
/// (C, D)) is GROUPING SETS ((A, B, C, D), (A, B), ()), NOTE 265's example.
#[test]
fn rollup_drops_sets_from_the_right() {
    let g = by(vec![GroupingElement::Rollup(vec![set(&["A", "B"]), set(&["C", "D"])])]);
    assert_eq!(names(&g), ["A,B,C,D", "A,B", ""]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 SR14] CUBE (A, B, C)
/// and CUBE ((A, B), (C, D)) are NOTE 266's GROUPING SETS, in its order.
#[test]
fn cube_takes_every_subset_by_its_bits() {
    let g = by(vec![GroupingElement::Cube(vec![set(&["A"]), set(&["B"]), set(&["C"])])]);
    assert_eq!(names(&g), ["A,B,C", "A,B", "A,C", "A", "B,C", "B", "C", ""]);
    let pairs = by(vec![GroupingElement::Cube(vec![set(&["A", "B"]), set(&["C", "D"])])]);
    assert_eq!(names(&pairs), ["A,B,C,D", "A,B", "C,D", ""]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §7.13 SR15, SR16] nested
/// GROUPING SETS are promoted, and two are concatenated, each set of the
/// first with each of the second, NOTE 272's example.
#[test]
fn grouping_sets_are_promoted_and_concatenated() {
    let first = GroupingElement::Sets(vec![GroupingElement::Ordinary(set(&["A", "B"])), GroupingElement::Ordinary(set(&["C"]))]);
    let second = GroupingElement::Sets(vec![GroupingElement::Ordinary(set(&["X", "Y"])), GroupingElement::Empty]);
    assert_eq!(names(&by(vec![first.clone(), second])), ["A,B,X,Y", "A,B", "C,X,Y", "C"]);
    let nested = GroupingElement::Sets(vec![GroupingElement::Ordinary(set(&["D"])), first, GroupingElement::Rollup(vec![set(&["E"])])]);
    assert_eq!(names(&by(vec![nested])), ["D", "A,B", "C", "E", ""]);
}

/// Verifies: [DESIGN] a GROUP BY standing for more than MAX_GROUPING_SETS
/// grouping sets is refused, by a CUBE or by a concatenation; one of
/// exactly that many is not.
#[test]
fn grouping_sets_past_the_limit_are_refused() {
    let singles = |n: usize| (0..n).map(|i| set(&[&format!("C{i}")])).collect::<Vec<_>>();
    let cube = |n: usize| by(vec![GroupingElement::Cube(singles(n))]);
    assert!(matches!(expand(&cube(12)), Ok(Expansion::Sets(s)) if s.len() == MAX_GROUPING_SETS));
    assert_eq!(expand(&cube(13)).err().map(|e| e.code), Some(Code::Unsupported));
    assert_eq!(expand(&cube(200)).err().map(|e| e.code), Some(Code::Unsupported));
    let doubled = by(vec![GroupingElement::Cube(singles(12)), GroupingElement::Rollup(singles(1))]);
    assert_eq!(expand(&doubled).err().map(|e| e.code), Some(Code::Unsupported));
}
