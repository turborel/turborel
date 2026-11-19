//! Unit tests of each production of a graph pattern [P1 §4, §5]: element
//! patterns, the seven orientations in both forms, label expressions,
//! quantifiers, unions and alternations, parenthesized path patterns,
//! path variables, prefixes, match modes and KEEP; each written back, and
//! read again to the same shape.

use super::*;

/// Verifies: [EXTENSION] [P1 §4.1] a node pattern's filler: none, a
/// variable, a label after `:` or IS, a WHERE, properties in braces, and
/// all of them; WHERE and the properties in either order.
#[test]
fn node_patterns_read_each_part_4_1() {
    assert_eq!(shaped("()"), "()");
    assert_eq!(shaped("(x)"), "(X)");
    assert_eq!(shaped("(x:Account)"), "(X:ACCOUNT)");
    assert_eq!(shaped("(x IS Account)"), "(X:ACCOUNT)");
    assert_eq!(shaped("(:Account)"), "(:ACCOUNT)");
    assert_eq!(shaped("(IS Account)"), "(:ACCOUNT)");
    assert_eq!(shaped("(x WHERE x.owner = 'Dave')"), "(X WHERE c)");
    assert_eq!(shaped("(x:Account WHERE x.is_blocked = 'no')"), "(X:ACCOUNT WHERE c)");
    assert_eq!(shaped("(a:Account {owner: 'Zoe', is_blocked: FALSE})"), "(A:ACCOUNT {OWNER: v, IS_BLOCKED: v})");
    assert_eq!(shaped("(a {owner: 'Zoe'} WHERE a.x = 1)"), "(A WHERE c {OWNER: v})");
    assert_eq!(shaped("(a WHERE a.x = 1 {owner: 'Zoe'})"), "(A WHERE c {OWNER: v})");
    assert_eq!(shaped("(\"x\")"), "(x)", "a delimited variable keeps its case");
}

/// Verifies: [EXTENSION] [P1 §4.1, Fig. 5] each of the seven orientations
/// in its full form and its abbreviation, as the table of edge patterns
/// writes them, read to its orientation and written back alike.
#[test]
fn each_orientation_reads_in_both_forms_4_1() {
    for form in crate::ast::ORIENTATIONS {
        for (text, abbreviated) in
            [(format!("(a){}e{}(b)", form.opening, form.closing), false), (format!("(a){}(b)", form.abbreviation), true)]
        {
            let read = pattern(&text).unwrap_or_else(|err| panic!("{text}: {err:?}"));
            let PathPrimary::Edge(edge) = &read.paths[0].expression.terms()[0].factors[1].primary else { panic!("{text}") };
            assert_eq!((edge.orientation, edge.abbreviated), (form.orientation, abbreviated), "{text}");
            assert_eq!(shape(&read), text.to_uppercase(), "{text}");
        }
    }
}

/// Verifies: [EXTENSION] [P1 §4.1] an edge pattern's filler: a variable,
/// a label, a WHERE and properties, as a node pattern's; spaces between
/// the delimiters of an arrow are allowed; `->`, `<->` and `~>` before
/// "[" are abbreviations, and "-[", "~[", "<-[" and "<~[" full forms.
#[test]
fn edge_fillers_read_like_node_fillers_4_1() {
    assert_eq!(shaped("(a)->[(b)]<->[(c)]~>[(d)]"), "(A)->[(B)]<->[(C)]~>[(D)]");
    assert!(pattern("(a)-[(b)]").is_err(), "-[ opens an edge pattern's filler");
    assert_eq!(shaped("-[e:Transfer WHERE e.amount > 5000000]->"), "-[E:TRANSFER WHERE c]->");
    assert_eq!(shaped("<-[IS Transfer]-"), "<-[:TRANSFER]-");
    assert_eq!(shaped("~[e {since: 2020}]~"), "~[E {SINCE: v}]~");
    assert_eq!(shaped("< ~[e]~"), "<~[E]~", "P1 writes `< ~[e]~`");
    assert_eq!(shaped("-[]-"), "-[]-");
}

