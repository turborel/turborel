//! Tests of parsing an SQL/JSON path expression: its mode, wffs,
//! accessors, predicates, the Syntax Rules checked, and the bounds.

use super::*;
use crate::json::path::{Comparison, Predicate};
use crate::value::Value;

/// The tree of `text`, written out: each operation in parentheses, a
/// member's key quoted, a method by its name.
fn tree(text: &str) -> String {
    let path = parse_path(text).unwrap_or_else(|e| panic!("{text}: {}", e.msg));
    format!("{:?} {}", path.mode, expr(&path.expr))
}

fn expr(e: &Expr) -> String {
    match e {
        Expr::Literal(item) => literal(item),
        Expr::Context => "$".to_owned(),
        Expr::Variable(name) => format!("${name}"),
        Expr::Current => "@".to_owned(),
        Expr::Last => "last".to_owned(),
        Expr::Access(base, op) => format!("{}{}", expr(base), accessor(op)),
        Expr::Signed(Sign::Plus, operand) => format!("(+{})", expr(operand)),
        Expr::Signed(Sign::Minus, operand) => format!("(-{})", expr(operand)),
        Expr::Arithmetic(left, op, right) => {
            let symbol = match op {
                Operator::Multiply => "*",
                Operator::Divide => "/",
                Operator::Modulo => "%",
                Operator::Add => "+",
                Operator::Subtract => "-",
            };
            format!("({} {symbol} {})", expr(left), expr(right))
        }
    }
}

fn literal(item: &Item) -> String {
    let Item::Scalar(scalar) = item else { return "null".to_owned() };
    let value = scalar.value();
    if let Value::Exact(d) = value {
        return if d.scale == 0 { format!("{}", d.units) } else { format!("{}/{}", d.units, d.scale) };
    }
    if let Value::Approx(x) = value {
        return format!("~{x}");
    }
    if let Value::Char(s) = value {
        return format!("'{s}'");
    }
    if let Value::Bool(b) = value {
        return format!("{b}");
    }
    format!("{value:?}")
}

fn accessor(op: &Accessor) -> String {
    match op {
        Accessor::Member(key) => format!(".\"{key}\""),
        Accessor::AnyMember => ".*".to_owned(),
        Accessor::Subscripts(list) => {
            let each: Vec<String> = list
                .iter()
                .map(|s| s.to.as_ref().map_or_else(|| expr(&s.from), |to| format!("{} to {}", expr(&s.from), expr(to))))
                .collect();
            format!("[{}]", each.join(", "))
        }
        Accessor::AnyElement => "[*]".to_owned(),
        Accessor::Filter(p) => format!("?({})", predicate(p)),
        Accessor::Method(m) => format!(".{m:?}()"),
    }
}

fn predicate(p: &Predicate) -> String {
    match p {
        Predicate::Exists(e) => format!("exists({})", expr(e)),
        Predicate::Compare(l, op, r) => {
            let symbol = match op {
                Comparison::Equal => "==",
                Comparison::NotEqual => "!=",
                Comparison::Less => "<",
                Comparison::Greater => ">",
                Comparison::LessOrEqual => "<=",
                Comparison::GreaterOrEqual => ">=",
            };
            format!("({} {symbol} {})", expr(l), expr(r))
        }
        Predicate::LikeRegex(e, _) => format!("({} like_regex)", expr(e)),
        Predicate::StartsWith(e, initial) => format!("({} starts with {})", expr(e), expr(initial)),
        Predicate::IsUnknown(inner) => format!("({} is unknown)", predicate(inner)),
        Predicate::Not(inner) => format!("!{}", predicate(inner)),
        Predicate::And(l, r) => format!("({} && {})", predicate(l), predicate(r)),
        Predicate::Or(l, r) => format!("({} || {})", predicate(l), predicate(r)),
    }
}

/// The regular expression of the like_regex predicate that is the one
/// filter of `text`.
fn regex_of(text: &str) -> crate::regex::Regex {
    let path = parse_path(text).unwrap();
    let Expr::Access(_, Accessor::Filter(filter)) = path.expr else { panic!("a filter") };
    let Predicate::LikeRegex(Expr::Current, regex) = *filter else { panic!("like_regex of @") };
    regex
}

fn refused(text: &str) -> SqlError {
    match parse_path(text) {
        Ok(path) => panic!("{text} parsed: {}", expr(&path.expr)),
        Err(e) => e,
    }
}

