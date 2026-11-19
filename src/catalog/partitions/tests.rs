//! Unit tests of the partitioning descriptors' walks.

use super::*;

fn name(ident: &str) -> QualifiedName {
    QualifiedName { auth: "S".to_owned(), ident: ident.to_owned() }
}

fn key() -> PartitionKey {
    PartitionKey { strategy: "LIST".to_owned(), columns: vec![0], expressions: Vec::new(), templates: Vec::new() }
}

fn of(parent: &str, id: u64) -> PartitionOf {
    PartitionOf { parent: name(parent), bound: PartitionBound::Default, id }
}

/// A root T of partitions A (itself of A1 and A2) and B.
fn tree() -> Partitioning {
    let mut p = Partitioning::default();
    p.put(PartitionDesc { table: name("T"), key: Some(key()), of: None });
    p.put(PartitionDesc { table: name("B"), key: None, of: Some(of("T", 2)) });
    p.put(PartitionDesc { table: name("A"), key: Some(key()), of: Some(of("T", 1)) });
    p.put(PartitionDesc { table: name("A2"), key: None, of: Some(of("A", 4)) });
    p.put(PartitionDesc { table: name("A1"), key: None, of: Some(of("A", 3)) });
    p
}

/// Verifies: [EXTENSION] (RULES.md D153) a tree's walks: children by id,
/// descendants each before its own, leaves, ancestors nearest first, and
/// the root.
#[test]
fn walks_follow_the_tree() {
    let p = tree();
    let names = |v: Vec<QualifiedName>| v.into_iter().map(|n| n.ident).collect::<Vec<_>>();
    assert_eq!(p.children(&name("T")).iter().map(|d| d.table.ident.as_str()).collect::<Vec<_>>(), ["A", "B"]);
    assert_eq!(names(p.descendants(&name("T"))), ["A", "A1", "A2", "B"]);
    assert_eq!(names(p.leaves(&name("T"))), ["A1", "A2", "B"]);
    assert_eq!(names(p.leaves(&name("B"))), ["B"]);
    assert_eq!(names(p.ancestors(&name("A2"))), ["A", "T"]);
    assert_eq!(p.root(&name("A2")).ident, "T");
    assert_eq!(p.root(&name("T")).ident, "T");
    assert_eq!(p.by_id(2).map(|d| d.table.ident.as_str()), Some("B"));
}

/// Verifies: [EXTENSION] (RULES.md D153) a descriptor of neither a key nor
/// a parent is forgotten as it is put.
#[test]
fn an_empty_descriptor_is_forgotten() {
    let mut p = tree();
    p.put(PartitionDesc { table: name("B"), key: None, of: None });
    assert!(p.get(&name("B")).is_none());
    assert!(!p.is_empty());
    assert_eq!((p.take_id(), p.take_id(), p.next_id()), (0, 1, 2));
}

/// Verifies: [EXTENSION] (RULES.md D158) a derived index's name gives back
/// its template's, and a user's name none.
#[test]
fn derived_names_give_their_template() {
    let derived = derived_index_name("SALES_ID", &name("SALES_2026"));
    assert_eq!(template_of(&derived), Some("SALES_ID"));
    assert_eq!(template_of("SALES_ID"), None);
}
