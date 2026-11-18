//! Unit tests of a graph pattern's Syntax Rules [P1 §4.2 to §5.3]: the
//! degree each variable is seen at, termination, where selectors go, and
//! each GRF condition, all SQLCODE -103 with the condition's name in the
//! message.

use super::*;

/// Verifies: [EXTENSION] [P1 §4.6] an implicit join on a conditional
/// singleton is GRF-0021: one bound under `?`, or in only some operands
/// of a union, and joined by a later path pattern or the next element.
#[test]
fn joins_on_conditional_singletons_are_grf_0021_4_6() {
    assert_eq!(grf("[(x)->(y)] | [(x)->(z)], (y)->(w)"), "GRF-0021", "P1's illegal query");
    assert_eq!(grf("(x) [->(y)]?, (y)->(w)"), "GRF-0021");
    assert_eq!(grf("(x) [->(y)]? (y)"), "GRF-0021");
    accepted("[(x)->(y)] | [(x)->(z)], (x)->(w)");
    accepted("[(x)->(y)] | [(x)->(y)], (y)->(w)");
    accepted("(x) [->(y)]?");
}

/// Verifies: [EXTENSION] [P1 §4.4] a group variable joined, or referenced
/// outside an aggregate, and a singleton inside one, are GRF-0022; `{0,1}`
/// exposes groups where `?` exposes conditional singletons.
#[test]
fn group_variables_outside_aggregates_are_grf_0022_4_4() {
    assert_eq!(grf("(a) [-[e]->(b)]{1,3}, (b)"), "GRF-0022");
    assert_eq!(grf("(a) [->(b)]{0,1}, (b)"), "GRF-0022");
    assert_eq!(grf("(a)-[e]->{1,3}(b) WHERE e.amount > 1"), "GRF-0022");
    assert_eq!(grf("(a)-[e]->{1,3}(b) WHERE SUM(a.amount) > 1"), "GRF-0022");
    assert_eq!(grf("(a)[-[e]->(b) WHERE COUNT(e) > 1]{1,3}"), "GRF-0022", "e is a singleton in its own brackets");
    let in_columns = table("GRAPH_TABLE (G MATCH (a)-[e]->{1,3}(b) COLUMNS (e.amount AS AMOUNT))").unwrap_err();
    assert!(in_columns.msg.contains("GRF-0022"), "{in_columns:?}");
    accepted("(a)-[e]->{1,3}(b) WHERE SUM(e.amount) > 1 AND COUNT(e) > 1");
    accepted("(a)[-[e]->(b) WHERE e.amount > 1]{1,3}");
    accepted("(a)[[-[e]->(b)]{1,2} WHERE SUM(e.amount) > 1]{1,3}");
}

/// Verifies: [EXTENSION] [P1 §4.7] SAME's and ALL_DIFFERENT's operands
/// are unconditional singletons, else GRF-0023.
#[test]
fn same_and_all_different_take_unconditional_singletons_grf_0023_4_7() {
    assert_eq!(grf("(a) [->(x)]? WHERE SAME(a, x)"), "GRF-0023");
    assert_eq!(grf("(a)-[e]->{1,2}(b) WHERE ALL_DIFFERENT(a, b, e)"), "GRF-0023");
    assert_eq!(grf("[(a)->(b)] | [(a)->(c)] WHERE VERTEX_EQUAL(a, b)"), "GRF-0023");
    accepted("(a)-[e]->(b)-[f]->(c) WHERE ALL_DIFFERENT(a, b, c) AND SAME(a, a) AND EDGE_EQUAL(e, f)");
}

/// Verifies: [EXTENSION] [P1 §5] an unbounded quantifier in the scope of
/// no selector and no restrictor but WALK is GRF-0031; a selector or
/// restrictor of the path pattern, of KEEP or of a parenthesized path
/// pattern around it covers it.
#[test]
fn unbounded_quantifiers_need_a_selector_or_restrictor_grf_0031_5() {
    for text in
        ["(a)->*(b)", "(a)->+(b)", "(a)->{2,}(b)", "WALK (a)->*(b)", "(a)[(x)->(y)]+(b)", "(a)[WALK (x)->*(y)](b)", "(a)->*(b) KEEP WALK"]
    {
        assert_eq!(grf(text), "GRF-0031", "{text}");
    }
    for text in [
        "ANY (a)->*(b)",
        "TRAIL (a)->+(b)",
        "ANY SHORTEST WALK (a)->+(b)",
        "(a)[ACYCLIC (x)->*(y)](b)",
        "(a)->*(b) KEEP SIMPLE",
        "(a)->*(b) KEEP ANY SHORTEST",
        "(a)->{1,9}(b)",
        "(a)[(x)->(y)]?(b)",
    ] {
        accepted(text);
    }
}

