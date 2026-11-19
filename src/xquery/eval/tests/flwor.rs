//! Tests of the expressions that bind variables [XQuery §3.8, §3.11,
//! §3.12.2], and of the prolog's variables and functions [XQuery §4.14,
//! §4.15].

use super::super::super::testing::{code_of, run};

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.8.1,
/// §3.8.2] for and let clauses make a tuple for each combination, the
/// leftmost the outermost, a positional variable its item's position; a
/// where clause keeps the tuples it is true of; a declared type not
/// matched err:XPTY0004.
#[test]
fn for_let_where() {
    assert_eq!(run("for $i in (1, 2), $j in (3, 4) return $i * 10 + $j"), "13 14 23 24");
    assert_eq!(run("for $x at $p in ('a', 'b') return concat($p, $x)"), "1a 2b");
    assert_eq!(run("let $s := (1, 2, 3) return count($s)"), "3");
    assert_eq!(run("for $i in 1 to 10 let $j := $i * $i where $j mod 2 = 0 return $j"), "4 16 36 64 100");
    assert_eq!(run("for $i in (1, 2) for $i in ($i * 10) return $i"), "10 20");
    assert_eq!(run("for $i as xs:integer in (1, 2) return $i"), "1 2");
    assert_eq!(code_of("let $s as xs:decimal := 'cat' return $s"), "err:XPTY0004");
    assert_eq!(code_of("for $i in 1 return $j"), "err:XPST0008");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.8.3] order
/// by sorts the tuples by each spec in turn, ascending or descending,
/// stably, the empty sequence least by default and greatest if asked, an
/// untyped key a string; keys of types that do not compare err:XPTY0004.
#[test]
fn order_by() {
    assert_eq!(run("for $x in (3, 1, 2) order by $x return $x"), "1 2 3");
    assert_eq!(run("for $x in (3, 1, 2) order by $x descending return $x"), "3 2 1");
    assert_eq!(run("for $p in (<p/>, <p>2</p>, <p>1</p>) order by $p/text() return string($p)"), " 1 2");
    let keyed = "for $p in (<p k='b' n='1'/>, <p n='2'/>, <p k='a' n='3'/>, <p k='b' n='4'/>)";
    assert_eq!(run(&format!("{keyed} stable order by $p/@k return string($p/@n)")), "2 3 1 4");
    assert_eq!(run(&format!("{keyed} order by $p/@k empty greatest return string($p/@n)")), "3 1 4 2");
    assert_eq!(run(&format!("{keyed} order by $p/@k descending empty least, $p/@n descending return string($p/@n)")), "4 1 3 2");
    assert_eq!(run("for $x in (xs:double('NaN'), 1e0, ()) order by $x empty greatest return string($x)"), "1 NaN");
    assert_eq!(code_of("for $x in (1, 'a') order by $x return $x"), "err:XPTY0004");
    assert_eq!(code_of("for $x in 1 order by (1, 2) return $x"), "err:XPTY0004");
    assert_eq!(code_of("for $x in 1 order by $x collation 'urn:no:such' return $x"), "err:XQST0076");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §4.4, §3.8.3]
/// \[EXTENSION\] a collation of the database is reachable by its URI,
/// `urn:turborel:collation:` and its name, as the default collation and
/// in order by; the codepoint collation by its URI.
#[test]
fn collations() {
    assert_eq!(run("for $x in ('b', 'B', 'a') order by $x return $x"), "B a b");
    let unicode = "for $x in ('b', 'B', 'a') order by $x collation 'urn:turborel:collation:UNICODE' return $x";
    assert_eq!(run(unicode), "a b B");
    assert_eq!(run("declare default collation 'http://www.w3.org/2005/xpath-functions/collation/codepoint'; 'a' lt 'B'"), "false");
    assert_eq!(code_of("declare default collation 'urn:no:such'; 1"), "err:XQST0038");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.11] some is
/// true if the test is true of a tuple, every if of each; of no tuple
/// false and true.
#[test]
fn quantified() {
    assert_eq!(run("some $x in (1, 2, 3), $y in (2, 3, 4) satisfies $x + $y = 4"), "true");
    assert_eq!(run("every $x in (1, 2, 3), $y in (2, 3, 4) satisfies $x + $y = 4"), "false");
    assert_eq!(run("some $x in () satisfies true(), every $x in () satisfies false()"), "false true");
    assert_eq!(code_of("some $x as xs:string in (1) satisfies true()"), "err:XPTY0004");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §3.12.2] a
/// typeswitch evaluates the first case its operand matches, its
/// variable bound to the operand, else the default.
#[test]
fn typeswitch() {
    let q =
        |v: &str| format!("typeswitch ({v}) case $i as xs:integer return $i + 1 case element(a) return 'a' default $d return count($d)");
    assert_eq!(run(&q("41")), "42");
    assert_eq!(run(&q("<a/>")), "a");
    assert_eq!(run(&q("('x', 'y')")), "2");
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18 GR2] [XQuery §4.14, §4.15]
/// the prolog's variables, each evaluated once as it is first read, and
/// its functions, recursive, their arguments and results converted to the
/// declared types; a declared function in no namespace err:XQST0060, in a
/// reserved one err:XQST0045, twice err:XQST0034; a variable declared
/// twice err:XQST0049; an initializing expression depending on its
/// variable err:XQST0054; an external variable passed no value
/// err:XPDY0002.
#[test]
fn prolog_variables_and_functions() {
    let fact = "declare function local:f($n as xs:integer) as xs:integer { if ($n le 1) then 1 else $n * local:f($n - 1) };";
    assert_eq!(run(&format!("{fact} local:f(20)")), "2432902008176640000");
    assert_eq!(run("declare variable $x := 2; declare variable $y as xs:integer := $x * 3; $y"), "6");
    assert_eq!(run("declare function local:g($s as xs:double) { $s }; local:g(1) instance of xs:double"), "true");
    assert_eq!(run("declare function local:h($e) { count($e) }; local:h(<a>1</a>/text())"), "1");
    assert_eq!(code_of("declare function local:k() as xs:integer { 'x' }; local:k()"), "err:XPTY0004");
    assert_eq!(code_of("declare function f() { 1 }; 1"), "err:XQST0045");
    assert_eq!(code_of("declare default function namespace ''; declare function f() { 1 }; 1"), "err:XQST0060");
    assert_eq!(code_of("declare function local:f() { 1 }; declare function local:f() { 2 }; 1"), "err:XQST0034");
    assert_eq!(code_of("declare variable $x := 1; declare variable $x := 2; 1"), "err:XQST0049");
    assert_eq!(code_of("declare variable $x := local:f(); declare function local:f() { $x }; $x"), "err:XQST0054");
    assert_eq!(code_of("declare variable $x external; $x"), "err:XPDY0002");
    assert_eq!(code_of("declare variable $x := $y; declare variable $y := 1; $x"), "err:XPST0008");
    assert_eq!(code_of("declare function local:e($x) external; 1"), "err:XPST0017");
}
