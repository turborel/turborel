//! Tests of the bounds of an evaluation: how deep it nests (`MAX_DEPTH`),
//! and how much it makes.

use super::super::super::testing::{compile_with, evaluate};
use crate::error::{Code, Result};
use crate::xml::{Atomic, AtomicType, Item, MAX_NODES, MAX_STORED, MAX_TEXT, Sequence};

/// Recursive functions of several shapes, each called with its depth: one
/// adding, one constructing an element of each level, one binding a
/// FLWOR variable of each, and one stepping down a path of each.
const SHAPES: [&str; 4] = [
    "declare function local:f($n) { if ($n eq 0) then 0 else $n + local:f($n - 1) }; local:f(N)",
    "declare function local:f($n) { if ($n eq 0) then () else <a>{local:f($n - 1)}</a> }; count(local:f(N))",
    "declare function local:f($n) { for $i in $n let $j := $i - 1 return if ($j lt 0) then 0 else local:f($j) }; local:f(N)",
    "declare function local:f($e, $n) { if ($n eq 0) then 1 else local:f($e/self::a, $n - 1) }; local:f(<a/>, N)",
];

/// Whether `shape` of depth `n` evaluates, and the code of its error.
fn depth(shape: &str, n: usize) -> Result<()> {
    let query = compile_with(&shape.replace('N', &n.to_string()), &[])?;
    evaluate(&query, None, Vec::new()).map(|_| ())
}

/// `run` on a thread of `stack` octets.
fn on_stack<T: Send + 'static>(stack: usize, run: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new().name("xquery-depth".to_owned()).stack_size(stack).spawn(run).unwrap().join().unwrap()
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] \[DESIGN\] RULES.md R-CF-6,
/// D86: a function recursing past `MAX_DEPTH` levels of operations and
/// calls is refused with SQLCODE -900, never a stack overflow; the
/// deepest evaluation of each shape accepted runs within half of a
/// thread's default stack.
#[test]
fn depth_is_bounded_within_the_stack() {
    const BIG_STACK: usize = 64 * 1024 * 1024;
    const HALF_DEFAULT_STACK: usize = 1024 * 1024;
    for shape in SHAPES {
        let first_refused = on_stack(BIG_STACK, move || (1..=super::super::MAX_DEPTH).find(|&n| depth(shape, n).is_err()).unwrap());
        let refused = on_stack(BIG_STACK, move || depth(shape, first_refused).unwrap_err());
        assert_eq!(refused.code, Code::Unsupported, "{shape}: {refused}");
        assert!(on_stack(HALF_DEFAULT_STACK, move || depth(shape, first_refused - 1).is_ok()), "{shape}");
    }
}

/// Verifies: [ISO/IEC 9075-14:2006 draft §6.18] \[DESIGN\] RULES.md R-CF-6,
/// D86: an evaluation making a sequence of more than `xml::MAX_NODES`
/// items, a string longer than `xml::MAX_TEXT`, or nodes holding more than
/// `xml::MAX_STORED` octets is refused with SQLCODE -900.
#[test]
fn sizes_are_bounded() {
    let past = |text: &str, variables: Vec<Sequence>| evaluate(&compile_with(text, &["s"]).unwrap(), None, variables).unwrap_err().code;
    let nodes = MAX_NODES + 1;
    assert_eq!(past(&format!("count(1 to {nodes})"), vec![Sequence::default()]), Code::Unsupported);
    let half = "x".repeat(MAX_TEXT.div_euclid(2) + 1);
    let s = || vec![Sequence::new(vec![Item::Atomic(Atomic::new(AtomicType::String, half.as_str()))])];
    assert_eq!(past("concat($s, $s)", s()), Code::Unsupported);
    let trees = MAX_STORED.div_euclid(half.len()) + 1;
    assert_eq!(past(&format!("for $i in 1 to {trees} return <a>{{$s}}</a>"), s()), Code::Unsupported);
}