/// Verifies: [EXTENSION] [P1 §4.1] label expressions: `!` binds tightest,
/// then `&`, then `|`; parentheses group; `%` is any label; `!%` none.
#[test]
fn label_expressions_bind_not_and_or_4_1() {
    assert_eq!(shaped("(x:A|B)"), "(X:(A|B))");
    assert_eq!(shaped("(x:A&B|C)"), "(X:((A&B)|C))");
    assert_eq!(shaped("(x:A|B&C)"), "(X:(A|(B&C)))");
    assert_eq!(shaped("(x:!A&B)"), "(X:(!A&B))");
    assert_eq!(shaped("(x:!(A&B))"), "(X:!(A&B))");
    assert_eq!(shaped("(:!%)"), "(:!%)");
    assert_eq!(shaped("(x:%)"), "(X:%)");
    assert_eq!(shaped("(x IS (A|B)&!C)"), "(X:((A|B)&!C))");
    assert_eq!(shaped("(x:!!A)"), "(X:!!A)");
}

/// Verifies: [EXTENSION] [P1 §4.4, Fig. 6] each quantifier, on an edge
/// pattern and on a parenthesized path pattern, with its bounds: `*` {0,},
/// `+` {1,}, `?` {0,1}, {n} {n,n}, {n,} {n,}, {,m} {0,m}, {n,m} as written.
#[test]
fn quantifiers_read_their_bounds_4_4() {
    let bounds = |text: &str| {
        let read = pattern(text).unwrap_or_else(|e| panic!("{text}: {e:?}"));
        let q = read.paths[0].expression.terms()[0].factors[0].quantifier.unwrap_or_else(|| panic!("{text}"));
        (q.lower, q.upper, q.sugar)
    };
    assert_eq!(bounds("->*"), (0, None, Some(QuantifierSugar::Star)));
    assert_eq!(bounds("->+"), (1, None, Some(QuantifierSugar::Plus)));
    assert_eq!(bounds("->?"), (0, Some(1), Some(QuantifierSugar::Question)));
    assert_eq!(bounds("->{3}"), (3, Some(3), None));
    assert_eq!(bounds("->{2,}"), (2, None, None));
    assert_eq!(bounds("->{,5}"), (0, Some(5), None));
    assert_eq!(bounds("->{2,5}"), (2, Some(5), None));
    assert_eq!(bounds("[(a)->(b)]{1,1}"), (1, Some(1), None));
    assert_eq!(bounds("((a)->(b))*"), (0, None, Some(QuantifierSugar::Star)));
    assert_eq!(shaped("(a)-[e]->{2,5}(b)"), "(A)-[E]->{2,5}(B)");
    assert_eq!(shaped("(x)[->(y)]?"), "(X)[->(Y)]?");
}

/// Verifies: [EXTENSION] [P1 §4.5] `|` is a path pattern union and `|+|`
/// a multiset alternation, of terms of any length; inside parentheses
/// they nest; a `||` is no union.
#[test]
fn unions_and_alternations_read_their_terms_4_5() {
    let read1 = pattern("(c:City) | (c:Country)").unwrap();
    assert!(matches!(&read1.paths[0].expression, PathExpression::Union(t) if t.len() == 2));
    let read2 = pattern("(c:City) |+| (c:Country) |+| (c)").unwrap();
    assert!(matches!(&read2.paths[0].expression, PathExpression::Alternation(t) if t.len() == 3));
    assert_eq!(shaped("->{1,5} | ->{3,7}"), "->{1,5} | ->{3,7}");
    assert_eq!(shaped("[(x)->(y)] | [(x)->(z)]"), "[(X)->(Y)] | [(X)->(Z)]");
    assert_eq!(shaped("(a)[-[:L]->(c:City) |+| -[:L]->(c:Country)]"), "(A)[-[:L]->(C:CITY) |+| -[:L]->(C:COUNTRY)]");
    assert!(pattern("(a)||(b)").is_err(), "|| is the concatenation operator");
}

