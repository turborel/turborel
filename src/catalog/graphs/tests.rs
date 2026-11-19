//! Unit tests of a property graph descriptor's read API (RULES.md
//! R-TEST-2): what a query of a graph binds through.

use super::*;
use crate::catalog::{Actions, MemoryCatalog, PrivilegeDesc, TableDesc};
use crate::value::Spelled;

/// The name `ident` of schema A.
fn a(ident: &str) -> QualifiedName {
    QualifiedName { auth: "A".into(), ident: ident.into() }
}

/// A property `name` of expression `expression` and type INTEGER.
fn property(name: &str, expression: &str) -> PropertyDesc {
    PropertyDesc {
        name: name.into(),
        expression: expression.into(),
        data_type: DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Integer },
    }
}

/// An element table of alias `alias` over table `table`, of `labels`.
fn element(alias: &str, table: &str, kind: ElementKind, labels: Vec<LabelDesc>) -> ElementTableDesc {
    let endpoints = (kind == ElementKind::Edge).then(|| Endpoints {
        source: EndpointDesc { vertex: "P".into(), columns: vec!["FROM_K".into()], references: vec!["K".into()] },
        destination: EndpointDesc { vertex: "P".into(), columns: vec!["TO_K".into()], references: vec!["K".into()] },
        undirected: false,
    });
    ElementTableDesc { alias: alias.into(), table: a(table), kind, key: vec!["K".into()], labels, endpoints, query: None }
}

/// Graph A.G: vertex tables P and Q of label THING, P's of label PERSON
/// too, and edge table E of label KNOWS, as A's, reading P, Q and E.
fn graph() -> GraphDesc {
    let thing = |p: &str| LabelDesc { name: "THING".into(), properties: vec![property("NAME", p), property("K", "K")] };
    let person = LabelDesc { name: "PERSON".into(), properties: vec![property("NAME", "N")] };
    let knows = LabelDesc { name: "KNOWS".into(), properties: Vec::new() };
    GraphDesc {
        name: a("G"),
        elements: vec![
            element("P", "P", ElementKind::Vertex, vec![thing("N"), person]),
            element("Q", "Q", ElementKind::Vertex, vec![thing("UPPER(\"m\")")]),
            element("E", "E", ElementKind::Edge, vec![knows]),
        ],
        mode: GraphMode::Enforced,
        invoker: false,
        names: "A".into(),
        reads: vec![a("P"), a("Q"), a("E")],
        horizon: Horizon::default(),
        valid: true,
        select: Vec::new(),
    }
}

