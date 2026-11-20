//! \[DESIGN\] Which reorderings of non-inner joins keep a query's result
//! (RULES.md D301), by the conflict detector CD-C of Moerkotte, Fender
//! and Eich, "On the Correct and Complete Enumeration of the Core Search
//! Space", 2013, §5.4: the properties of each pair of operators (their
//! Tables 1 to 3) say whether associativity, left asscom or right asscom
//! holds of them, and each that does not becomes a conflict rule of the
//! upper operator, `T1 → T2`, which a join of the sets S1 and S2 obeys if
//! `T1 ∩ (S1 ∪ S2) ≠ ∅` implies `T2 ⊆ S1 ∪ S2` (their Fig. 9). The rules
//! are correct and complete for the operators here, so every valid plan
//! of the core search space is enumerated, and none other.
//!
//! An entry the tables mark as holding only if a predicate rejects nulls
//! is taken to hold only if both operators' predicates reject nulls on
//! every table they read, which implies each footnote's condition: a
//! conflict assumed where none is only narrows the search, never admits an
//! invalid plan.

use super::bits::{Set, within};

/// \[DESIGN\] A binary operator of the initial tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// The inner join, or a product.
    Inner,
    /// The LEFT outer join, its left operand preserved; a RIGHT one is
    /// this of its operands swapped.
    Left,
    /// The FULL outer join.
    Full,
}

impl Kind {
    /// Table 1: whether its operands may be swapped.
    pub(super) fn commutes(self) -> bool {
        matches!(self, Kind::Inner | Kind::Full)
    }
}

/// An operator of the initial tree, as the detector sees it: its kind,
/// the tables of its operands, those its predicate reads, and whether
/// that predicate rejects nulls on each of them.
#[derive(Debug, Clone, Copy)]
pub(super) struct Shape {
    /// Its kind.
    pub(super) kind: Kind,
    /// T(left(◦)) and T(right(◦)).
    pub(super) left: Set,
    pub(super) right: Set,
    /// FT(◦): the tables its predicate reads.
    pub(super) reads: Set,
    /// Whether its predicate is false or unknown of every row whose
    /// columns of any one table it reads are all null.
    pub(super) strong: bool,
}

impl Shape {
    /// T(◦): the tables of its subtree.
    fn tables(&self) -> Set {
        self.left | self.right
    }
}

/// Whether a footnoted entry holds of `a` and `b`: both predicates reject
/// nulls on every table they read.
fn both_strong(a: &Shape, b: &Shape) -> bool {
    a.strong && b.strong
}

/// Table 2: assoc(◦a, ◦b), `(e1 ◦a e2) ◦b e3 ≡ e1 ◦a (e2 ◦b e3)`.
fn assoc(a: &Shape, b: &Shape) -> bool {
    match (a.kind, b.kind) {
        (Kind::Inner, Kind::Inner | Kind::Left) => true,
        (Kind::Left | Kind::Full, Kind::Left) | (Kind::Full, Kind::Full) => both_strong(a, b),
        (Kind::Inner, Kind::Full) | (Kind::Left, Kind::Inner | Kind::Full) | (Kind::Full, Kind::Inner) => false,
    }
}

/// Table 3: l-asscom(◦a, ◦b), `(e1 ◦a e2) ◦b e3 ≡ (e1 ◦b e3) ◦a e2`.
fn left_asscom(a: &Shape, b: &Shape) -> bool {
    match (a.kind, b.kind) {
        (Kind::Inner | Kind::Left, Kind::Inner | Kind::Left) => true,
        (Kind::Left, Kind::Full) | (Kind::Full, Kind::Left | Kind::Full) => both_strong(a, b),
        (Kind::Full, Kind::Inner) | (Kind::Inner, Kind::Full) => false,
    }
}

/// Table 3: r-asscom(◦a, ◦b), `e1 ◦a (e2 ◦b e3) ≡ e2 ◦b (e1 ◦a e3)`.
fn right_asscom(a: &Shape, b: &Shape) -> bool {
    match (a.kind, b.kind) {
        (Kind::Inner, Kind::Inner) => true,
        (Kind::Full, Kind::Full) => both_strong(a, b),
        (Kind::Inner, Kind::Left | Kind::Full)
        | (Kind::Left, Kind::Inner | Kind::Left | Kind::Full)
        | (Kind::Full, Kind::Inner | Kind::Left) => false,
    }
}

