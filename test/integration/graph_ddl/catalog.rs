//! The `Catalog` contract of property graph descriptors (RULES.md
//! D106), for `MemoryCatalog`.

use turborel::catalog::{
    Catalog, ElementKind, ElementTableDesc, GraphDesc, GraphMode, Horizon, LabelDesc, MemoryCatalog, QualifiedName, TableDesc,
};

/// The name `ident` in schema A.
fn in_a(ident: &str) -> QualifiedName {
    QualifiedName { auth: "A".into(), ident: ident.into() }
}

/// A table `ident` of schema A, of no columns.
fn table_desc(ident: &str) -> TableDesc {
    TableDesc {
        name: in_a(ident),
        columns: Vec::new(),
        constraints: Vec::new(),
        engine: "rowstore".into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
        typed: None,
        periods: Vec::new(),
    }
}

/// A property graph named `ident` in schema A of one vertex table, `table`.
fn graph_desc(ident: &str, table: &QualifiedName) -> GraphDesc {
    let label = LabelDesc { name: table.ident.clone(), properties: Vec::new() };
    let element = ElementTableDesc {
        alias: table.ident.clone(),
        table: table.clone(),
        kind: ElementKind::Vertex,
        key: vec!["K".into()],
        labels: vec![label],
        endpoints: None,
        query: None,
    };
    GraphDesc {
        name: in_a(ident),
        elements: vec![element],
        mode: GraphMode::Trusted,
        invoker: false,
        names: "A".into(),
        reads: vec![table.clone()],
        horizon: Horizon::default(),
        valid: true,
        select: Vec::new(),
    }
}

/// Verifies: [DESIGN] `Catalog` contract, for `MemoryCatalog`, of property
/// graphs (RULES.md D106): each recorded is found by name, in
/// place of one of its name; `graphs` lists them in the order of their
/// names; one removed is found by none, the others as before; a duplicate
/// is independent; a graph is no table or view.
#[test]
fn memory_catalog_keeps_the_graph_contract() {
    let mut catalog = MemoryCatalog::default();
    catalog.create_schema("A", "A", vec![table_desc("T"), table_desc("U")]);
    let (t, u) = (in_a("T"), in_a("U"));
    for (ident, table) in [("Z", &t), ("M", &u), ("B", &t)] {
        catalog.put_graph(graph_desc(ident, table));
    }
    let idents = |all: Vec<&GraphDesc>| all.iter().map(|d| d.name.ident.clone()).collect::<Vec<_>>();
    assert_eq!(idents(catalog.graphs()), vec!["B", "M", "Z"], "by name");
    let m = in_a("M");
    assert_eq!(catalog.graph(&m).map(|g| g.reads.clone()), Some(vec![u.clone()]));
    let copy = catalog.duplicate();
    catalog.put_graph(GraphDesc { valid: false, ..graph_desc("M", &t) });
    assert_eq!(catalog.graph(&m).map(|g| (g.valid, g.reads.clone())), Some((false, vec![t.clone()])), "in place");
    assert_eq!(idents(catalog.graphs()), vec!["B", "M", "Z"], "one of each name");
    catalog.remove_graph(&m);
    assert!(catalog.graph(&m).is_none(), "removed");
    assert_eq!(idents(catalog.graphs()), vec!["B", "Z"], "the others as before");
    assert_eq!(idents(copy.graphs()), vec!["B", "M", "Z"], "the duplicate is independent");
    assert_eq!(copy.graph(&m).map(|g| g.valid), Some(true));
    assert!(catalog.table(&m).is_none() && catalog.view(&m).is_none(), "a graph is no table or view");
}
