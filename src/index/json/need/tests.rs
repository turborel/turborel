//! Unit tests of what a call needs of a json index: each document a call
//! is true of posts what it needs, so a lookup misses none, and calls of
//! values ask more than their paths (RULES.md R-TEST-1, R-TEST-2).

use std::collections::BTreeSet;

use super::*;
use crate::functions::{JsonCall, JsonContains, JsonOperation, ScalarFunction};
use crate::index::json::terms::terms_of;
use crate::json::{ExistsBehavior, parse_path};
use crate::value::Decimal;
use crate::value::datetime::Clocked;

/// Whether `terms` hold what `need` asks.
fn holds(need: &Need, terms: &BTreeSet<Term>) -> bool {
    match need {
        Need::Every => true,
        Need::Term(term) => terms.contains(term),
        Need::And(parts) => parts.iter().all(|p| holds(p, terms)),
        Need::Or(parts) => parts.iter().any(|p| holds(p, terms)),
    }
}

/// JSON_EXISTS of `path`, of PASSING variables named `names`.
fn exists(path: &str, names: &[&str]) -> JsonCall {
    let passing = names.iter().map(|n| ((*n).to_owned(), false)).collect();
    JsonCall::new(path, parse_path(path, Clocked::default()).unwrap(), passing, JsonOperation::Exists(ExistsBehavior::False))
}

const SHAPES: [Shape; 3] =
    [Shape { paths_only: false, max_depth: 128 }, Shape { paths_only: true, max_depth: 128 }, Shape { paths_only: false, max_depth: 1 }];

const DOCUMENTS: [&str; 8] = [
    r#"{"a":{"b":3}}"#,
    r#"{"a":[{"b":3},{"b":"3"}]}"#,
    r#"{"a":{"b":[1,2,3]},"k":"x"}"#,
    r#"{"k":"x ","n":1.0}"#,
    r#"{"k":"y","n":1,"t":[true,null]}"#,
    r#"[{"a":{"b":4}}]"#,
    r#"{"tags":["t1","t2"],"items":[{"k":"x2","v":1}]}"#,
    "3",
];

/// Verifies: [EXTENSION] a document that JSON_EXISTS is true of posts
/// what the call needs, of every shape: chains, filters of literals and
/// variables, && and ||, wildcards, subscripts, strict and lax, methods,
/// comparisons other than equality, negation.
#[test]
fn every_document_found_posts_what_it_needs() {
    let paths: [(&str, &[&str], Vec<Value>); 12] = [
        ("lax $.a.b", &[], vec![]),
        ("lax $.a.b ? (@ == 3)", &[], vec![]),
        ("strict $.a.b ? (@ == 3)", &[], vec![]),
        ("lax $ ? (@.k == \"x\" && @.n == 1)", &[], vec![]),
        ("lax $ ? (@.k == \"y\" || @.a.b == 4)", &[], vec![]),
        ("lax $.tags ? (@ == $t)", &["t"], vec![Value::Char("t2".into())]),
        ("lax $.items ? (@.k == $k && @.v == $v)", &["k", "v"], vec![Value::Char("x2".into()), Value::Exact(Decimal::new(1, 0))]),
        ("lax $.*.b", &[], vec![]),
        ("lax $.a[0].b", &[], vec![]),
        ("lax $.n ? (@ > 0)", &[], vec![]),
        ("lax $ ? (!(@.k == \"x\"))", &[], vec![]),
        ("lax $.t ? (@ == null)", &[], vec![]),
    ];
    for (path, names, values) in paths {
        let call = exists(path, names);
        let probe = call.json_probe().unwrap();
        for shape in SHAPES {
            let need = need_of(&probe, &Value::Row(values.clone().into()), shape);
            for doc in DOCUMENTS {
                let mut args = vec![Value::Char(doc.into())];
                args.extend(values.iter().cloned());
                let truth = call.call(&args).unwrap();
                if matches!(truth, Value::Bool(true)) {
                    let terms = terms_of(&parse(doc, Keys::Any).unwrap(), shape);
                    assert!(holds(&need, &terms), "{path} of {doc}, {shape:?}");
                }
            }
        }
    }
}

/// Verifies: [EXTENSION] a document that JSON_CONTAINS is true of posts
/// what the call needs, of every shape.
#[test]
fn every_document_containing_posts_what_it_needs() {
    let probe = JsonContains.json_probe().unwrap();
    let candidates = [r#"{"a":{"b":3}}"#, r#"{"k":"x"}"#, "3", "[1,2]", "[]", r#"{"t":[null]}"#, r#"{"a":{"b":4}}"#];
    for candidate in candidates {
        for shape in SHAPES {
            let need = need_of(&probe, &Value::Row(vec![Value::Char(candidate.into())].into()), shape);
            for doc in DOCUMENTS {
                let truth = JsonContains.call(&[Value::Char(doc.into()), Value::Char(candidate.into())]).unwrap();
                if matches!(truth, Value::Bool(true)) {
                    let terms = terms_of(&parse(doc, Keys::Any).unwrap(), shape);
                    assert!(holds(&need, &terms), "{candidate} in {doc}, {shape:?}");
                }
            }
        }
    }
}

/// Verifies: [EXTENSION] a filter of a value needs its value posted, not
/// its path alone, so documents with the path and another value are not
/// found; paths alone need the path.
#[test]
fn a_filter_of_a_value_needs_the_value() {
    let call = exists("lax $.a.b ? (@ == 3)", &[]);
    let probe = call.json_probe().unwrap();
    let whole = SHAPES[0];
    let need = need_of(&probe, &Value::Row(Vec::new().into()), whole);
    let other = terms_of(&parse(r#"{"a":{"b":5}}"#, Keys::Any).unwrap(), whole);
    assert!(!holds(&need, &other));
    let paths = need_of(&probe, &Value::Row(Vec::new().into()), SHAPES[1]);
    assert!(holds(&paths, &terms_of(&parse(r#"{"a":{"b":5}}"#, Keys::Any).unwrap(), SHAPES[1])));
    assert_ne!(need, Need::Every);
}
