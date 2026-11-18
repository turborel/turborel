//! Tests of what an XML index is asked.

use super::*;
use crate::index::xml::VALUE_PREFIX_BYTES;
use crate::index::xml::keys::{ordinal_of, values_of};

const CHILD: Axis = Axis::Child;
const DESCENDANT: Axis = Axis::Descendant;

/// Whether `steps`, of names as letters, `*` any, select the last name of
/// `path`, from the item down.
fn selects(steps: &[(Axis, char)], path: &str) -> bool {
    let steps: Vec<(Axis, Option<char>)> = steps.iter().map(|&(a, c)| (a, (c != '*').then_some(c))).collect();
    steps_match(&steps, &path.chars().collect::<Vec<_>>())
}

/// Verifies: [DESIGN] RULES.md D106 [XQuery §3.2, §3.2.4]: a child step
/// takes the name right below the step before's, or the item's; a
/// descendant step any below; `*` any name; the last step ends at the
/// path's node. MC/DC of `reached && named`: each pair differs in one.
#[test]
fn steps_select_by_axis_and_name() {
    assert!(selects(&[(CHILD, 'a'), (CHILD, 'b')], "ab"));
    assert!(!selects(&[(CHILD, 'a'), (CHILD, 'b')], "axb"), "not reached: a child step skips nothing");
    assert!(selects(&[(CHILD, 'a'), (DESCENDANT, 'b')], "axb"), "reached by a descendant step");
    assert!(!selects(&[(CHILD, 'a'), (DESCENDANT, 'c')], "axb"), "reached, not named");
    assert!(selects(&[(CHILD, 'a'), (CHILD, '*')], "ab"), "any name");
    assert!(!selects(&[(CHILD, 'b')], "ab"), "the first step is the item's child");
    assert!(selects(&[(DESCENDANT, 'b')], "ab"));
    assert!(!selects(&[(DESCENDANT, 'a')], "ab"), "the last step ends at the node");
    assert!(selects(&[(DESCENDANT, 'a'), (DESCENDANT, 'a')], "aaa"));
    assert!(!selects(&[(DESCENDANT, 'a'), (DESCENDANT, 'a')], "a"), "two steps need two names");
    assert!(!selects(&[(CHILD, 'a')], ""), "no node");
}

/// Verifies: [DESIGN] RULES.md D106 [Pal et al. §3.2.1]: the dictionary's
/// prefix of a path is its kind and its last steps' tokens, reversed, up
/// to a descendant step's or before a wildcard; whole only of named child
/// steps.
#[test]
fn a_paths_prefix_is_its_fixed_last_steps() {
    let prefix = |steps: &[(Axis, Option<u64>)]| prefix_of(ELEMENT, steps);
    let tokens = |ts: &[u64]| [vec![ELEMENT], ts.iter().flat_map(|&t| ordinal_of(t)).collect()].concat();
    assert_eq!(prefix(&[(CHILD, Some(1)), (CHILD, Some(2))]), (tokens(&[2, 1]), true));
    assert_eq!(prefix(&[(CHILD, Some(1)), (DESCENDANT, Some(2)), (CHILD, Some(3))]), (tokens(&[3, 2]), false));
    assert_eq!(prefix(&[(CHILD, Some(1)), (CHILD, None), (CHILD, Some(3))]), (tokens(&[3]), false));
    assert_eq!(prefix(&[(CHILD, None)]), (tokens(&[]), false));
}

/// `comparisons`' ranges, for a literal `literal` compared by each of
/// `ops`.
fn ranges(ops: &[CompOp], literal: &Literal) -> Option<Vec<ValueRange>> {
    value_ranges(&ops.iter().map(|&op| Comparison { op, literal: literal.clone() }).collect::<Vec<_>>(), VALUE_PREFIX_BYTES)
}

/// Whether a node of value `text` is in `ranges`.
fn within(ranges: &[ValueRange], text: &str) -> bool {
    let bytes = text.as_bytes();
    let keys = values_of(bytes.get(..VALUE_PREFIX_BYTES).unwrap_or(bytes), bytes.len() <= VALUE_PREFIX_BYTES);
    keys.iter().any(|k| ranges.iter().any(|r| r.holds(k)))
}

/// Verifies: [DESIGN] RULES.md D106 [XQuery §3.5.2]: a string literal
/// selects the string keys its comparison may hold of, a tie at a cut
/// kept; `!=` selects every value.
#[test]
fn strings_select_their_range() {
    let s = |t: &str| Literal::String(t.to_owned());
    let eq = ranges(&[CompOp::Eq], &s("m")).unwrap();
    assert!(within(&eq, "m") && !within(&eq, "mm") && !within(&eq, "l"));
    let lt = ranges(&[CompOp::Lt], &Literal::Untyped("m".to_owned())).unwrap();
    assert!(within(&lt, "a") && within(&lt, "m") && !within(&lt, "n"), "a tie is kept, to be checked");
    let between = ranges(&[CompOp::Ge, CompOp::Le], &s("m")).unwrap();
    assert!(within(&between, "m") && !within(&between, "a"));
    let long = "q".repeat(100);
    let cut = ranges(&[CompOp::Eq], &s(&long)).unwrap();
    assert!(within(&cut, &format!("{}r", "q".repeat(99))), "cut alike: a candidate");
    assert!(!within(&eq, "12"), "no number");
    assert!(ranges(&[CompOp::Ne], &s("m")).is_none());
}

/// Verifies: [DESIGN] RULES.md D106, D15 [XQuery §3.5.2]: a number literal
/// selects the numbers its comparison holds of, exactly, and every value
/// that is no number, whose comparison would raise err:FORG0001; NaN
/// selects those alone.
#[test]
fn numbers_select_their_range_and_the_unordered() {
    let n = Literal::Number(10.0);
    let gt = ranges(&[CompOp::Gt], &n).unwrap();
    assert!(within(&gt, "10.5") && !within(&gt, "10") && !within(&gt, "9"));
    assert!(within(&gt, "ten"), "no number: its cast raises");
    let le = ranges(&[CompOp::Le], &n).unwrap();
    assert!(within(&le, "10") && within(&le, " -INF ") && !within(&le, "11"));
    let between = value_ranges(
        &[
            Comparison { op: CompOp::Gt, literal: Literal::Number(1.0) },
            Comparison { op: CompOp::Lt, literal: Literal::Number(3.0) },
            Comparison { op: CompOp::Eq, literal: Literal::String("x".to_owned()) },
        ],
        VALUE_PREFIX_BYTES,
    )
    .unwrap();
    assert!(within(&between, "2") && !within(&between, "3") && !within(&between, "1"), "the first's kind alone");
    assert!(within(&between, "x"), "no number: a candidate, its string comparison left to the check");
    let nan = ranges(&[CompOp::Eq], &Literal::Number(f64::NAN)).unwrap();
    assert!(within(&nan, "x") && !within(&nan, "1"));
    let empty = ranges(&[CompOp::Gt, CompOp::Lt], &n).unwrap();
    assert_eq!(empty.len(), 1, "of the numbers, none: the unordered alone");
}