/// Verifies: [EXTENSION] [P1 §4.4] a parenthesized path pattern in
/// parentheses or brackets, with a subpath variable, a restrictor and a
/// WHERE; "(" begins a node pattern when ")", IS, ":", WHERE or "{" or a
/// variable and one of them follows, else a parenthesized path pattern.
#[test]
fn parenthesized_path_patterns_read_each_part_4_4() {
    assert_eq!(shaped("[(a)->(b) WHERE a.x = b.x]{2,5}"), "[(A)->(B) WHERE c]{2,5}");
    assert_eq!(shaped("((a)->(b))"), "((A)->(B))");
    assert_eq!(shaped("(q = TRAIL (a)->+(b))"), "(Q = TRAIL (A)->+(B))");
    assert_eq!(shaped("(TRAIL q = (a)->+(b))"), "(Q = TRAIL (A)->+(B))");
    assert_eq!(shaped("(->)"), "(->)");
    assert_eq!(shaped("[[(a)]]"), "[[(A)]]");
    let read = pattern("(a)").unwrap();
    assert!(matches!(read.paths[0].expression.terms()[0].factors[0].primary, PathPrimary::Node(_)));
}

/// Verifies: [EXTENSION] [P1 §4.2, §4.3] path variables before or after
/// the prefix, several path patterns joined by commas, and node and edge
/// patterns concatenated in any order.
#[test]
fn path_patterns_read_variables_and_lists_4_3() {
    assert_eq!(shaped("p = (a)->(b)"), "P = (A)->(B)");
    assert_eq!(shaped("ANY SHORTEST p = (a)->*(b)"), "ANY SHORTEST P = (A)->*(B)");
    assert_eq!(shaped("p = ANY SHORTEST (a)->*(b)"), "ANY SHORTEST P = (A)->*(B)");
    assert_eq!(shaped("(s)->(x), (x)->(s)"), "(S)->(X), (X)->(S)");
    assert_eq!(shaped("->->(a)(b)<-"), "->->(A)(B)<-");
    let read = pattern("p = (a)").unwrap();
    assert_eq!(read.paths[0].variable.as_ref().map(|v| v.name.as_str()), Some("P"));
}

/// Verifies: [EXTENSION] [P1 §5.1, Fig. 7, Fig. 8] each restrictor and
/// selector, the selector before the restrictor, PATH or PATHS after
/// them, GROUP or GROUPS after SHORTEST k's restrictor, and the CHEAPEST
/// selectors with COST in parentheses or of a property.
#[test]
fn prefixes_read_selectors_and_restrictors_5_1() {
    for (text, shape_of) in [
        ("WALK (a)", "WALK (A)"),
        ("TRAIL (a)", "TRAIL (A)"),
        ("ACYCLIC (a)", "ACYCLIC (A)"),
        ("SIMPLE PATH (a)", "SIMPLE (A)"),
        ("ANY (a)", "ANY (A)"),
        ("ANY 3 (a)", "ANY 3 (A)"),
        ("ANY SHORTEST (a)", "ANY SHORTEST (A)"),
        ("ALL SHORTEST TRAIL PATHS (a)", "ALL SHORTEST TRAIL (A)"),
        ("SHORTEST 2 (a)", "SHORTEST 2 (A)"),
        ("SHORTEST 2 GROUP (a)", "SHORTEST 2 GROUPS (A)"),
        ("SHORTEST 2 ACYCLIC PATHS GROUPS (a)", "SHORTEST 2 GROUPS ACYCLIC (A)"),
        ("ANY CHEAPEST (a)", "ANY CHEAPEST (A)"),
        ("ALL CHEAPEST COST (t.amount * 2) (a)", "ALL CHEAPEST COST (v) (A)"),
        ("CHEAPEST 3 COST t.amount (a)-[t]->*(b)", "CHEAPEST 3 COST (v) (A)-[T]->*(B)"),
    ] {
        assert_eq!(shaped(text), shape_of, "{text}");
    }
    assert_eq!(shaped("TRAIL = (a)"), "TRAIL = (A)", "a name before = is a path variable's");
}

