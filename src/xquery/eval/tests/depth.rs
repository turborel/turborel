//! Tests of the bound on how deep an evaluation nests (`MAX_DEPTH`).

use super::super::super::testing::{compile_with, evaluate};
use crate::error::{Code, Result};

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
