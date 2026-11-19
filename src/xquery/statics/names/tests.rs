//! Tests of the names of element and attribute types narrowed by a
//! wildcard's namespace constraint.

use super::super::super::plan::Namespace;
use super::super::types::Name;
use super::{Namespaces, name_within, names_both, narrowed};

/// The namespace `uri`.
fn uri(uri: &str) -> Namespace {
    Namespace::Uri(uri.into())
}

/// `{urn:a|urn:b}*`, as `namespace="urn:b urn:a urn:a"` allows.
fn among() -> Name {
    narrowed(Namespaces::Among(vec![uri("urn:b"), uri("urn:a"), uri("urn:a")]), None, Vec::new()).unwrap()
}

/// `##other` of target namespace `urn:t`: neither it nor none.
fn other() -> Name {
    narrowed(Namespaces::NotAmong(vec![uri("urn:t"), Namespace::Absent]), None, Vec::new()).unwrap()
}

/// Verifies: [EXTENSION] RULES.md D106 [Schema1 §3.10.4 Wildcard allows
/// Namespace Name] [XQueryFS §8.3.2]: a name narrowed to a set of
/// namespaces, or to all but some, matches the QNames of those alone; one
/// name is within another as each name it matches is.
#[test]
fn wildcard_names_admit_their_namespaces_alone() {
    let (among, other) = (among(), other());
    assert_eq!(among.to_string(), "{urn:a|urn:b}*");
    assert_eq!(other.to_string(), "{^|urn:t}*");
    assert!(name_within(&Name::new(Some("urn:a"), "x"), &among));
    assert!(!name_within(&Name::new(Some("urn:c"), "x"), &among));
    assert!(!name_within(&Name::new(None, "x"), &among));
    assert!(name_within(&Name::new(Some("urn:u"), "x"), &other));
    assert!(!name_within(&Name::new(Some("urn:t"), "x"), &other), "not its namespace [2.2]");
    assert!(!name_within(&Name::new(None, "x"), &other), "nor none [2.3]");
    assert!(name_within(&among, &other) && !name_within(&other, &among));
    assert!(name_within(&among, &Name::ANY) && !name_within(&Name::ANY, &among));
}

/// Verifies: [EXTENSION] RULES.md D106 [Schema1 §3.10.4 Wildcard allows
/// Namespace Name] [XQueryFS §8.2.3.1.1]: a name test of a narrowed name
/// keeps the names both match, none if none; of two all but some, all but
/// those of either.
#[test]
fn name_tests_of_wildcard_names_keep_what_both_match() {
    let (among, other) = (among(), other());
    let all_but_none = narrowed(Namespaces::NotAmong(vec![Namespace::Absent]), None, Vec::new()).unwrap();
    assert!(name_within(&other, &all_but_none) && !name_within(&all_but_none, &other));
    assert_eq!(names_both(&other, &Name::tested(&uri("urn:t"), None)), None);
    assert_eq!(names_both(&other, &Name::tested(&uri("urn:u"), None)), Some(Name::tested(&uri("urn:u"), None)));
    let local = names_both(&among, &Name::tested(&Namespace::Any, Some("x"))).unwrap();
    assert_eq!(local.to_string(), "{urn:a|urn:b}x");
    assert_eq!(names_both(&among, &Name::tested(&uri("urn:b"), Some("x"))), Some(Name::new(Some("urn:b"), "x")));
    let neither = narrowed(Namespaces::NotAmong(vec![uri("urn:u")]), None, Vec::new()).unwrap();
    assert_eq!(names_both(&other, &neither).unwrap().to_string(), "{^|urn:t|urn:u}*");
    assert_eq!(names_both(&among, &Name::tested(&Namespace::Absent, None)), None);
    assert_eq!(narrowed(Namespaces::Among(Vec::new()), None, Vec::new()), None, "of an empty set, none");
    assert_eq!(narrowed(Namespaces::NotAmong(Vec::new()), None, Vec::new()), Some(Name::ANY));
}

/// Verifies: [EXTENSION] RULES.md D106 [Schema1 §3.4.4 Element Locally
/// Valid (Complex Type) 3.1, 3.2]: an attribute wildcard's name does not
/// match the names of the attribute uses beside it; a name test of one of
/// those finds none of the wildcard's, of another its own.
#[test]
fn attribute_wildcards_match_no_declared_name() {
    let code = (Namespace::Absent, Box::<str>::from("code"));
    let unrelated = (uri("urn:z"), Box::<str>::from("code"));
    let wild = narrowed(Namespaces::NotAmong(Vec::new()), None, vec![code.clone(), code.clone(), unrelated]).unwrap();
    assert_eq!(wild.to_string(), "* except {}code except {urn:z}code");
    assert!(!name_within(&Name::new(None, "code"), &wild));
    assert!(name_within(&Name::new(None, "note"), &wild));
    assert!(name_within(&wild, &Name::ANY) && !name_within(&Name::ANY, &wild));
    assert_eq!(names_both(&wild, &Name::new(None, "code")), None);
    assert_eq!(names_both(&wild, &Name::new(None, "note")), Some(Name::new(None, "note")));
    let local = narrowed(Namespaces::Among(vec![Namespace::Absent]), None, vec![code]).unwrap();
    assert_eq!(local.to_string(), "* except {}code");
    assert!(name_within(&local, &wild), "each name it matches the other does");
    assert!(!name_within(&wild, &local));
    let some = names_both(&wild, &Name::tested(&Namespace::Any, Some("code"))).unwrap();
    assert_eq!(some.to_string(), "*:code except {}code except {urn:z}code");
    assert!(name_within(&Name::new(Some("urn:y"), "code"), &some) && !name_within(&Name::new(None, "code"), &some));
}
