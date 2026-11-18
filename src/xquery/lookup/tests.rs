//! Tests of the reduction of a query to an XML index's lookup.

use super::super::testing::compile_with;

/// The lookup `text` reduces to, as EXPLAIN shows it, and whether it
/// raises no error; none if it reduces to none.
fn reduced(text: &str) -> Option<(String, bool)> {
    let query = compile_with(text, &[]).unwrap();
    query.reduced().map(|r| (r.lookup.to_string(), r.raise_free))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §8.4 GR2] [XQuery
/// §3.2, §3.2.4] RULES.md D86: paths of child, descendant and attribute
/// steps over names and `*`, from the context item or `/`, `//` and
/// `self::node()` among them, reduce to the lookup of their steps, which
/// raises no error; a name in a namespace keeps it.
#[test]
fn paths_reduce() {
    for (text, lookup) in [
        ("/order/item", "/order/item"),
        ("order/item", "/order/item"),
        ("//item", "//item"),
        ("/order//name", "/order//name"),
        ("descendant::item/@id", "//item/@id"),
        ("//@id", "//@id"),
        ("/*/item", "/*/item"),
        ("./order/./item", "/order/item"),
        ("/order/self::node()/item", "/order/item"),
        ("declare namespace p = 'urn:p'; /p:order/p:item", "/Q{urn:p}order/Q{urn:p}item"),
        ("declare default element namespace 'urn:d'; //item/@id", "//Q{urn:d}item/@id"),
    ] {
        assert_eq!(reduced(text), Some((lookup.to_owned(), true)), "{text}");
    }
}

/// Verifies: [EXTENSION] [XQuery §3.2.2, §3.5.1, §3.5.2] RULES.md D15,
/// D86: a predicate of the last step, a relative path or its comparison
/// with a literal, continues the lookup: a general comparison with a
/// string is its comparison, raising no error; with a number too, which
/// may raise; a value comparison, or one with another literal, leaves
/// the path alone, and may raise; a string compared by another collation
/// leaves the path alone, raising none.
#[test]
fn predicates_reduce() {
    for (text, lookup, free) in [
        ("//item[price]", "//item/price", true),
        ("//item[@price > 5]", "//item/@price > 5", false),
        ("//item[5 > @price]", "//item/@price < 5", false),
        ("//item[5 < @price]", "//item/@price > 5", false),
        ("//item[5 = @price]", "//item/@price = 5", false),
        ("//item[5 != @price]", "//item/@price != 5", false),
        ("//item[5 <= @price]", "//item/@price >= 5", false),
        ("//item[5 >= @price]", "//item/@price <= 5", false),
        ("//item[@price <= 3]", "//item/@price <= 3", false),
        ("//item[name = 'pen']", "//item/name = 'pen'", true),
        ("//item[. = 'it''s']", "//item = 'it''s'", true),
        ("//item[.//name != 'x']", "//item//name != 'x'", true),
        ("//@id[. >= 7.5]", "//@id >= 7.5", false),
        ("//item[price eq 5]", "//item/price", false),
        ("//item[price = xs:date('2026-01-01')]", "", false),
        ("//item[price = true()]", "", false),
        ("declare default collation 'urn:turborel:collation:UNICODE'; //item[name = 'pen']", "//item/name", true),
    ] {
        let expected = (!lookup.is_empty()).then(|| (lookup.to_owned(), free));
        assert_eq!(reduced(text), expected, "{text}");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §8.4 GR2] RULES.md
/// D86: a query of any other shape reduces to none: a comparison, whose
/// value is never empty; a step of another axis or a kind test; a
/// predicate not of the last step, or two, or a positional one; a
/// wildcard of part of a name; a prolog's variable or function; `/`
/// alone, `.`, or a path ending at `//`.
#[test]
fn other_queries_reduce_to_none() {
    for text in [
        "//item = 'pen'",
        "//item/..",
        "//item/text()",
        "//item/following-sibling::x",
        "//item[name]/price",
        "//item[1]",
        "//item[a][b]",
        "//p:*",
        "declare namespace p = 'urn:p'; //p:*",
        "//*:item",
        "declare variable $x := 1; //item",
        "declare function local:f() { 1 }; //item",
        "/",
        ".",
        "//item[@a]/b",
        "(//item)[1]",
        "//@id/x",
        "//@id[x]",
        "//item[/a]",
        "//item[. is .]",
        "//item[name = name]",
        "//self::node()[a]",
        "//item/.[name]",
        "//item/(name)",
        "//item/self::node()[name]",
        "/order/descendant-or-self::node()",
        "//item[count(a) = 1]",
        "//item[. is 1]",
    ] {
        let reduced = compile_with(text, &[]).ok().and_then(|q| q.reduced()).map(|r| r.lookup.to_string());
        assert_eq!(reduced, None, "{text}");
    }
}