/// A catalog of schema A, owned by A, of tables P, Q and E.
fn catalog() -> MemoryCatalog {
    let table = |ident: &str| TableDesc {
        name: a(ident),
        columns: Vec::new(),
        constraints: Vec::new(),
        engine: String::new(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
        typed: None,
        periods: Vec::new(),
    };
    let mut catalog = MemoryCatalog::default();
    catalog.create_schema("A", "A", vec![table("P"), table("Q"), table("E")]);
    catalog
}

/// Verifies: [EXTENSION] §4.1 R4.4: the element tables of a graph are
/// found by alias, kind, label and table, and its labels by kind.
#[test]
fn the_read_api_finds_each_part() {
    let g = graph();
    let aliases = |all: Vec<&ElementTableDesc>| all.iter().map(|e| e.alias.clone()).collect::<Vec<_>>();
    assert_eq!(aliases(g.vertices().collect()), ["P", "Q"]);
    assert_eq!(aliases(g.edges().collect()), ["E"]);
    assert_eq!(aliases(g.labeled("THING")), ["P", "Q"]);
    assert_eq!(aliases(g.of_table(&a("Q"))), ["Q"]);
    assert!(g.element("Z").is_none() && g.element("E").is_some());
    assert_eq!(g.labels(None), ["THING", "PERSON", "KNOWS"]);
    assert_eq!(g.labels(Some(ElementKind::Edge)), ["KNOWS"]);
}

/// Verifies: [EXTENSION] §4.1 R4.4, R4.5: an element table's properties
/// of all its labels once each, a label's, and its endpoints' columns.
#[test]
fn the_read_api_finds_properties_and_endpoints() {
    let g = graph();
    let p = g.element("P").unwrap();
    assert_eq!(p.properties().iter().map(|q| q.name.as_str()).collect::<Vec<_>>(), ["NAME", "K"]);
    assert_eq!(p.property("NAME").map(|q| q.expression.as_str()), Some("N"));
    assert!(p.has_label("PERSON") && !p.has_label("KNOWS") && p.label("KNOWS").is_none());
    assert_eq!(g.property_types("THING", "NAME").len(), 2);
    assert_eq!(g.property_types("PERSON", "K"), Vec::<&DataType>::new());
    let e = g.element("E").unwrap();
    assert_eq!(e.source().map(|s| s.columns.clone()), Some(vec!["FROM_K".to_owned()]));
    assert_eq!(e.destination().map(|s| s.columns.clone()), Some(vec!["TO_K".to_owned()]));
    assert!(p.source().is_none() && p.destination().is_none());
    assert!(e.names_column("TO_K") && e.names_column("K") && !e.names_column("V"));
    assert!(p.names_column("N") && !p.names_column("M"));
}

/// Verifies: [EXTENSION] §20, §22 GRF-0005; [ISO/IEC 9075:1992 §11.19
/// GR4]: its owner may query a graph, others with SELECT granted on it;
/// one WITH INVOKER RIGHTS needs SELECT on what it reads too; an INVALID
/// one is queried by none.
#[test]
fn who_may_query_a_graph_20() {
    let mut catalog = catalog();
    let mut g = graph();
    let who = |c: &MemoryCatalog, auth: &'static str, of: &GraphDesc| of.queryable(c, Who::sole(c.roles(), auth));
    assert!(who(&catalog, "A", &g).is_ok());
    assert_eq!(who(&catalog, "B", &g).map_err(|e| e.code), Err(Code::Privilege));
    g.select.push(UsageDesc { grantor: "A".into(), grantee: Some("B".into()), grantable: false });
    assert!(who(&catalog, "B", &g).is_ok(), "granted, of the owner's rights");
    g.invoker = true;
    assert_eq!(who(&catalog, "B", &g).map_err(|e| e.code), Err(Code::Privilege), "no SELECT on its tables");
    let select = |table: &str| PrivilegeDesc {
        grantor: "A".into(),
        grantee: None,
        table: a(table),
        actions: Actions { select: true, ..Actions::default() },
        grantable: false,
    };
    catalog.grant(vec![select("P"), select("Q"), select("E")]);
    assert!(who(&catalog, "B", &g).is_ok());
    assert_eq!(g.select_of(&catalog, Who::sole(catalog.roles(), "B")), (true, false));
    assert_eq!(g.select_of(&catalog, Who::sole(catalog.roles(), "A")), (true, true));
    assert_eq!(g.select_of(&catalog, Who::sole(catalog.roles(), "C")), (false, false));
    g.valid = false;
    let e = who(&catalog, "A", &g).unwrap_err();
    assert!(e.code == Code::SyntaxRule && e.msg.contains(GRAPH_INVALID), "{e:?}");
}

/// Verifies: [EXTENSION] §20: a name is written as a regular identifier
/// where one can write it, else delimited, its quotes doubled.
#[test]
fn names_are_written_as_identifiers() {
    assert_eq!(identifier("ACCOUNT_2"), "ACCOUNT_2");
    assert_eq!(identifier("owner"), "\"owner\"");
    assert_eq!(identifier("SELECT"), "\"SELECT\"");
    assert_eq!(identifier("2X"), "\"2X\"");
    assert_eq!(identifier("A\"B"), "\"A\"\"B\"");
    assert_eq!(identifier(""), "\"\"");
    assert_eq!(qualified_identifier(&QualifiedName { auth: "a".into(), ident: "T".into() }), "\"a\".T");
}