/// Verifies: [EXTENSION] [P1 §5.3] a prefilter's aggregate of a group
/// variable unbounded where it is written is GRF-0032; a bound or a
/// restrictor bounds it, and a postfilter sees it bounded by the
/// selector.
#[test]
fn prefilter_aggregates_of_unbounded_groups_are_grf_0032_5_3() {
    assert_eq!(grf("ALL SHORTEST [(x)-[e]->*(y) WHERE COUNT(e)/(COUNT(e)+1) > 1]"), "GRF-0032");
    assert_eq!(grf("ANY (a)[[-[e]->]+ WHERE SUM(e.x) > 1](b)"), "GRF-0032");
    accepted("ALL SHORTEST [TRAIL (x)-[e]->*(y) WHERE COUNT(e)/(COUNT(e)+1) > 1]");
    accepted("ALL SHORTEST [(x)-[e]->{0,10}(y) WHERE COUNT(e)/(COUNT(e)+1) > 1]");
    accepted("ALL SHORTEST (x)-[e]->*(y) WHERE COUNT(e)/(COUNT(e)+1) > 1");
    accepted("TRAIL (a)[[-[e]->]+ WHERE SUM(e.x) > 1](b)");
}

/// Verifies: [EXTENSION] [P1 §5.1] a selector inside a parenthesized path
/// pattern is GRF-0033; a restrictor there is allowed.
#[test]
fn selectors_inside_parentheses_are_grf_0033_5_1() {
    assert_eq!(grf("(a)[ANY SHORTEST (x)->*(y)](b)"), "GRF-0033");
    assert_eq!(grf("TRAIL (a)[(c)[ALL SHORTEST (x)->(y)]](b)"), "GRF-0033");
    accepted("(a)[TRAIL (x)->*(y)](b)");
}

/// Verifies: [EXTENSION] [Graph standard §7.2] a CHEAPEST selector's COST
/// literal is a number not less than 0, else GRF-0034; another value the
/// binder checks by its type.
#[test]
fn literal_costs_are_non_negative_numbers_grf_0034_7_2() {
    for cost in ["(-1)", "(-0.5)", "(-1E0)", "('ten')", "(TRUE)"] {
        assert_eq!(grf(&format!("ANY CHEAPEST COST {cost} (a)-[e]->*(b)")), "GRF-0034", "{cost}");
    }
    assert_eq!(grf("(a)-[e]->*(b) KEEP CHEAPEST 2 COST (-3)"), "GRF-0034");
    for cost in ["(0)", "(2.5)", "(1E1)", "e.amount", "(e.amount * 2)", "(+4)"] {
        accepted(&format!("ANY CHEAPEST COST {cost} (a)-[e]->*(b)"));
    }
}

/// Verifies: [EXTENSION] [P1 §11.1 of the graph standard] PREV or FIRST
/// of PREV or FIRST is GRF-0041.
#[test]
fn nested_prev_is_grf_0041_11_1() {
    assert_eq!(grf("ANY (a)[-[t]-> WHERE PREV(PREV(t)) IS NULL]+(b)"), "GRF-0041");
    assert_eq!(grf("ANY (a)[-[t]-> WHERE FIRST(PREV(t)) IS NULL]+(b)"), "GRF-0041");
    assert_eq!(grf("ANY (a)[-[t]-> WHERE PREV(FIRST(t)) IS NULL]+(b)"), "GRF-0041");
}

/// Verifies: [EXTENSION] [P1 §4.2] one name is one kind of variable: a
/// node's and an edge's, or an element's and a path's, is SQLCODE -103.
#[test]
fn one_name_one_kind_of_variable_4_2() {
    for text in ["(x)-[x]->(y)", "p = (p)", "(a)-[e]->(b), (e)", "p = (a), q = (p)", "(a)[q = (x)->(y)](q)"] {
        assert_eq!(refused(text), -103_i32, "{text}");
    }
    accepted("(x)-[e]->(y), (y)-[f]->(x)");
    accepted("p = (a), q = (a)");
}

/// Verifies: [EXTENSION] [P1 §4.4, §4.7] each reference names a variable
/// of the kind its place needs: a property of no path, a function's
/// operand of its kind, a predicate's of its, and `x`.* of an element;
/// a name no pattern declares in a predicate or function is -103.
#[test]
fn references_name_variables_of_their_kind_4_7() {
    for text in [
        "p = (a)->(b) WHERE p.x = 1",
        "(a)-[e]->(b) WHERE a IS SOURCE OF b",
        "(a)-[e]->(b) WHERE e IS SOURCE OF e",
        "(a)-[e]->(b) WHERE a IS DIRECTED",
        "(a)-[e]->(b) WHERE VERTEX_EQUAL(e, e)",
        "(a)-[e]->(b) WHERE EDGE_EQUAL(a, b)",
        "(a)-[e]->(b) WHERE SAME(a, z)",
        "p = (a)->(b) WHERE SAME(p, p)",
        "(a)-[e]->(b) WHERE EDGE_ID(a) = 1",
        "(a)-[e]->(b) WHERE VERTEX_ID(e) = 1",
        "(a)-[e]->(b) WHERE PATH_LENGTH(a) = 1",
        "(a)-[e]->(b) WHERE a.x IS SOURCE OF e",
        "(a)-[e]->(b) WHERE 1 IS LABELED L",
    ] {
        assert_eq!(refused(text), -103_i32, "{text}");
    }
    let star = table("GRAPH_TABLE (G MATCH p = (a) COLUMNS (p.*))").unwrap_err();
    assert_eq!(star.sqlcode(), -103_i32);
    accepted("(a)-[e]->(b) WHERE a.x = OUTER_COLUMN AND T.C = 1");
}

