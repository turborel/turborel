//! Tests of the reduction of a query to an XML index's lookup.

use super::super::testing::compile_with;

/// The lookup `text` reduces to, as EXPLAIN shows it, and whether it
/// raises no error; none if it reduces to none.
fn reduced(text: &str) -> Option<(String, bool)> {
    let query = compile_with(text, &[]).unwrap();
    query.reduced().map(|r| (r.query.to_string(), r.raise_free))
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §8.4 GR2] [XQuery
/// §3.2, §3.2.4] RULES.md D106: paths of child, descendant and attribute
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
/// D106: a predicate of the last step, a relative path or its comparison
/// with a literal, continues the lookup: a general comparison with a
/// string is its comparison, raising no error; with a number too, which
/// may raise; a value comparison, or one with another literal, leaves
/// the path alone, and may raise; a string compared by another collation
/// leaves the path alone, raising none.
#[test]
fn predicates_reduce() {
    for (text, lookup, free) in [
        ("//item[price]", "//item/price", true),
        ("//item/.[price]", "//item/price", true),
        ("//item/self::node()[price]", "//item/price", true),
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

/// The query `text` reduces to, whether it raises no error, whether it is
/// exact, and the places of its lookups that may raise; none if it
/// reduces to none.
fn exactly(text: &str) -> Option<(String, bool, bool, Vec<usize>)> {
    let query = compile_with(text, &[]).unwrap();
    query.reduced().map(|r| (r.query.to_string(), r.raise_free, r.exact, r.raising))
}

/// Verifies: [EXTENSION] [XQuery §3.2.2, §3.6] [Pal et al. §3.1.2]
/// RULES.md D15, D106: a predicate of a step before the last, a relative
/// path or its comparison, is a filter of that step's node, of any step,
/// as many as there are, each conjunct of `and` one, and the reduction is
/// exact if every comparison is kept, none raises, and each filter's
/// anchor is fixed, of child steps after it or before it; a comparison
/// with a number, or a value comparison, of a filter, is of a lookup that
/// may raise; a predicate of `.` of a step before the last reduces to
/// none.
#[test]
fn predicates_of_steps_are_filters() {
    let unicode = "declare default collation 'urn:turborel:collation:UNICODE';";
    for (text, expected) in [
        ("//item[name]/price", Some(("//item[name]/price", true, true, vec![]))),
        ("/BOOK[@ISBN = '1']/SECTION", Some(("/BOOK[@ISBN = '1']/SECTION", true, true, vec![]))),
        ("/BOOK[@ISBN = '1']/SECTION[TITLE = 'x']", Some(("/BOOK[@ISBN = '1']/SECTION/TITLE = 'x'", true, true, vec![]))),
        ("/BOOK[.//TITLE]/SECTION/@id", Some(("/BOOK[.//TITLE]/SECTION/@id", true, true, vec![]))),
        ("//item[price > 5]", Some(("//item/price > 5", false, false, vec![0]))),
        ("//item[price eq '5']", Some(("//item/price", false, false, vec![0]))),
        ("/BOOK[@ISBN = 1]/SECTION", Some(("/BOOK[@ISBN = 1]/SECTION", false, false, vec![1]))),
        ("/BOOK[@ISBN eq '1']/SECTION", Some(("/BOOK[@ISBN]/SECTION", false, false, vec![1]))),
        ("/BOOK[@a]//SECTION", Some(("/BOOK[@a]//SECTION", true, true, vec![]))),
        ("/BOOK[@a]/CHAPTER[@b]/SECTION", Some(("/BOOK[@a]/CHAPTER[@b]/SECTION", true, true, vec![]))),
        ("//BOOK[@a]//SECTION", Some(("//BOOK[@a]//SECTION", true, false, vec![]))),
        ("//item[name = 'pen'][qty]", Some(("//item[qty]/name = 'pen'", true, true, vec![]))),
        ("//item[name = 'pen' and qty and @id = '7']", Some(("//item[qty][@id = '7']/name = 'pen'", true, true, vec![]))),
        ("//item[. = 'x'][qty]", Some(("//item[qty] = 'x'", true, true, vec![]))),
        ("//item[.//name][qty]", Some(("//item[.//name]/qty", true, true, vec![]))),
        ("//order[item[@n > 1]]", None),
        ("//order[@id = '1' and item/@n > 1]/x", Some(("//order[@id = '1'][item/@n > 1]/x", false, false, vec![2]))),
        ("/BOOK[. = 'x']/SECTION", None),
        ("//item[a or b]", None),
    ] {
        let wanted = expected.map(|(q, f, e, r)| (q.to_owned(), f, e, r));
        assert_eq!(exactly(text), wanted, "{text}");
    }
    let names = Some(("//item/name".to_owned(), true, false, vec![]));
    assert_eq!(exactly(&format!("{unicode} //item[name = 'pen']")), names);
    assert_eq!(exactly(&format!("{unicode} /B[@x = 'y']/S")), Some(("/B[@x]/S".to_owned(), true, false, vec![])));
}

/// Verifies: [EXTENSION] [XQuery §3.2.2] [Pal et al. §3.3] RULES.md D106:
/// a query's lookups are its own and each filter's, its steps to its
/// anchor then the filter's, with the filter's comparisons.
#[test]
fn a_querys_lookups_are_its_own_and_its_filters() {
    let query = compile_with("/BOOK[@ISBN = '1']/CHAPTER[.//TITLE]/SECTION", &[]).unwrap();
    let lookups: Vec<String> = query.reduced().unwrap().query.lookups().iter().map(ToString::to_string).collect();
    assert_eq!(lookups, ["/BOOK/CHAPTER/SECTION", "/BOOK/@ISBN = '1'", "/BOOK/CHAPTER//TITLE"]);
}

/// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §8.4 GR2] RULES.md
/// D106: a query of any other shape reduces to none: a comparison, whose
/// value is never empty; a step of another axis or a kind test; a
/// positional predicate, or one of `or`; a wildcard of a part of a name;
/// a prolog's variable or function; `/` alone, `.`, or a path ending at
/// `//`.
#[test]
fn other_queries_reduce_to_none() {
    for text in [
        "//item = 'pen'",
        "//item/..",
        "//item/text()",
        "//item/following-sibling::x",
        "//item[1]",
        "//p:*",
        "declare namespace p = 'urn:p'; //p:*",
        "//*:item",
        "declare variable $x := 1; //item",
        "declare function local:f() { 1 }; //item",
        "/",
        ".",
        "(//item)[1]",
        "//@id/x",
        "//@id[x]",
        "//item[/a]",
        "//item[. is .]",
        "//item[name = name]",
        "//self::node()[a]",
        "//item/(name)",
        "/order/descendant-or-self::node()",
        "//item[count(a) = 1]",
        "//item[. is 1]",
        "//item[a or b]",
        "//item[a[b]]",
    ] {
        let reduced = compile_with(text, &[]).ok().and_then(|q| q.reduced()).map(|r| r.query.to_string());
        assert_eq!(reduced, None, "{text}");
    }
}
