//! Tests of the library of functions: the contract of `XQueryFunction`
//! each built-in keeps, and registration.

use std::sync::Arc;

use super::{FN_NAMESPACE, Library, SeqType, XQueryFunction};
use crate::error::Code;
use crate::xml::QName;

/// Verifies: [EXTENSION] [XQuery §3.1.5] [DESIGN] RULES.md R-SAFE-3, D86:
/// every built-in function keeps the contract of `XQueryFunction`: its
/// name in the `fn` namespace, its name and arity alone in the library,
/// found by them.
#[test]
fn every_builtin_keeps_the_contract() {
    let library = Library::builtin();
    let all = super::builtin::all();
    assert_eq!(all.len(), 150, "a function of each arity of each of XQueryFO's");
    for f in &all {
        assert_eq!(f.name().uri.as_deref(), Some(FN_NAMESPACE), "{:?}", f.name());
        let found = library.find(f.name(), f.parameters().len()).unwrap();
        assert_eq!((found.name(), found.parameters().len(), found.variadic()), (f.name(), f.parameters().len(), f.variadic()));
        assert_eq!(f.result(), &SeqType::ANY);
    }
    let concat = QName { prefix: None, uri: Some(FN_NAMESPACE.into()), local: "concat".into() };
    assert!(library.find(&concat, 5).is_some(), "fn:concat takes two arguments or more");
    assert!(library.find(&concat, 1).is_none());
}

/// Verifies: [EXTENSION] [DESIGN] RULES.md D53, D86: a function of a name
/// and an arity the library has is refused, built in or registered, and
/// so is one in no namespace, each SQLCODE -910; none is replaced.
#[test]
fn registration_refuses_clashes() {
    let mut library = Library::builtin();
    let count = super::builtin::all().into_iter().find(|f| &*f.name().local == "count").unwrap();
    assert_eq!(library.add(Arc::clone(&count)).unwrap_err().code, Code::Api);
    assert_eq!(library.add(Arc::new(Unnamed(QName::local("f")))).unwrap_err().code, Code::Api);
    let named = QName { prefix: None, uri: Some("urn:x".into()), local: "f".into() };
    library.add(Arc::new(Unnamed(named.clone()))).unwrap();
    assert!(library.find(&named, 0).is_some());
    assert_eq!(library.configurables().count(), 0);
    let query = super::super::testing::compile_in("declare namespace x = 'urn:x'; count(x:f())", &[], &library).unwrap();
    assert_eq!(super::super::testing::shown(&super::super::testing::evaluate(&query, None, Vec::new()).unwrap()), "0");
}

/// A function of the name it is given and no parameter.
struct Unnamed(QName);

impl XQueryFunction for Unnamed {
    fn name(&self) -> &QName {
        &self.0
    }
    fn parameters(&self) -> &[SeqType] {
        &[]
    }
    fn result(&self) -> &SeqType {
        &SeqType::Empty
    }
    fn call(&self, _: &super::Context<'_>, _: Vec<crate::xml::Sequence>) -> crate::error::Result<crate::xml::Sequence> {
        Ok(crate::xml::Sequence::default())
    }
}
