//! Unit tests of a graph pattern's normal form [P1 §6.2]: edge patterns
//! in full, quantified edges bracketed with their end nodes, implicit
//! nodes inserted, `*` and `+` as bounds, anonymous variables named, and
//! the orientation table's rows.

use super::*;
use crate::ast::{Directions, ORIENTATIONS, Orientation};

/// The shape of `text`'s normal form.
fn normal(text: &str) -> String {
    shape(&normalize(&pattern(text).unwrap_or_else(|e| panic!("{text}: {e:?}"))))
}

/// Verifies: [EXTENSION] [P1 §6.2] an abbreviated edge pattern is its
/// full form, of its orientation, and gets an anonymous node on each side
/// where none is written; an edge pattern's own variable is kept.
#[test]
fn edges_are_full_with_nodes_on_each_side_6_2() {
    assert_eq!(normal("->"), "($1)-[$2]->($3)");
    assert_eq!(normal("(a)<~(b)"), "(A)<~[$1]~(B)");
    assert_eq!(normal("-[e]-"), "($1)-[E]-($2)");
    assert_eq!(normal("(a)->->(b)"), "(A)-[$1]->($2)-[$3]->(B)");
    assert_eq!(normal("(a)(b)"), "(A)(B)", "two nodes stay two");
    assert_eq!(normal("(x)-[:Transfer]->()-[:isLocatedIn]->(y)"), "(X)-[$1:TRANSFER]->($2)-[$3:ISLOCATEDIN]->(Y)");
}

/// Verifies: [EXTENSION] [P1 §4.4, §6.2] a quantified edge pattern is the
/// bracketed path pattern of it and an anonymous node on each side, the
/// quantifier on the brackets; `*` and `+` become their bounds and `?`
/// stays.
#[test]
fn quantified_edges_are_bracketed_6_2() {
    assert_eq!(normal("(a)-[t:Transfer]->*(b)"), "(A)[($1)-[T:TRANSFER]->($2)]{0,}(B)");
    assert_eq!(normal("(a)->+(b)"), "(A)[($1)-[$2]->($3)]{1,}(B)");
    assert_eq!(normal("(a)~>{2,5}(b)"), "(A)[($1)~[$2]~>($3)]{2,5}(B)");
    assert_eq!(normal("(x)[->(y)]?"), "(X)[($1)-[$2]->(Y)]?");
    assert_eq!(normal("((a)->(b))*"), "((A)-[$1]->(B)){0,}");
}

/// Verifies: [EXTENSION] [P1 §6.2] a parenthesized path pattern is no
/// node, so an edge pattern beside it gets an anonymous node; inside
/// unions and alternations each operand is normal on its own.
#[test]
fn parentheses_and_unions_are_normal_inside_6_2() {
    assert_eq!(normal("[(a)->(b)]->[(c)]"), "[(A)-[$1]->(B)]($2)-[$3]->($4)[(C)]");
    assert_eq!(normal("-> | <-"), "($1)-[$2]->($3) | ($4)<-[$5]-($6)");
    assert_eq!(normal("(a)[->(c) |+| ~(c)]"), "(A)[($1)-[$2]->(C) |+| ($3)~[$4]~(C)]");
}

/// Verifies: [EXTENSION] [P1 §6.2] anonymous variables are named `$n` in
/// the order written, from 1, past names declared anywhere in the
/// pattern, a later path pattern's or a delimited `$1` among them; each
/// is marked anonymous; normalizing twice changes nothing.
#[test]
fn anonymous_variables_are_named_in_order_6_2() {
    assert_eq!(normal("(\"$1\")->(), (\"$3\")"), "($1)-[$2]->($4), ($3)");
    let read = normalize(&pattern("(a)-[]->()").unwrap());
    let terms = read.paths[0].expression.terms();
    let names: Vec<(String, bool)> = terms[0]
        .factors
        .iter()
        .map(|f| match &f.primary {
            PathPrimary::Node(e) => e.variable.clone().unwrap(),
            PathPrimary::Edge(e) => e.filler.variable.clone().unwrap(),
            PathPrimary::Parenthesized(_) => panic!(),
        })
        .map(|v| (v.name, v.anonymous))
        .collect();
    assert_eq!(names, [("A".into(), false), ("$1".into(), true), ("$2".into(), true)]);
    for text in ["(a)->*(b)", "[(a)->(b)]->[(c)]", "-> | <-", "(x)[->(y)]?"] {
        let once = normalize(&pattern(text).unwrap());
        assert_eq!(shape(&normalize(&once)), shape(&once), "{text}");
    }
}

/// Verifies: [EXTENSION] [P1 §4.1, Fig. 5] the orientation table: each
/// row's orientation, its delimiters, the directions it matches, and its
/// reverse, which is the row of the mirrored directions.
#[test]
fn orientation_table_rows_4_1() {
    let d = |left, undirected, right| Directions { left, undirected, right };
    let expected = [
        (Orientation::Left, "<-[", "]-", "<-", d(true, false, false)),
        (Orientation::Undirected, "~[", "]~", "~", d(false, true, false)),
        (Orientation::Right, "-[", "]->", "->", d(false, false, true)),
        (Orientation::LeftOrUndirected, "<~[", "]~", "<~", d(true, true, false)),
        (Orientation::UndirectedOrRight, "~[", "]~>", "~>", d(false, true, true)),
        (Orientation::LeftOrRight, "<-[", "]->", "<->", d(true, false, true)),
        (Orientation::Any, "-[", "]-", "-", d(true, true, true)),
    ];
    for (row, (orientation, opening, closing, abbreviation, directions)) in ORIENTATIONS.iter().zip(expected) {
        assert_eq!((row.orientation, row.opening, row.closing, row.abbreviation), (orientation, opening, closing, abbreviation));
        assert_eq!(row.directions, directions, "{orientation:?}");
        assert_eq!(orientation.form().abbreviation, abbreviation);
        assert_eq!(orientation.directions(), directions);
        assert_eq!(Orientation::of(directions), Some(orientation));
        let mirrored = d(directions.right, directions.undirected, directions.left);
        assert_eq!(orientation.reversed().directions(), mirrored, "{orientation:?}");
        assert_eq!(orientation.reversed().reversed(), orientation);
    }
    assert_eq!(Orientation::of(d(false, false, false)), None);
}
