//! Tests of writing a tree back as text into a writer that fails: each
//! failure of the writer ends the text, and what was written before it
//! is the start of the whole text.

use std::fmt::{self, Write};

use crate::xml::Version;
use crate::xquery::parse;

/// A writer that refuses its `fail_at`th write, counting from zero, and
/// every write after it.
struct Failing {
    /// The text written before the refusal.
    out: String,
    /// The writes taken so far.
    writes: usize,
    /// The index of the first write refused.
    fail_at: usize,
}

impl Write for Failing {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let index = self.writes;
        self.writes = self.writes.saturating_add(1);
        if index >= self.fail_at {
            return Err(fmt::Error);
        }
        self.out.push_str(s);
        Ok(())
    }
}

/// Writes the module `text` into a writer failing at each write in turn:
/// each fails, and leaves the start of the whole text written.
fn fails_at_each_write(text: &str) {
    let module = parse(text, Version::V11).unwrap_or_else(|e| panic!("{text}: {e}"));
    let whole = module.to_string();
    let mut fail_at = 0;
    loop {
        let mut writer = Failing { out: String::new(), writes: 0, fail_at };
        let result = write!(writer, "{module}");
        if result.is_ok() {
            assert_eq!(writer.out, whole, "{text}");
            break;
        }
        assert!(whole.starts_with(&writer.out), "{text} at write {fail_at}: {}", writer.out);
        assert!(writer.out.len() < whole.len(), "{text} at write {fail_at}");
        fail_at = fail_at.saturating_add(1);
    }
    assert!(fail_at > 0, "{text}");
}

/// Queries of every expression, prolog declaration, constructor and
/// sequence type the text is written for.
const QUERIES: &[&str] = &[
    "xquery version \"1.0\" encoding \"UTF-8\"; declare boundary-space preserve; declare default collation \"c\"; \
     declare base-uri \"b\"; declare construction strip; declare ordering unordered; declare default order empty least; \
     declare copy-namespaces no-preserve, no-inherit; declare namespace p = \"u\"; declare default element namespace \"e\"; \
     declare default function namespace \"f\"; import schema namespace s = \"s\" at \"h1\", \"h2\"; \
     import module namespace m = \"m\" at \"x\"; declare variable $v as xs:integer := 1; declare variable $w external; \
     declare function p:g($a as item()*, $b) as node()? { $a }; declare function p:h() external; declare option p:o \"x\"; 1",
    "xquery version \"1.0\"; declare boundary-space strip; declare construction preserve; declare ordering ordered; \
     declare default order empty greatest; declare copy-namespaces preserve, inherit; import schema default element namespace \"d\"; \
     import schema \"t\"; import module \"n\"; 2",
    "module namespace m = \"urn:m\"; declare variable $m:a := 1;",
    "1 or 2 and 3, 1 = 2, 1 != 2, 1 < 2, 1 <= 2, 1 > 2, 1 >= 2, 1 eq 2, 1 ne 2, 1 lt 2, 1 le 2, 1 gt 2, 1 ge 2, \
     $a is $b, $a << $b, $a >> $b, 1 to 2, 1 + 2 - 3, 1 * 2 div 3 idiv 4 mod 5, $a | $b union $c, $a intersect $b except $c, \
     $a instance of xs:int+, $a treat as item()?, $a castable as xs:int?, $a cast as xs:int, -+1, \"q\"\"&amp;\", 1.5, 1e0",
    "if (1) then 2 else 3, some $x in 1, $y in 2 satisfies $x, every $x as xs:int in 1 satisfies $x, \
     typeswitch (1) case $c as xs:int return 1 case xs:string return 2 default $d return 3, typeswitch (1) case node() return 4 default return 5, \
     ordered { 1 }, unordered { 2 }, validate { 1 }, validate lax { 1 }, validate strict { 1 }, (#p:a b#) (#c#) { 1 }, (), \
     f(1, 2), $a[1][2], (1)[1], .",
    "for $x as xs:int at $i in 1, $y in 2 let $z as item() := 3, $w := 4 where $x stable order by $x ascending empty greatest \
     collation \"c\", $y descending empty least return $x, for $a in 1 order by $a return $a",
    "/, /a, //b, a/b//c, child::a, descendant::a, attribute::a, self::a, descendant-or-self::a, following-sibling::a, \
     following::a, parent::a, ancestor::a, preceding-sibling::a, preceding::a, ancestor-or-self::a, @b, .., a[1], p:*, *:b, *, \
     text(), comment(), node(), processing-instruction(), processing-instruction(t), document-node(), document-node(element(a)), \
     element(), element(*), element(a, t?), attribute(), attribute(*, t), schema-element(a), schema-attribute(a), \
     $a instance of empty-sequence()",
    "<a b=\"x{1}&amp;&#x9;\"\"{{\" c='y'>t{2}&lt;<![CDATA[d]]><e/><!--k--><?p q?><?r?>}}</a>, <!--c-->, <?p?>, <?p x?>, \
     document { 1 }, element a { 1 }, element { $n } { }, attribute b { 2 }, attribute { $n } { }, text { 1 }, comment { 1 }, \
     processing-instruction p { 1 }, processing-instruction { $n } { }",
];

/// Verifies: [DESIGN] a query written back as text into a writer that
/// fails stops at the failure, at every write of every expression,
/// prolog declaration, constructor and sequence type, and passes the
/// failure on: what was written is the start of the whole text, never
/// more.
#[test]
fn writer_failures_end_the_text() {
    for text in QUERIES {
        fails_at_each_write(text);
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] [XQuery Appendix A.1
/// \[38\]] an `order by` that is not `stable` is written without it, and
/// parses back to the same tree.
#[test]
fn order_by_without_stable_is_written_so() {
    let module = parse("for $a in 1 order by $a return $a", Version::V11).unwrap();
    let text = module.to_string();
    assert!(text.contains("order by $a"), "{text}");
    assert!(!text.contains("stable"), "{text}");
    assert_eq!(parse(&text, Version::V11).unwrap(), module);
}