fn code(text: &str) -> i32 {
    refused(text).sqlcode()
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 F] a `<JSON path
/// expression>` is its `<JSON path mode>`, strict or lax, written in
/// lower case, then a wff: without its mode, a wff, or with tokens after
/// it, it is SQLCODE -102.
#[test]
fn a_path_is_its_mode_and_a_wff() {
    assert_eq!(tree("lax $"), "Lax $");
    assert_eq!(tree("strict $"), "Strict $");
    assert_eq!(tree("lax\n$"), "Lax $");
    for text in ["", "$", "$.a", "LAX $", "Strict $", "lax", "strict", "lax $ $", "lax $ )", "lax lax $", "lax 1 2"] {
        assert_eq!(code(text), -102_i32, "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 F, §9.45 SR4, GR1] a
/// `<JSON path primary>` is a literal, number, string, true, false or
/// null; the context variable `$`; a named variable, `$` and its name; or
/// a parenthesized wff; a key name, or a symbol that begins none, is
/// SQLCODE -102.
#[test]
fn primaries_are_literals_variables_and_parenthesized_wffs() {
    assert_eq!(tree("lax 1.50"), "Lax 150/2");
    assert_eq!(tree("lax 1e1"), "Lax ~10");
    assert_eq!(tree("lax \"a\""), "Lax 'a'");
    assert_eq!(tree("lax 'a'"), "Lax 'a'");
    assert_eq!(tree("lax true"), "Lax true");
    assert_eq!(tree("lax false"), "Lax false");
    assert_eq!(tree("lax null"), "Lax null");
    assert_eq!(tree("lax $x"), "Lax $x");
    assert_eq!(tree("lax $$"), "Lax $$");
    assert_eq!(tree("lax ((1))"), "Lax 1");
    for text in ["lax a", "lax True", "lax exists", "lax ]", "lax *", "lax (1", "lax ()", "lax (1 2)", "lax $ x"] {
        assert_eq!(code(text), -102_i32, "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 F, §9.45 SR2d, NOTE
/// 483] the accessor ops: a member by key name or string literal, `.*`,
/// `[*]`, subscripts and ranges, filters and item methods, applied in
/// turn; a key word with no `(` after it is a key name.
#[test]
fn accessor_ops_apply_in_turn() {
    assert_eq!(tree("lax $.a"), "Lax $.\"a\"");
    assert_eq!(tree("lax $.\"a b\".'c'"), "Lax $.\"a b\".\"c\"");
    assert_eq!(tree("lax $.*[*]"), "Lax $.*[*]");
    assert_eq!(tree("lax $[0]"), "Lax $[0]");
    assert_eq!(tree("lax $[1, 2 to 3, $.a to last]"), "Lax $[1, 2 to 3, $.\"a\" to last]");
    assert_eq!(tree("lax $.a[0].b"), "Lax $.\"a\"[0].\"b\"");
    assert_eq!(tree("lax $ ? (@ > 1).a"), "Lax $?((@ > 1)).\"a\"");
    assert_eq!(tree("lax $x.a"), "Lax $x.\"a\"");
    assert_eq!(tree("lax (1).type()"), "Lax 1.Type()");
    assert_eq!(tree("lax $.type"), "Lax $.\"type\"");
    for word in ["exists", "last", "lax", "strict", "true", "null", "to", "is", "with", "datetime", "keyvalue", "a1", "é"] {
        assert_eq!(tree(&format!("lax $.{word}")), format!("Lax $.\"{word}\""), "{word}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 F] the item methods
/// turborel implements, each written with `()` and nothing in it; a
/// method of another name, or with something in its parentheses, is
/// SQLCODE -102.
#[test]
fn item_methods_are_parsed() {
    assert_eq!(tree("lax $.type()"), "Lax $.Type()");
    assert_eq!(tree("lax $.size ( )"), "Lax $.Size()");
    assert_eq!(tree("lax $.double().ceiling().floor().abs()"), "Lax $.Double().Ceiling().Floor().Abs()");
    for text in ["lax $.foo()", "lax $.a(", "lax $.type(1)", "lax $.type(", "lax $.Type()"] {
        assert_eq!(code(text), -102_i32, "{text}");
    }
}

/// Verifies: [DESIGN] [ISO/IEC 9075-2:2023 §9.46 Conformance Rules 2, 8
/// to 21] RULES.md D18: an item method turborel does not implement,
/// written as a method, is SQLCODE -900 naming it and its Feature; its
/// name with no `(` after it is a key name.
#[test]
fn methods_not_implemented_are_refused() {
    for (name, feature) in [
        ("datetime", "T832"),
        ("keyvalue", "T832"),
        ("bigint", "T865"),
        ("boolean", "T866"),
        ("date", "T867"),
        ("decimal", "T868"),
        ("integer", "T870"),
        ("number", "T871"),
        ("string", "T872"),
        ("time", "T873"),
        ("time_tz", "T874"),
        ("timestamp", "T876"),
        ("timestamp_tz", "T877"),
    ] {
        let error = refused(&format!("lax $.{name}()"));
        assert_eq!(error.sqlcode(), -900_i32, "{name}");
        assert!(error.msg.contains(&format!("{name}() (Feature {feature}")), "{}", error.msg);
        assert_eq!(tree(&format!("lax $.{name}")), format!("Lax $.\"{name}\""));
    }
    assert_eq!(code("lax $.datetime(\"HH24\")"), -900_i32);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 F] what follows `.`
/// or `[` is to be an accessor: `.` and nothing, a number, a symbol or a
/// variable, `[` not closed, empty, or `[*` not closed, are SQLCODE -102.
#[test]
fn malformed_accessors_are_refused() {
    for text in [
        "lax $.",
        "lax $.[0]",
        "lax $.(",
        "lax $.$a",
        "lax $.$",
        "lax $.1",
        "lax $[",
        "lax $[]",
        "lax $[1",
        "lax $[1,]",
        "lax $[*",
        "lax $[*,1]",
        "lax $[1 to]",
        "lax $?",
        "lax $?@",
        "lax $?(@ > 1",
        "lax $.a b",
    ] {
        assert_eq!(code(text), -102_i32, "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 F] precedence as the
/// Format gives it: accessor ops bind tighter than signs, signs than `*`,
/// `/` and `%`, and those than `+` and `-`; binary operators associate to
/// the left.
#[test]
fn operators_bind_as_the_format_nests_them() {
    assert_eq!(tree("lax -$.a"), "Lax (-$.\"a\")");
    assert_eq!(tree("lax - 1 .type()"), "Lax (-1.Type())");
    assert_eq!(tree("lax +-1"), "Lax (+(-1))");
    assert_eq!(tree("lax 1 + 2 * 3"), "Lax (1 + (2 * 3))");
    assert_eq!(tree("lax 1 - 2 - 3"), "Lax ((1 - 2) - 3)");
    assert_eq!(tree("lax 8 / 4 % 3 * 2"), "Lax (((8 / 4) % 3) * 2)");
    assert_eq!(tree("lax (1 + 2) * -3"), "Lax ((1 + 2) * (-3))");
    assert_eq!(tree("lax $.a - -$.b"), "Lax ($.\"a\" - (-$.\"b\"))");
    for text in ["lax 1 +", "lax 1 * * 2", "lax -", "lax 1 + )"] {
        assert_eq!(code(text), -102_i32, "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 SR5] `@` is the
/// current item within a filter's predicate, at any depth in it, but
/// outside one, even after one closes, is SQLCODE -103 citing SR5.
#[test]
fn the_current_item_is_only_within_a_predicate() {
    assert_eq!(tree("lax $ ? (@ == 1)"), "Lax $?((@ == 1))");
    assert_eq!(tree("lax $ ? ($[@] == @.a)"), "Lax $?(($[@] == @.\"a\"))");
    assert_eq!(tree("lax $ ? (@ ? (@ > 1) == 1)"), "Lax $?((@?((@ > 1)) == 1))");
    for text in ["lax @", "lax $[@]", "lax $ ? (@ > 1) + @", "lax @.a ? (@ > 1)"] {
        let error = refused(text);
        assert_eq!(error.sqlcode(), -103_i32, "{text}");
        assert_eq!(error.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 SR5"));
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 SR6] `last` is a
/// primary within a subscript, a filter's within one too, but outside
/// one, even after one closes, is SQLCODE -103 citing SR6.
#[test]
fn last_is_only_within_a_subscript() {
    assert_eq!(tree("lax $[last]"), "Lax $[last]");
    assert_eq!(tree("lax $[last - 1 to last]"), "Lax $[(last - 1) to last]");
    assert_eq!(tree("lax $[$ ? (@ == last)]"), "Lax $[$?((@ == last))]");
    for text in ["lax last", "lax $ ? (@ == last)", "lax $[0] + last", "lax -last"] {
        let error = refused(text);
        assert_eq!(error.sqlcode(), -103_i32, "{text}");
        assert_eq!(error.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 SR6"));
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 F, NOTE 489, NOTE 490]
/// each `<JSON comp op>`, `<>` and `!=` alike, of one precedence and not
/// associative: a comparison of a comparison is SQLCODE -102, and so is
/// a wff alone where a predicate is.
#[test]
fn comparisons_do_not_associate() {
    for (op, written) in [("==", "=="), ("!=", "!="), ("<>", "!="), ("<", "<"), (">", ">"), ("<=", "<="), (">=", ">=")] {
        assert_eq!(tree(&format!("lax $ ? (@ {op} 1)")), format!("Lax $?((@ {written} 1))"));
    }
    assert_eq!(tree("lax $ ? (@.a + 1 == 2 * @.b)"), "Lax $?(((@.\"a\" + 1) == (2 * @.\"b\")))");
    for text in
        ["lax $ ? (1 == 2 == 3)", "lax $ ? (1 < 2 > 3)", "lax $ ? (@)", "lax $ ? (@.a)", "lax $ ? (@ ==)", "lax $ ? ()", "lax 1 == 1"]
    {
        assert_eq!(code(text), -102_i32, "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 F] `&&` binds tighter
/// than `||`, both to the left; `!` negates a delimited predicate only,
/// `exists (…)` or a parenthesized predicate; `exists` with no `(` after
/// it is a key name, no primary.
#[test]
fn boolean_operators_and_negation() {
    assert_eq!(tree("lax $ ? (@ == 1 || @ == 2 && @ == 3)"), "Lax $?(((@ == 1) || ((@ == 2) && (@ == 3))))");
    assert_eq!(tree("lax $ ? (@ == 1 && @ == 2 && @ == 3)"), "Lax $?((((@ == 1) && (@ == 2)) && (@ == 3)))");
    assert_eq!(tree("lax $ ? (@ == 1 || @ == 2 || @ == 3)"), "Lax $?((((@ == 1) || (@ == 2)) || (@ == 3)))");
    assert_eq!(tree("lax $ ? (exists (@.a))"), "Lax $?(exists(@.\"a\"))");
    assert_eq!(tree("lax $ ? (!exists(@.a) && !(@ > 1))"), "Lax $?((!exists(@.\"a\") && !(@ > 1)))");
    assert_eq!(tree("lax $ ? (!(!(@ > 1)))"), "Lax $?(!!(@ > 1))");
    assert_eq!(code("lax $ ? (!!(@ > 1))"), -102_i32);
    let texts = [
        "lax $ ? (!@ == 1)",
        "lax $ ? (!(@) == 1)",
        "lax $ ? (exists == 1)",
        "lax $ ? (exists(@)",
        "lax $ ? (exists(@) == 1)",
        "lax $ ? (@ == 1 &&)",
        "lax $ ? (@ == 1 || @)",
        "lax $ ? (@ == 1 && (@))",
        "lax $ ? (!(@ > 1) is unknown)",
        "lax $ ? (!)",
        "lax $ ? (!(@ > 1)",
    ];
    for text in texts {
        assert_eq!(code(text), -102_i32, "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 F] a `(` where a
/// predicate begins opens a parenthesized predicate, a `<JSON unknown
/// predicate>`, or the primary of a wff a comparison, like_regex or
/// starts with begins: each is parsed, nested too, and `is` without
/// `unknown`, or a parenthesized wff alone, is SQLCODE -102.
#[test]
fn a_parenthesis_opens_a_predicate_or_a_wff() {
    assert_eq!(tree("lax $ ? ((@ > 1))"), "Lax $?((@ > 1))");
    assert_eq!(tree("lax $ ? ((@ > 1) is unknown)"), "Lax $?(((@ > 1) is unknown))");
    assert_eq!(tree("lax $ ? ((@ > 1) is unknown || @ == 2)"), "Lax $?((((@ > 1) is unknown) || (@ == 2)))");
    assert_eq!(tree("lax $ ? (($.a + 1) == 2)"), "Lax $?((($.\"a\" + 1) == 2))");
    assert_eq!(tree("lax $ ? ((($.a)) == 1)"), "Lax $?(($.\"a\" == 1))");
    assert_eq!(tree("lax $ ? (($.a).b * 2 + 1 > 0)"), "Lax $?(((($.\"a\".\"b\" * 2) + 1) > 0))");
    assert_eq!(tree("lax $ ? ((-@) == 1)"), "Lax $?(((-@) == 1))");
    assert_eq!(tree("lax $ ? ((@ == 1) && (@ == 2))"), "Lax $?(((@ == 1) && (@ == 2)))");
    assert_eq!(tree("lax $ ? (((@ == 1)) || ((@) == 2))"), "Lax $?(((@ == 1) || (@ == 2)))");
    assert_eq!(tree("lax $ ? ((@) like_regex \"a\")"), "Lax $?((@ like_regex))");
    assert_eq!(tree("lax $ ? ((@) starts with \"a\")"), "Lax $?((@ starts with 'a'))");
    for text in ["lax $ ? ((@))", "lax $ ? ((@ > 1) is)", "lax $ ? ((@ > 1) is known)", "lax $ ? ((@ > 1)", "lax $ ? ((@) && @ == 1)"] {
        assert_eq!(code(text), -102_i32, "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 F, SR7] like_regex
/// takes a string literal pattern and, after `flag`, string literal
/// flags, compiled when parsed; a pattern that is no XQuery regular
/// expression is SQLSTATE 2201S, flags that are no XQuery option flag
/// 2201T, and no string literal where one is SQLCODE -102.
#[test]
fn like_regex_compiles_its_pattern() {
    let caseless = regex_of("lax $ ? (@ like_regex \"^a.c$\" flag \"i\")");
    assert!(caseless.is_match("AbC").unwrap());
    assert!(!caseless.is_match("abd").unwrap());
    assert_eq!(tree("lax $ ? (@.a like_regex 'b')"), "Lax $?((@.\"a\" like_regex))");
    assert!(!regex_of("lax $ ? (@ like_regex \"^a\")").is_match("A").unwrap());
    assert_eq!(refused("lax $ ? (@ like_regex \"(\")").sqlstate().as_str(), "2201S");
    assert_eq!(refused("lax $ ? (@ like_regex \"a\" flag \"z\")").sqlstate().as_str(), "2201T");
    for text in [
        "lax $ ? (@ like_regex)",
        "lax $ ? (@ like_regex 1)",
        "lax $ ? (@ like_regex $x)",
        "lax $ ? (@ like_regex \"a\" flag)",
        "lax $ ? (@ like_regex \"a\" flag i)",
    ] {
        assert_eq!(code(text), -102_i32, "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 F] starts with takes
/// as its initial a string literal or a named variable only: the context
/// variable, a number, a key name or a wff is SQLCODE -102, as is `starts`
/// without `with`.
#[test]
fn starts_with_takes_a_string_or_a_named_variable() {
    assert_eq!(tree("lax $ ? (@ starts with \"ab\")"), "Lax $?((@ starts with 'ab'))");
    assert_eq!(tree("lax $ ? (@.a starts with $p)"), "Lax $?((@.\"a\" starts with $p))");
    let texts = [
        "lax $ ? (@ starts with $)",
        "lax $ ? (@ starts with 1)",
        "lax $ ? (@ starts with a)",
        "lax $ ? (@ starts with (\"a\"))",
        "lax $ ? (@ starts with)",
        "lax $ ? (@ starts \"a\")",
        "lax $ ? (@ starts with \"a\" + 1)",
    ];
    for text in texts {
        assert_eq!(code(text), -102_i32, "{text}");
    }
}

/// Verifies: [DESIGN] RULES.md R-CF-6: parentheses nested `MAX_NESTING`
/// deep are parsed, one more is SQLCODE -900, around a wff and around a
/// predicate alike; so are signs, negations, filters and subscripts.
#[test]
fn nesting_around_what_is_read_is_bounded() {
    let parens = |levels: usize| format!("lax {}1{}", "(".repeat(levels), ")".repeat(levels));
    assert_eq!(tree(&parens(MAX_NESTING)), "Lax 1");
    assert_eq!(code(&parens(MAX_NESTING + 1)), -900_i32);
    let predicates = |levels: usize| format!("lax $ ? ({}@ == 1{})", "(".repeat(levels), ")".repeat(levels));
    assert!(parse_path(&predicates(MAX_NESTING - 1)).is_ok());
    assert_eq!(code(&predicates(MAX_NESTING)), -900_i32);
    let negations = |levels: usize| format!("lax $ ? ({}(@ == 1){})", "!(".repeat(levels), ")".repeat(levels));
    assert!(parse_path(&negations(MAX_NESTING - 3)).is_ok());
    assert_eq!(code(&negations(MAX_NESTING - 2)), -900_i32);
    assert_eq!(code(&negations(MAX_NESTING)), -900_i32);
    let subscripts = |levels: usize| format!("lax {}0{}", "$[".repeat(levels), "]".repeat(levels));
    assert!(parse_path(&subscripts(MAX_NESTING - 1)).is_ok());
    assert_eq!(code(&subscripts(MAX_NESTING + 1)), -900_i32);
    let exists = |levels: usize| format!("lax $ ? ({}@ == 1{})", "exists($ ? (".repeat(levels), "))".repeat(levels));
    assert!(parse_path(&exists(30)).is_ok());
    assert_eq!(code(&exists(MAX_NESTING)), -900_i32);
    let signs = |levels: usize| format!("lax {}1", "-".repeat(levels));
    assert!(parse_path(&signs(MAX_NESTING - 1)).is_ok());
    assert_eq!(code(&signs(MAX_NESTING + 1)), -900_i32);
}

/// Verifies: [DESIGN] RULES.md R-CF-6: a syntax tree `MAX_NESTING` nodes
/// high is built, one a node higher is SQLCODE -900, whether built by a
/// chain of operators, of accessors, of `&&` or of `||`, or by signs; so
/// a walk of the tree is bounded.
#[test]
fn the_syntax_trees_height_is_bounded() {
    let sums = |operators: usize| format!("lax 1{}", " + 1".repeat(operators));
    assert!(parse_path(&sums(MAX_NESTING - 1)).is_ok());
    assert_eq!(code(&sums(MAX_NESTING)), -900_i32);
    let products = |operators: usize| format!("lax 1{}", " * 1".repeat(operators));
    assert!(parse_path(&products(MAX_NESTING - 1)).is_ok());
    assert_eq!(code(&products(MAX_NESTING)), -900_i32);
    let members = |accessors: usize| format!("lax ${}", ".a".repeat(accessors));
    assert!(parse_path(&members(MAX_NESTING - 1)).is_ok());
    assert_eq!(code(&members(MAX_NESTING)), -900_i32);
    let signs = |levels: usize| format!("lax {}1", "-".repeat(levels));
    assert_eq!(code(&signs(MAX_NESTING)), -900_i32);
    // The filter's Access, its && or || chain, then each comparison over its two leaves.
    let ands = |operators: usize| format!("lax $ ? (@ == 1{})", " && @ == 1".repeat(operators));
    assert!(parse_path(&ands(MAX_NESTING - 3)).is_ok());
    assert_eq!(code(&ands(MAX_NESTING - 2)), -900_i32);
    let ors = |operators: usize| format!("lax $ ? (@ == 1{})", " || @ == 1".repeat(operators));
    assert!(parse_path(&ors(MAX_NESTING - 3)).is_ok());
    assert_eq!(code(&ors(MAX_NESTING - 2)), -900_i32);
    let wide = format!("lax $[{}]", vec!["1"; 1000].join(", "));
    assert!(parse_path(&wide).is_ok());
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-2:2023 §9.46 F, §9.45 SR2] a
/// refusal names its rule and the character it was refused at, counted
/// from 1, the end of the text one past its last; a lexical refusal is
/// SQLCODE -101 through the parser too.
#[test]
fn a_refusal_names_its_rule_and_place() {
    let error = refused("lax $ ]");
    assert_eq!(error.rule.as_deref(), Some("ISO/IEC 9075-2:2023 §9.46 F"));
    assert!(error.msg.contains("(at character 7)"), "{}", error.msg);
    let end = refused("lax $ +");
    assert!(end.msg.contains("(at character 8)"), "{}", end.msg);
    let at = refused("lax $.a + @");
    assert!(at.msg.contains("(at character 11)"), "{}", at.msg);
    assert_eq!(code("lax $ # 1"), -101_i32);
    assert_eq!(code("lax 0x1"), -900_i32);
}
