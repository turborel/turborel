//! Tests of expressions: the operators and their precedence, the
//! extra-grammatical constraints and grammar notes, FLWOR, quantified,
//! typeswitch and conditional expressions, paths, sequence types and the
//! primary expressions. The direct constructors' are in
//! `direct/tests.rs`, the prolog's in `prolog/tests.rs`.

use crate::xquery::tests::{body, codes, kinds, round_trips, shapes, syntax_error, syntax_errors};
use crate::xquery::{
    Direction, EmptyOrder, Expr, Flwor, FlworClause, ForwardAxis, KindTest, NameTest, NodeTest, Path, QName, Quantifier, ReverseAxis, Root,
    Step, StepExpr,
};

/// The path the query body `text` is.
fn path(text: &str) -> Path {
    let Expr::Path(path) = body(text) else { panic!("{text} is no path") };
    *path
}

/// The step of the one-step path `text`.
fn step(text: &str) -> Step {
    let Some(StepExpr::Axis(axis)) = path(text).first else { panic!("{text} is no axis step") };
    axis.step
}

/// The name test of the name `local`.
fn name_test(local: &str) -> NodeTest {
    NodeTest::Name(NameTest::Name(QName::local(local)))
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.4]
/// [XQuery Appendix A.1 \[46\] to \[58\]] the operators by their
/// precedence, lowest first, each chain of one precedence left to right
/// in one node: `or`, `and`, comparisons, `to`, `+ -`, `* div idiv mod`,
/// `union |`, `intersect except`, the postfix operators, the signs.
#[test]
fn operators_by_precedence() {
    shapes(&[
        ("1 or 2 and 3 or 4", "(or 1 (and 2 3) 4)"),
        ("1 = 2 or 3 and 4 eq 5", "(or (General(Eq) 1 2) (and 3 (Value(Eq) 4 5)))"),
        ("1 to 2 + 3 = 4", "(General(Eq) (to 1 (2 Plus 3)) 4)"),
        ("1 + 2 * 3 - 4 div 5", "(1 Plus (2 Times 3) Minus (4 Div 5))"),
        ("1 idiv 2 mod 3 * $a | $b", "(1 Idiv 2 Mod 3 Times (union $a $b))"),
        ("$a union $b intersect $c except $d | $e", "(union $a ($b Intersect $c Except $d) $e)"),
        ("$a intersect $b instance of xs:int", "($a Intersect (instance-of $b xs:int))"),
        ("$a is $b", "(Node(Is) $a $b)"),
        ("$a << $b", "(Node(Precedes) $a $b)"),
        ("$a >> $b", "(Node(Follows) $a $b)"),
        ("1 != 2", "(General(Ne) 1 2)"),
        ("1 >= 2", "(General(Ge) 1 2)"),
        ("1 <= 2", "(General(Le) 1 2)"),
        ("1 > 2", "(General(Gt) 1 2)"),
        (
            "1 ne 2 or 1 lt 2 or 1 le 2 or 1 gt 2 or 1 ge 2",
            "(or (Value(Ne) 1 2) (Value(Lt) 1 2) (Value(Le) 1 2) (Value(Gt) 1 2) (Value(Ge) 1 2))",
        ),
        ("(1 + 2) * 3", "((paren (1 Plus 2)) Times 3)"),
        ("1, 2 + 3, 4", "(, 1 (2 Plus 3) 4)"),
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.1
/// \[48\], \[49\]] a comparison and a range have two operands each, so
/// neither chains without parentheses: err:XPST0003.
#[test]
fn comparisons_and_ranges_do_not_chain() {
    for text in ["1 = 2 = 3", "1 eq 2 lt 3", "$a is $b << $c", "1 to 2 to 3"] {
        assert!(syntax_error(text).contains("neither one itself"), "{text}");
    }
    assert!(syntax_error("1 to 2 to 3").contains("at line 1, column 8"));
    shapes(&[
        ("(1 = 2) = 3", "(General(Eq) (paren (General(Eq) 1 2)) 3)"),
        ("1 = 2 and 2 = 3", "(and (General(Eq) 1 2) (General(Eq) 2 3))"),
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.1
/// \[54\] to \[57\]] the postfix operators apply at most once each, `cast`
/// innermost, then `castable`, `treat` and `instance of`.
#[test]
fn postfix_operators_once_each_in_order() {
    shapes(&[
        ("1 cast as a castable as b treat as c instance of d", "(instance-of (treat (castable (cast 1 a) b) c) d)"),
        ("-1 cast as xs:int?", "(cast ([Minus] 1) xs:int?)"),
    ]);
    syntax_errors(&[
        "1 cast as a cast as b",
        "1 instance of a instance of b",
        "1 instance of a treat as b",
        "1 treat as a cast as b",
        "1 instance a",
        "1 cast xs:int",
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.1
/// \[58\]] a unary expression's signs, in the order written.
#[test]
fn unary_signs() {
    shapes(&[
        ("- + -1", "([Minus, Plus, Minus] 1)"),
        ("--1", "([Minus, Minus] 1)"),
        ("1 - -1", "(1 Minus ([Minus] 1))"),
        ("+$a", "([Plus] $a)"),
    ]);
    syntax_errors(&["-", "1 -"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.1.2,
/// occurrence-indicators] a `?`, `*` or `+` right after an item type is
/// its occurrence indicator, never an operator: `4 treat as item() + -
/// 5` is `(4 treat as item()+) - 5`, and `4 treat as item() + 5` is no
/// expression; a single type takes only `?`.
#[test]
fn occurrence_indicators_bind_to_item_types() {
    shapes(&[
        ("4 treat as item() + - 5", "((treat 4 item()+) Minus 5)"),
        ("(4 treat as item()) + -5", "((paren (treat 4 item())) Plus ([Minus] 5))"),
        ("$a instance of element()* ", "(instance-of $a element()*)"),
        ("$a instance of xs:int?", "(instance-of $a xs:int?)"),
        ("1 cast as xs:int * 2", "((cast 1 xs:int) Times 2)"),
    ]);
    assert!(syntax_error("4 treat as item() + 5").contains("the number 5 is not what the grammar allows here"));
    syntax_errors(&["1 cast as xs:int+"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.1.2,
/// leading-lone-slash] [XQuery §3.2] a `/` is a path alone or begins one;
/// a `*` or a keyword after it is a name test, so `/ * 5` is no
/// expression and `(/) * 5` and `5 * /` are; `//` begins a path and is
/// none alone.
#[test]
fn leading_lone_slash() {
    assert_eq!(path("/"), Path { root: Root::Slash, first: None, rest: Vec::new() });
    shapes(&[("(/) * 5", "((paren /) Times 5)"), ("5 * /", "(5 Times /)"), ("/ = 1", "(General(Eq) / 1)"), ("/ < 3", "(General(Lt) / 3)")]);
    syntax_errors(&["/ * 5", "/ union /", "/ div 2", "a/"]);
    round_trips(&["/<a/>", "/*", "/a"]);
    assert_eq!((path("/a").root, path("// a").root), (Root::Slash, Root::DoubleSlash));
    assert!(syntax_error("//").contains("'//' begins a path and is no path alone"));
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.2]
/// keywords are not reserved: where the grammar has no keyword, each is a
/// name, so `for`, `if`, `div div div` and `child::div` are paths, and a
/// variable may be named `for`.
#[test]
fn keywords_are_names_elsewhere() {
    for keyword in ["for", "let", "if", "return", "element", "declare", "xquery", "typeswitch", "validate", "some", "text", "ordered"] {
        assert_eq!(step(keyword), Step::Abbreviated(name_test(keyword)));
    }
    shapes(&[("div div div", "(div Div div)")]);
    round_trips(&["$for + $return", "child::div/for", "for $for in for return for"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.1.2,
/// reserved-function-names] [XQuery Appendix A.3] no function call is
/// named unprefixed by a reserved function name; prefixed, each is.
#[test]
fn reserved_function_names() {
    for name in ["empty-sequence", "item", "typeswitch", "if"] {
        assert!(syntax_error(&format!("a/{name}(1)")).contains("reserved function name"), "{name}");
    }
    let reserved = [
        "attribute",
        "comment",
        "document-node",
        "element",
        "empty-sequence",
        "if",
        "item",
        "node",
        "processing-instruction",
        "schema-attribute",
        "schema-element",
        "text",
        "typeswitch",
    ];
    syntax_errors(&reserved.map(|name| format!("{name}(1)")).iter().map(String::as_str).collect::<Vec<_>>());
    kinds(&reserved.map(|name| format!("p:{name}(1)")).iter().map(|t| (t.as_str(), "FunctionCall")).collect::<Vec<_>>());
    kinds(&[("fn:true()", "FunctionCall"), ("true()", "FunctionCall")]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.1.3,
/// grammar-note: parens] a name before a comment is no function call,
/// and a keyword before one still a keyword: `address (: c :)` is a
/// path, `for (: c :) $x in ...` a FLWOR expression; a `(` after the
/// comment makes a call.
#[test]
fn comments_between_a_name_and_what_follows() {
    kinds(&[
        ("address (: this may be empty :)", "Path"),
        ("for (: whom the bell :) $tolls in 3 return $tolls", "Flwor"),
        ("f (: c :) (1)", "FunctionCall"),
        ("5 instance (: strange place for a comment :) of xs:integer", "InstanceOf"),
    ]);
}

/// The FLWOR expression the query body `text` is.
fn flwor(text: &str) -> Flwor {
    let Expr::Flwor(flwor) = body(text) else { panic!("{text} is no FLWOR expression") };
    *flwor
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.8] [XQuery
/// Appendix A.1 \[33\] to \[37\]] FLWOR expressions: `for` with types and
/// positional variables, `let`, `where`, `return`; each clause as the
/// grammar has it.
#[test]
fn flwor_clauses() {
    let f = flwor("for $x as xs:int at $i in 1, $y in 2 let $z := 3 where 4 return 7");
    let FlworClause::For(bindings) = &f.clauses[0] else { panic!("for") };
    assert_eq!((bindings.len(), bindings[0].position.clone(), bindings[1].position.clone()), (2, Some(QName::local("i")), None));
    assert_eq!((bindings[0].declared.is_some(), bindings[1].declared.is_some()), (true, false));
    let FlworClause::Let(lets) = &f.clauses[1] else { panic!("let") };
    assert_eq!((lets.len(), f.condition.is_some(), f.order_by.is_none()), (1, true, true));
    syntax_errors(&[
        "for $x in 1",
        "let $x = 1 return 2",
        "for x in 1 return 2",
        "for $x 1 return 2",
        "for $x in 1 where return 2",
        "for $x at i in 1 return 2",
        "for $x in 1, return 2",
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.8.3] [XQuery
/// Appendix A.1 \[38\] to \[41\]] `order by` and `stable order by`, each
/// order specification with a direction, an empty order and a collation
/// if written.
#[test]
fn order_by_clauses() {
    let f = flwor("for $x in 1 stable order by 5 descending empty least collation 'c', 6 ascending empty greatest, 7 return 7");
    let order = f.order_by.unwrap();
    let modifiers: Vec<_> = order.specs.iter().map(|s| (s.direction, s.empty, s.collation.clone())).collect();
    assert_eq!(
        modifiers,
        vec![
            (Some(Direction::Descending), Some(EmptyOrder::Least), Some("c".to_owned())),
            (Some(Direction::Ascending), Some(EmptyOrder::Greatest), None),
            (None, None, None),
        ]
    );
    assert!(order.stable);
    assert!(!flwor("let $x := 1 order by $x return $x").order_by.unwrap().stable);
    syntax_errors(&[
        "for $x in 1 order 2 return 3",
        "for $x in 1 stable by 2 return 3",
        "for $x in 1 order by 2 empty return 3",
        "for $x in 1 order by 2 collation return 3",
        "for $x in 1 order by return 3",
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.11] [XQuery
/// Appendix A.1 \[42\]] quantified expressions: `some` or `every`, one or
/// more variables, `satisfies`.
#[test]
fn quantified_expressions() {
    let Expr::Quantified(every) = body("every $x as item() in 1, $y in 2 satisfies $x") else { panic!() };
    assert_eq!((every.quantifier, every.bindings.len()), (Quantifier::Every, 2));
    let Expr::Quantified(some) = body("some $x in 1 satisfies $x") else { panic!() };
    assert_eq!(some.quantifier, Quantifier::Some);
    syntax_errors(&["some $x in 1", "some $x satisfies 1", "every $x in 1 return 1", "some x in 1 satisfies 1"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.12.2] [XQuery
/// Appendix A.1 \[43\], \[44\]] typeswitch: one or more case clauses, each
/// with a variable if bound, then `default`, with a variable if bound.
#[test]
fn typeswitch_expressions() {
    let Expr::Typeswitch(t) = body("typeswitch ($a, $b) case $i as xs:int return 1 case node() return 2 default $d return 3") else {
        panic!()
    };
    assert_eq!((t.cases.len(), t.cases[0].var.is_some(), t.cases[1].var.is_some()), (2, true, false));
    assert_eq!((kind_of(&t.operand), t.default_var.clone()), ("Sequence".to_owned(), Some(QName::local("d"))));
    assert!(syntax_error("typeswitch ($a) default return 1").contains("one or more case clauses"));
    syntax_errors(&[
        "typeswitch ($a) case xs:int return 1",
        "typeswitch ($a) case $i xs:int return 1 default return 2",
        "typeswitch $a case xs:int return 1 default return 2",
    ]);
}

/// The kind of `expr`: its variant's name.
fn kind_of(expr: &Expr) -> String {
    format!("{expr:?}").split(['(', ' ']).next().unwrap().to_owned()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.10] [XQuery
/// Appendix A.1 \[45\]] a conditional expression has both its branches.
#[test]
fn conditional_expressions() {
    let Expr::If(expr) = body("if ($a, $b) then 1 else if (2) then 3 else 4") else { panic!() };
    assert_eq!(kind_of(&expr.otherwise), "If");
    syntax_errors(&["if ($a) then 1", "if ($a) 1 else 2", "if () then 1 else 2"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.2.1.1, §3.2.4]
/// [XQuery Appendix A.1 \[71\] to \[77\]] every forward and reverse axis,
/// and the abbreviations `@`, `..` and a step with no axis; a name
/// before `::` that is no axis is a syntax error.
#[test]
fn axes_and_abbreviations() {
    let forward = [
        ("child", ForwardAxis::Child),
        ("descendant", ForwardAxis::Descendant),
        ("attribute", ForwardAxis::Attribute),
        ("self", ForwardAxis::SelfAxis),
        ("descendant-or-self", ForwardAxis::DescendantOrSelf),
        ("following-sibling", ForwardAxis::FollowingSibling),
        ("following", ForwardAxis::Following),
    ];
    for (name, axis) in forward {
        assert_eq!(step(&format!("{name}::a")), Step::Forward(axis, name_test("a")));
    }
    let reverse = [
        ("parent", ReverseAxis::Parent),
        ("ancestor", ReverseAxis::Ancestor),
        ("preceding-sibling", ReverseAxis::PrecedingSibling),
        ("preceding", ReverseAxis::Preceding),
        ("ancestor-or-self", ReverseAxis::AncestorOrSelf),
    ];
    for (name, axis) in reverse {
        assert_eq!(step(&format!("{name}:: a")), Step::Reverse(axis, name_test("a")));
    }
    assert_eq!(
        [step("@a"), step("@*"), step("..")],
        [Step::Attribute(name_test("a")), Step::Attribute(NodeTest::Name(NameTest::Any)), Step::Parent]
    );
    assert!(syntax_error("sideways::a").contains("'::' follows the name of an axis"));
    syntax_errors(&["child::", "p:child::a", "child:: ::a"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.2] [XQuery
/// Appendix A.1 \[68\] to \[70\], \[81\] to \[83\]] paths of steps after `/`
/// and `//`, each an axis step or a filter expression with predicates;
/// a filter expression alone is no path.
#[test]
fn paths_steps_and_predicates() {
    let p = path("a//b[1][2]/(c, d)[3]/f(1)/.");
    assert_eq!((p.root, p.rest.len()), (Root::Relative, 4));
    let steps: Vec<String> = p.rest.iter().map(|(_, s)| s.to_string()).collect();
    assert_eq!(steps, ["b[1][2]", "(c, d)[3]", "f(1)", "."]);
    let StepExpr::Axis(axis) = &p.rest[0].1 else { panic!() };
    assert_eq!(axis.predicates.len(), 2);
    kinds(&[("$a[1]", "Filter"), ("$a", "Variable"), ("a", "Path"), ("(1)[1]", "Filter"), ("a[1]", "Path")]);
    syntax_errors(&["a[1", "a[]", "a//", "a[1]]"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.2.1.2] [XQuery
/// Appendix A.1 \[78\] to \[80\], \[123\] to \[139\]] node tests: names,
/// the wildcards `*`, `p:*` and `*:l`, and every kind test with the
/// arguments its production allows.
#[test]
fn node_tests_and_kind_tests() {
    assert_eq!(step("p:*"), Step::Abbreviated(NodeTest::Name(NameTest::Prefix("p".to_owned()))));
    assert_eq!(step("*:l"), Step::Abbreviated(NodeTest::Name(NameTest::Local("l".to_owned()))));
    assert_eq!(step("child::*"), Step::Forward(ForwardAxis::Child, NodeTest::Name(NameTest::Any)));
    for (test, kind) in [
        ("node()", KindTest::AnyKind),
        ("text()", KindTest::Text),
        ("comment()", KindTest::Comment),
        ("processing-instruction()", KindTest::Pi(None)),
        ("element()", KindTest::Element(None)),
        ("attribute()", KindTest::Attribute(None)),
        ("document-node()", KindTest::Document(None)),
    ] {
        assert_eq!(step(test), Step::Abbreviated(NodeTest::Kind(kind)), "{test}");
    }
    round_trips(&[
        "element(*)",
        "element(a, t)",
        "element(a, t?)",
        "element(*, t)",
        "attribute(*, t)",
        "attribute(a)",
        "schema-element(a)",
        "schema-attribute(a)",
        "processing-instruction(t)",
        "processing-instruction(\"t\")",
        "document-node(element(a))",
        "document-node(element())",
        "document-node(schema-element(a))",
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.1
/// \[123\] to \[139\]] a kind test takes only the arguments its production
/// allows.
#[test]
fn kind_tests_take_their_arguments_alone() {
    syntax_errors(&[
        "element(*:a)",
        "element(a, b, c)",
        "element(a,)",
        "attribute(a, t?)",
        "processing-instruction(p:a)",
        "document-node(attribute())",
        "document-node(element(a)",
        "schema-element()",
        "schema-element(*)",
        "text(1)",
        "node(a)",
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §2.5.3] [XQuery
/// Appendix A.1 \[117\] to \[122\]] sequence types: `empty-sequence()`,
/// `item()`, kind tests and atomic types, each item type with an
/// occurrence indicator if written; `empty-sequence()` takes none.
#[test]
fn sequence_types() {
    round_trips(&[
        "$a instance of empty-sequence()",
        "$a instance of item()",
        "$a instance of item()?",
        "$a instance of node()*",
        "$a instance of document-node(element(a))+",
        "$a instance of xs:integer",
        "$a treat as schema-attribute(a)?",
        "$a instance of element",
    ]);
    syntax_errors(&[
        "$a instance of empty-sequence()?",
        "$a instance of item(1)",
        "$a instance of 1",
        "$a instance of *",
        "$a instance of p:*",
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.1] [XQuery
/// Appendix A.1 \[84\] to \[93\]] primary expressions: literals, variable
/// references, parenthesized expressions empty or not, the context item,
/// function calls, ordered and unordered expressions.
#[test]
fn primary_expressions() {
    kinds(&[
        ("()", "Parenthesized"),
        (".", "ContextItem"),
        ("$ p:x", "Variable"),
        ("f()", "FunctionCall"),
        ("ordered { 1 }", "Ordered"),
        ("unordered { 1, 2 }", "Unordered"),
        ("1", "Literal"),
    ]);
    let Expr::FunctionCall(_, arguments) = body("f(1, (2, 3), 4)") else { panic!() };
    assert_eq!(arguments.len(), 3);
    round_trips(&["1.0e0, 01, 1., .5, \"x\"", "$p:x", "f()", "()"]);
    syntax_errors(&["f(1,)", "f(1", "$", "(1", ")", "ordered { }", "1 2", "f(1 2)", "$1"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.7.3] [XQuery
/// Appendix A.1 \[109\] to \[116\]] computed constructors: `document`,
/// `element` and `attribute` named or by an expression, `text`,
/// `comment` and `processing-instruction` with an `NCName` or an
/// expression; content empty only where the grammar allows.
#[test]
fn computed_constructors() {
    round_trips(&[
        "document { 1 }",
        "element a { }",
        "element p:a { 1 }",
        "element { $n } { 1 }",
        "attribute a { }",
        "attribute { \"n\" } { \"v\" }",
        "text { 1 }",
        "comment { \"c\" }",
        "processing-instruction p { }",
        "processing-instruction { \"p\" } { \"x\" }",
        "element element { element div { } }",
    ]);
    syntax_errors(&[
        "document { }",
        "text { }",
        "comment { }",
        "element a",
        "element a { 1",
        "processing-instruction p:a { 1 }",
        "element * { }",
    ]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery §3.13, §3.14]
/// [XQuery Appendix A.1 \[63\] to \[67\]] validate expressions with or
/// without a mode, and extension expressions of one or more pragmas and
/// an expression in braces, which may be absent.
#[test]
fn validate_and_extension_expressions() {
    round_trips(&["validate { 1 }", "validate lax { 1 }", "validate strict { 1 }", "(#p:a #) (#b c#) { 1 }", "(#p#) { }"]);
    kinds(&[("validate", "Path"), ("(# p:a #) { 1 }", "Extension")]);
    let Expr::Extension(pragmas, None) = body("(#a#)(#b#){}") else { panic!() };
    assert_eq!(pragmas.len(), 2);
    syntax_errors(&["validate lax", "validate { }", "(#p#)", "(#p#) 1", "validate lax 1"]);
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix F] a
/// query parses as a whole: what follows its body, or a token where the
/// grammar has none, is err:XPST0003.
#[test]
fn a_query_is_one_module() {
    codes(&[("1 2", "XPST0003"), ("1;", "XPST0003"), ("", "XPST0003"), ("(: only a comment :)", "XPST0003")]);
}
