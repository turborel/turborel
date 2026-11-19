//! Tests of an XML index's dictionary.

use super::*;
use crate::index::xml::keys::{ELEMENT, ordinal_of};
use crate::index::xml::tests::tree;

/// Verifies: [DESIGN] RULES.md D106 [Pal et al. §3.1.1]: a name is given a
/// token once, by its namespace URI and local name, not its prefix; one
/// not given none is none; tokens count from 1.
#[test]
fn names_are_given_tokens_once() {
    let (tree, lock) = (tree(), Mutex::new(()));
    let dictionary = Dictionary { tree: &tree, lock: &lock };
    let named =
        |uri: Option<&str>, prefix: Option<&str>| QName { prefix: prefix.map(Into::into), uri: uri.map(Into::into), local: "a".into() };
    assert_eq!(dictionary.name(&named(None, None), false).unwrap(), None, "not given");
    assert_eq!(dictionary.name(&named(None, None), true).unwrap(), Some(1));
    assert_eq!(dictionary.name(&named(Some("urn:x"), Some("p")), true).unwrap(), Some(2));
    assert_eq!(dictionary.name(&named(Some("urn:x"), Some("q")), true).unwrap(), Some(2), "another prefix, the same name");
    assert_eq!(dictionary.name(&named(None, None), false).unwrap(), Some(1));
}

/// Verifies: [DESIGN] RULES.md D106: a path is given an id once, found
/// again, and by its id.
#[test]
fn paths_are_given_ids_once() {
    let (tree, lock) = (tree(), Mutex::new(()));
    let dictionary = Dictionary { tree: &tree, lock: &lock };
    let path = |tokens: &[u64]| [vec![ELEMENT], tokens.iter().flat_map(|&t| ordinal_of(t)).collect()].concat();
    let ab = dictionary.path(&path(&[2, 1]), true).unwrap().unwrap();
    let cb = dictionary.path(&path(&[2, 3]), true).unwrap().unwrap();
    let a = dictionary.path(&path(&[1]), true).unwrap().unwrap();
    assert_eq!((ab, cb, a), (1, 2, 3));
    assert_eq!(dictionary.path(&path(&[2, 1]), true).unwrap(), Some(1), "given once");
    assert_eq!(dictionary.path_of(cb).unwrap(), Some(path(&[2, 3])));
    assert_eq!(dictionary.path_of(9).unwrap(), None);
    assert_eq!(tokens_of(&path(&[2, 300])).unwrap(), (ELEMENT, vec![2, 300]));
}

/// Verifies: [DESIGN] RULES.md D106, D18: a name whose key is longer than
/// the tree's longest is refused, SQLCODE -900.
#[test]
fn a_name_too_long_is_refused() {
    let (tree, lock) = (tree(), Mutex::new(()));
    let dictionary = Dictionary { tree: &tree, lock: &lock };
    let long = QName::local(&"n".repeat(tree.max_key()));
    assert_eq!(dictionary.name(&long, true).unwrap_err().sqlcode(), -900_i32);
}
