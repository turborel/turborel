//! Tests of the functions a plug-in registers [XQuery §3.1.5, §4.15]: the
//! dynamic context a call reads, the conversion of its arguments, its own
//! errors; and of a plan as its evaluation reads it.

use std::sync::Arc;

use super::super::super::plan::{CallOp, Callee, Op};
use super::super::super::testing::{compile_in, evaluate, shown};
use super::super::super::{Context, ErrorCode, ItemKind, Library, Occurrence, SeqType, error};
use crate::error::{Code, Result};
use crate::xml::{Atomic, AtomicType, Item, QName, Sequence};

/// The namespace of the tests' functions.
const TEST_NAMESPACE: &str = "urn:test";

/// What a test function does when called.
#[derive(Clone, Copy)]
enum Does {
    /// Gives a string of what its dynamic context holds.
    Probe,
    /// Gives its argument, of type `numeric*`.
    Same,
    /// Fails with an error code of its own.
    Fail,
}

/// A function of the tests, in `TEST_NAMESPACE`.
struct Test {
    /// Its name.
    name: QName,
    /// Its parameters.
    parameters: Vec<SeqType>,
    /// What it does.
    does: Does,
}

/// `item()*`.
static ANY: SeqType = SeqType::Of(ItemKind::Item, Occurrence::Any);

impl Test {
    /// The function `local` of `TEST_NAMESPACE` doing `does`.
    fn new(local: &str, does: Does) -> Arc<Test> {
        let parameters = match does {
            Does::Probe | Does::Fail => Vec::new(),
            Does::Same => vec![SeqType::Of(ItemKind::Numeric, Occurrence::Any)],
        };
        Arc::new(Test { name: QName { prefix: None, uri: Some(TEST_NAMESPACE.into()), local: local.into() }, parameters, does })
    }
}

/// The string of what `c` holds: the context position and size, the
/// implicit timezone, whether the current dateTime is the tests', the
/// default collation, whether the codepoint collation is known by its URI,
/// the base URI, and whether the statement goes on.
fn probe(c: &Context<'_>) -> Result<String> {
    c.check()?;
    let known = c.collation_named(super::super::super::CODEPOINT).is_some();
    let now = c.now() == super::super::super::testing::now();
    let item = c.item().is_some();
    let (position, size, zone) = (c.position(), c.size(), c.timezone());
    Ok(format!("{item}/{position}/{size}/{zone}/{now}/{}/{known}/{}", c.collation().uri(), c.base_uri().unwrap_or("none")))
}

impl super::super::super::XQueryFunction for Test {
    fn name(&self) -> &QName {
        &self.name
    }
    fn parameters(&self) -> &[SeqType] {
        &self.parameters
    }
    fn result(&self) -> &SeqType {
        &ANY
    }
    fn call(&self, c: &Context<'_>, arguments: Vec<Sequence>) -> Result<Sequence> {
        match self.does {
            Does::Probe => Ok(Sequence::new(vec![Item::Atomic(Atomic { kind: AtomicType::String, lexical: probe(c)?.into() })])),
            Does::Same => Ok(arguments.into_iter().next().unwrap_or_default()),
            Does::Fail => Err(error(ErrorCode::new("TEST0001"), "the test function fails")),
        }
    }
}

/// The built-in functions and the tests'.
fn library() -> Library {
    let mut library = Library::default();
    for (local, does) in [("probe", Does::Probe), ("same", Does::Same), ("fail", Does::Fail)] {
        library.add(Test::new(local, does)).unwrap();
    }
    library
}

/// The value of `text` of the tests' library, serialized, or its error.
fn outcome(text: &str) -> std::result::Result<String, String> {
    let query = compile_in(text, &[], &library()).map_err(|e| e.msg)?;
    evaluate(&query, None, Vec::new()).map(|v| shown(&v)).map_err(|e| e.msg)
}

/// The prolog naming the tests' namespace.
const PROLOG: &str = "declare namespace t = \"urn:test\";";

/// Verifies: [EXTENSION] [XQuery §2.1.2, §4.4, §4.5, §4.7, §4.8] [DESIGN]
/// RULES.md D44, D86: a plug-in's function reads its dynamic context: the
/// focus of its call, the implicit timezone and current dateTime of the
/// evaluation, the default collation and base URI the prolog declares,
/// the collations known; ordering and empty order settings are taken.
#[test]
fn a_function_reads_its_dynamic_context() {
    let setters = "declare base-uri \"urn:base\"; declare ordering unordered; declare default order empty greatest;";
    let codepoint = super::super::super::CODEPOINT;
    let probe = |text: &str| outcome(&format!("{PROLOG} {setters} {text}")).unwrap();
    let at = |n: usize| format!("true/{n}/2/0/true/{codepoint}/true/urn:base");
    assert_eq!(probe("(<a/>, <b/>)/t:probe()"), format!("{} {}", at(1), at(2)));
    assert_eq!(outcome(&format!("{PROLOG} t:probe()")).unwrap(), format!("false/0/0/0/true/{codepoint}/true/none"));
    let ordered = probe("for $x in (2, 1, 3) order by (if ($x = 1) then () else $x) return $x");
    assert_eq!(ordered, "2 3 1", "the prolog's empty greatest");
}

/// Verifies: [EXTENSION] [XQuery §3.1.5] [XQueryFO §1.4] a parameter of the
/// type `numeric` an argument converts to: an untyped value cast to
/// xs:double, a number kept as it is, another value err:XPTY0004; a call
/// of no function of its arity err:XPST0017; a function's own error code
/// raised as it gives it.
#[test]
fn a_function_converts_its_arguments() {
    let same = |text: &str| outcome(&format!("{PROLOG} {text}"));
    assert_eq!(same("t:same(<a>2</a>) instance of xs:double, t:same(2) instance of xs:integer").unwrap(), "true true");
    assert!(same("t:same('a')").unwrap_err().contains("err:XPTY0004"));
    assert!(same("t:same()").unwrap_err().contains("err:XPST0017"));
    assert!(same("t:fail()").unwrap_err().contains("err:TEST0001"));
}

/// Verifies: [DESIGN] RULES.md R-SAFE-3, D86: a compiled query shows its
/// plan, its declared, external and library functions named; a plan that
/// names a variable or function it does not have fails with SQLCODE -910,
/// not a panic.
#[test]
fn plans_show_and_fail_safely() {
    let text = format!("{PROLOG} declare function local:f() {{ 1 }}; declare function t:fail() external; local:f(), t:probe(), t:fail()");
    let mut query = compile_in(&text, &[], &library()).unwrap();
    let shown = format!("{query:?}");
    for part in ["Plan", "Declared(", "Library(", "External(", "Body("] {
        assert!(shown.contains(part), "{part} in {shown}");
    }
    query.0.body.op = Op::Global(7);
    assert_eq!(evaluate(&query, None, Vec::new()).unwrap_err().code, Code::Api);
    query.0.body.op = Op::Call(Box::new(CallOp { callee: Callee::Declared(7), arguments: Vec::new() }));
    assert_eq!(evaluate(&query, None, Vec::new()).unwrap_err().code, Code::Api);
}