/// Verifies: [EXTENSION] [P1 §7.2] KEEP gives a selector or restrictor to
/// path patterns with none of that kind; one of the same kind on a path
/// pattern is -103; a selector on one and a restrictor in KEEP mix.
#[test]
fn keep_does_not_clash_with_path_prefixes_7_2() {
    assert_eq!(refused("ANY (a)->*(b) KEEP ANY SHORTEST"), -103_i32);
    assert_eq!(refused("TRAIL (a)->*(b) KEEP ACYCLIC"), -103_i32);
    accepted("ANY (a)->*(b) KEEP TRAIL");
    accepted("(a)->*(b), (b)->*(c) KEEP ANY SHORTEST TRAIL");
}

/// Verifies: [EXTENSION] [P1 §4.4] the bounds of a quantifier and a
/// selector's k: a quantified node pattern, an upper bound less than
/// the lower or 0, and k 0 are SQLCODE -103.
#[test]
fn quantifier_bounds_and_k_are_checked_4_4() {
    for text in ["(a)*", "(a){2}", "->{3,1}", "->{0}", "->{0,0}", "ANY 0 (a)", "SHORTEST 0 (a)", "CHEAPEST 0 (a)"] {
        let e = pattern(text).map(|_| panic!("{text}: accepted")).unwrap_err();
        assert_eq!(e.sqlcode(), -103_i32, "{text}: {e:?}");
    }
    assert!(pattern("->{,}").is_ok(), "{{,}} is {{0,}}");
    assert!(pattern("->{4294967295}").is_ok());
    assert!(pattern("->{4294967296}").is_err(), "a bound is a u32");
}

/// Verifies: [EXTENSION] [P1 §8.3 of the graph standard] ONE ROW PER
/// VERTEX's and STEP's variables are new names, of a vertex, an edge and a
/// vertex, which COLUMNS sees as singletons.
#[test]
fn rows_variables_are_new_names_8_3() {
    let read1 = table("GRAPH_TABLE (G MATCH p = (a)->{1,3}(b) ONE ROW PER STEP (v1, e, v2) COLUMNS (v1.x, e.y, v2.z))").unwrap();
    assert!(matches!(&read1.rows, GraphRows::Step(a, e, b) if (a.name.as_str(), e.name.as_str(), b.name.as_str()) == ("V1", "E", "V2")));
    let read2 = table("GRAPH_TABLE (G MATCH (a) ONE ROW PER VERTEX (v) COLUMNS (v.x))").unwrap();
    assert!(matches!(&read2.rows, GraphRows::Vertex(v) if v.name == "V"));
    let read3 = table("GRAPH_TABLE (G MATCH (a) ONE ROW PER MATCH COLUMNS (a.x))").unwrap();
    assert!(matches!(read3.rows, GraphRows::Match));
    for text in [
        "GRAPH_TABLE (G MATCH (a) ONE ROW PER VERTEX (a) COLUMNS (a.x))",
        "GRAPH_TABLE (G MATCH (a) ONE ROW PER STEP (v, e, v) COLUMNS (v.x))",
        "GRAPH_TABLE (G MATCH (a) ONE ROW PER STEP (v, e, w) COLUMNS (IS_TRAIL(e)))",
    ] {
        assert_eq!(table(text).map(|_| ()).unwrap_err().sqlcode(), -103_i32, "{text}");
    }
}

/// Verifies: [EXTENSION] [P1 §11.1, §11.2 of the graph standard] FOR EACH
/// SEGMENT's edges are two new names, of a path variable, its condition
/// seeing them; EXISTS IN PATH's variables are its own, of a path
/// variable.
#[test]
fn segments_and_paths_declare_their_own_variables_11() {
    accepted("ANY p = (a)->+(b) FOR EACH SEGMENT (e1, e2) IN p WHERE e1.ts < e2.ts");
    accepted("ANY p = (x)->+(y) WHERE NOT EXISTS IN PATH p (-[u]->()-[v]-> WHERE u.k >= v.k)");
    for text in [
        "ANY p = (a)->+(b) FOR EACH SEGMENT (a, e2) IN p WHERE TRUE",
        "ANY p = (a)->+(b) FOR EACH SEGMENT (e, e) IN p WHERE TRUE",
        "ANY p = (a)->+(b) FOR EACH SEGMENT (e1, e2) IN a WHERE TRUE",
        "ANY p = (a)->+(b) FOR EACH SEGMENT (e1, e2) IN p WHERE e3.ts < 1 AND SAME(e1, e3)",
        "ANY (a)-[e]->+(b) WHERE EXISTS IN PATH e (()->())",
        "ANY p = (x)->+(y) WHERE EXISTS IN PATH p (-[u]->(u))",
    ] {
        assert_eq!(refused(text), -103_i32, "{text}");
    }
}