/// Verifies: [EXTENSION] [P1 §7.1] each match mode in each spelling,
/// KEEP before or after the WHERE, and FOR EACH SEGMENT after both.
#[test]
fn match_modes_keep_and_segments_read_7_1() {
    assert_eq!(shaped("REPEATABLE ELEMENTS (a)"), "REPEATABLE ELEMENTS (A)");
    assert_eq!(shaped("REPEATABLE ELEMENT BINDINGS (a)"), "REPEATABLE ELEMENTS (A)");
    assert_eq!(shaped("DIFFERENT EDGES (a)"), "DIFFERENT EDGES (A)");
    assert_eq!(shaped("DIFFERENT EDGE (a)"), "DIFFERENT EDGES (A)");
    assert_eq!(shaped("DIFFERENT VERTICES (a)"), "DIFFERENT VERTICES (A)");
    assert_eq!(shaped("DIFFERENT VERTEX BINDINGS (a)"), "DIFFERENT VERTICES (A)");
    assert_eq!(shaped("(x)-[e]->*(y) KEEP ANY SHORTEST WHERE x.a < 1"), "(X)-[E]->*(Y) KEEP ANY SHORTEST WHERE c");
    assert_eq!(shaped("(x)-[e]->*(y) WHERE AVG(e.a) < 1 KEEP ANY SHORTEST"), "(X)-[E]->*(Y) KEEP ANY SHORTEST WHERE c");
    assert_eq!(
        shaped("p = (a)->+(b) WHERE a.x = 1 FOR EACH SEGMENT (e1, e2) IN p WHERE e1.ts < e2.ts"),
        "P = (A)->+(B) WHERE c FOR EACH SEGMENT (E1, E2) IN P WHERE c"
    );
}

/// Verifies: [EXTENSION] [P1 §4] every shape written back reads again to
/// itself, so `shape` writes each production as it reads.
#[test]
fn shapes_read_back_to_themselves_4() {
    for text in [
        "ALL SHORTEST TRAIL p = (a:A|B WHERE a.x = 1)-[e IS T]->{1,3}(b), (b)<~(c {k: 1})",
        "DIFFERENT EDGES ANY 2 ACYCLIC (a)[(x)~[f]~>(y) |+| (x)<->(y)]?(b) KEEP SIMPLE WHERE a.x = b.x",
        "(:!%)[q = (a)-[]-(b) WHERE c]*",
    ] {
        let once = shaped(text);
        assert_eq!(shaped(&once), once, "{text}");
    }
}

/// Verifies: [EXTENSION] [P1 §4] text that breaks the Format is refused
/// with SQLCODE -102: an empty pattern, an unclosed element, an arrow cut
/// short, a filler after an abbreviation, a mismatched closing, mixed `|`
/// and `|+|`, a bad label, a quantifier unclosed or of no number.
#[test]
fn malformed_patterns_are_syntax_errors_4() {
    for text in [
        "",
        "(a",
        "-[e]",
        "<",
        "<>(a)",
        "-[e]~",
        "<~[e]-",
        "~[e]-",
        "<-[e]~",
        "(a)->[e]->(b)",
        "(a) | (b) |+| (c)",
        "(a) |+| (b) | (c)",
        "(x:)",
        "(x:A|)",
        "(x:(A)",
        "->{",
        "->{a}",
        "->{1,2",
        "(a {k 1})",
        "ALL (a)",
        "KEEP",
        "(a) KEEP (b)",
        "ANY SHORTEST",
        "(a) FOR EACH SEGMENT (e1) IN p WHERE TRUE",
    ] {
        let e = pattern(text).map(|_| panic!("{text}: accepted")).unwrap_err();
        assert_eq!(e.sqlcode(), -102_i32, "{text}: {e:?}");
    }
}