/// A conflict rule, `T1 → T2`.
pub(super) type Rule = (Set, Set);

/// The conflict rules of operator `b` of `ops`, by CD-C (Fig. 11): of each
/// operator below it, a rule for each property that does not hold of the
/// two, its right side the tables of the lower operator's other operand
/// that its predicate reads, or all of them if it reads none.
pub(super) fn rules(ops: &[Shape], b: usize) -> Vec<Rule> {
    let Some(upper) = ops.get(b) else { return Vec::new() };
    let mut out = Vec::new();
    // The tables of `side` the predicate of `a` reads, or all of `side`'s.
    let read = |a: &Shape, side: Set| if side & a.reads != 0 { side & a.reads } else { side };
    for (n, a) in ops.iter().enumerate() {
        if n == b {
            continue;
        }
        if within(a.tables(), upper.left) {
            if !assoc(a, upper) {
                out.push((a.right, read(a, a.left)));
            }
            if !left_asscom(a, upper) {
                out.push((a.left, read(a, a.right)));
            }
        } else if within(a.tables(), upper.right) {
            if !assoc(upper, a) {
                out.push((a.left, read(a, a.right)));
            }
            if !right_asscom(upper, a) {
                out.push((a.right, read(a, a.left)));
            }
        }
    }
    out
}

/// Whether the join of `s1` and `s2` obeys `rules` (Fig. 9, lines 2-6).
pub(super) fn obeyed(rules: &[Rule], s: Set) -> bool {
    rules.iter().all(|&(t1, t2)| t1 & s == 0 || within(t2, s))
}

// Unit tests: the tables' entries and the rules they give for the
// examples of the paper (RULES.md R-TEST-1, R-TEST-8).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::physical::enumerate::bits::bit;

    fn shape(kind: Kind, (left, right): (Set, Set), reads: Set) -> Shape {
        Shape { kind, left, right, reads, strong: true }
    }

    /// Verifies: [DESIGN] RULES.md D301: inner joins and left outer joins
    /// over them reassociate freely; a left outer join reassociates over
    /// another only if both predicates reject nulls (the pair strong
    /// against the pair not).
    #[test]
    fn the_tables_say_what_reassociates() {
        let r = (bit(0), bit(1));
        let (inner, left) = (shape(Kind::Inner, r, 0), shape(Kind::Left, r, 0));
        assert!(assoc(&inner, &left));
        assert!(!assoc(&left, &inner));
        assert!(assoc(&left, &left));
        let weak = Shape { strong: false, ..left };
        assert!(!assoc(&weak, &left));
        assert!(left_asscom(&left, &inner));
        assert!(!left_asscom(&inner, &shape(Kind::Full, r, 0)));
        assert!(right_asscom(&inner, &inner));
        assert!(!right_asscom(&inner, &left));
        assert!(Kind::Full.commutes() && !Kind::Left.commutes());
    }

    /// Verifies: [DESIGN] RULES.md D301: of `(R0 ⟕ R1) ⋈ R2` the inner
    /// join may not take R1 without R0 (assoc fails), as `R0 ⟕ (R1 ⋈ R2)`
    /// would change the result, but may take R0 first (l-asscom holds);
    /// the rule is obeyed by a set holding R1 only with R0.
    #[test]
    fn a_left_join_below_an_inner_one_keeps_its_preserved_side() {
        let ops = [shape(Kind::Left, (bit(0), bit(1)), bit(0) | bit(1)), shape(Kind::Inner, (bit(0) | bit(1), bit(2)), bit(1) | bit(2))];
        let rules = rules(&ops, 1);
        assert_eq!(rules, [(bit(1), bit(0))]);
        assert!(!obeyed(&rules, bit(1) | bit(2)));
        assert!(obeyed(&rules, bit(0) | bit(2)));
        assert!(obeyed(&rules, bit(0) | bit(1) | bit(2)));
    }
}
