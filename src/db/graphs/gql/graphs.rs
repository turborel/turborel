//! \[EXTENSION\] Mode C of the property graph implementation standard
//! (§4.3, RULES.md D110): graph types, and GQL graphs typed or open.
//!
//! CREATE GRAPH TYPE records a type, its definition kept as text and read
//! again where it is used. CREATE GRAPH makes, each statement logged as
//! it is made, a sequence of the graph's element numbers, its tables, and
//! the property graph over them, then records the GQL graph: of a closed
//! graph a table `G$T` of each node and edge type, "$ID" its key and, of
//! an edge, "$SRC" and "$DST" referencing its endpoint types' tables; of
//! an open graph `G$NODES` and `G$EDGES`, each element's labels as text,
//! `|A|B|`, and its properties a JSON object. DROP GRAPH drops the graph
//! with them; DROP GRAPH TYPE a type no graph is of, or, with CASCADE,
//! the graphs of it too.

use crate::ast::{GqlGraphDefinition, GraphTypeDefinition, GraphTyping, PropertyTypeDefinition, SchemaStatement, SqlStatement};
use crate::binder::qualify;
use crate::catalog::{GqlGraphDesc, GraphTypeDesc, QualifiedName, identifier, qualified_identifier};
use crate::db::ddl::Dropped;
use crate::db::definitions::{Defined, in_schema};
use crate::db::{Completion, Session, State, rule};
use crate::error::{Code, Result, SqlError};
use crate::parser::Parser;

/// The savepoint a GQL statement's statements are made under.
pub(super) const SAVEPOINT: &str = "\"$GQL\"";

/// The name of graph `g`'s table `suffix`, `G$SUFFIX`.
pub(crate) fn graph_table(g: &QualifiedName, suffix: &str) -> QualifiedName {
    QualifiedName { auth: g.auth.clone(), ident: format!("{}${suffix}", g.ident) }
}

/// The open graph's table of vertices.
pub(crate) const NODES: &str = "NODES";
/// The open graph's table of edges.
pub(crate) const EDGES: &str = "EDGES";
/// A GQL graph's sequence of element numbers.
pub(crate) const SEQUENCE: &str = "SEQ";

/// The graph type `text` defines.
///
/// # Errors
///
/// SQLCODE -102 if it defines none.
pub(crate) fn graph_type_of(text: &str) -> Result<GraphTypeDefinition> {
    let SqlStatement::Schema { statement: SchemaStatement::GraphType(t), .. } = Parser::for_stored(text)?.sql_statement()? else {
        return Err(rule(format!("[DESIGN] {text} defines no graph type")));
    };
    Ok(*t)
}

/// `p` as a column definition.
fn column(p: &PropertyTypeDefinition) -> String {
    format!("{} {}{}", identifier(&p.name), p.data_type, if p.not_null { " NOT NULL" } else { "" })
}

/// The statements that make closed graph `g` of type `t`.
fn closed_statements(g: &QualifiedName, t: &GraphTypeDefinition) -> Vec<String> {
    let table = |name: &str| qualified_identifier(&graph_table(g, name));
    let alias = |name: &str| identifier(&graph_table(g, name).ident);
    let mut out = vec![format!("CREATE SEQUENCE {}", table(SEQUENCE))];
    let mut vertices = Vec::new();
    for n in &t.nodes {
        let cols: Vec<String> =
            std::iter::once("\"$ID\" BIGINT NOT NULL PRIMARY KEY".to_owned()).chain(n.properties.iter().map(column)).collect();
        out.push(format!("CREATE TABLE {} ({})", table(&n.name), cols.join(", ")));
        vertices.push(format!("{} KEY (\"$ID\") {}", table(&n.name), labeled(&n.labels, &n.properties)));
    }
    let mut edges = Vec::new();
    for e in &t.edges {
        let mut cols = vec![
            "\"$ID\" BIGINT NOT NULL PRIMARY KEY".to_owned(),
            format!("\"$SRC\" BIGINT NOT NULL REFERENCES {}", table(&e.source)),
            format!("\"$DST\" BIGINT NOT NULL REFERENCES {}", table(&e.destination)),
        ];
        cols.extend(e.properties.iter().map(column));
        out.push(format!("CREATE TABLE {} ({})", table(&e.name), cols.join(", ")));
        edges.push(format!(
            "{} KEY (\"$ID\"){} SOURCE KEY (\"$SRC\") REFERENCES {} (\"$ID\") DESTINATION KEY (\"$DST\") REFERENCES {} (\"$ID\") {}",
            table(&e.name),
            if e.directed { "" } else { " UNDIRECTED" },
            alias(&e.source),
            alias(&e.destination),
            labeled(&e.labels, &e.properties)
        ));
    }
    let edge_tables = if edges.is_empty() { String::new() } else { format!(" EDGE TABLES ({})", edges.join(", ")) };
    out.push(format!("CREATE PROPERTY GRAPH {} VERTEX TABLES ({}){edge_tables}", qualified_identifier(g), vertices.join(", ")));
    out
}

/// Each label of `labels`, with every property of `properties`.
fn labeled(labels: &[String], properties: &[PropertyTypeDefinition]) -> String {
    let props = if properties.is_empty() {
        "NO PROPERTIES".to_owned()
    } else {
        format!("PROPERTIES ({})", properties.iter().map(|p| identifier(&p.name)).collect::<Vec<_>>().join(", "))
    };
    labels.iter().map(|l| format!("LABEL {} {props}", identifier(l))).collect::<Vec<_>>().join(" ")
}

/// The statements that make open graph `g`.
fn open_statements(g: &QualifiedName) -> Vec<String> {
    let (seq, nodes, edges) = (graph_table(g, SEQUENCE), graph_table(g, NODES), graph_table(g, EDGES));
    let (nodes_t, edges_t) = (qualified_identifier(&nodes), qualified_identifier(&edges));
    let alias = identifier(&nodes.ident);
    vec![
        format!("CREATE SEQUENCE {}", qualified_identifier(&seq)),
        format!("CREATE TABLE {nodes_t} (\"ID\" BIGINT NOT NULL PRIMARY KEY, \"LABELS\" VARCHAR(1000) NOT NULL, \"PROPS\" JSON)"),
        format!(
            "CREATE TABLE {edges_t} (\"ID\" BIGINT NOT NULL PRIMARY KEY, \"SRC\" BIGINT NOT NULL REFERENCES {nodes_t}, \"DST\" BIGINT NOT NULL REFERENCES {nodes_t}, \"LABELS\" VARCHAR(1000) NOT NULL, \"PROPS\" JSON, \"DIRECTED\" BOOLEAN NOT NULL)"
        ),
        format!(
            "CREATE PROPERTY GRAPH {} VERTEX TABLES ({nodes_t} KEY (\"ID\") LABEL \"NODE\" PROPERTIES (\"LABELS\", \"PROPS\")) EDGE TABLES ({edges_t} KEY (\"ID\") SOURCE KEY (\"SRC\") REFERENCES {alias} (\"ID\") DESTINATION KEY (\"DST\") REFERENCES {alias} (\"ID\") LABEL \"EDGE\" PROPERTIES (\"LABELS\", \"PROPS\", \"DIRECTED\"))",
            qualified_identifier(g)
        ),
    ]
}

impl Session {
    /// \[EXTENSION\] CREATE GRAPH `def`, written `text`, whose property
    /// graph is not yet made: its sequence, tables and property graph
    /// made, then `text` run again to record it, all or none.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the graph is there, or its type is not; as each
    /// statement's.
    pub(in crate::db) fn create_gql_graph(&self, def: &GqlGraphDefinition, text: &str) -> Result<Completion> {
        let st = self.gql_state()?;
        let qn = qualify(&def.name, &self.default_schema());
        if st.catalog.gql().gql_graph(&qn).is_some() {
            if def.if_not_exists {
                return Ok(Completion::Success);
            }
            return Err(SqlError::new(Code::SyntaxRule, format!("[EXTENSION] there is a graph {qn}")).or_at(def.name.at));
        }
        let statements = match &def.typing {
            GraphTyping::Typed(t) => {
                let tq = qualify(t, &self.default_schema());
                let Some(desc) = st.catalog.gql().graph_type(&tq) else {
                    return Err(SqlError::at(Code::SyntaxRule, t.at, format!("[EXTENSION] GRF-0012 there is no graph type {tq}")));
                };
                closed_statements(&qn, &graph_type_of(&desc.definition)?)
            }
            GraphTyping::Open => open_statements(&qn),
        };
        self.all_or_none(statements.iter().map(String::as_str).chain([text]))
    }

    /// Runs each of `statements`, under a savepoint: all, or, if one
    /// fails, none.
    pub(super) fn all_or_none<'s>(&self, statements: impl Iterator<Item = &'s str>) -> Result<Completion> {
        self.run(&format!("SAVEPOINT {SAVEPOINT}"))?;
        for sql in statements {
            if let Err(e) = self.run(sql) {
                self.run(&format!("ROLLBACK TO SAVEPOINT {SAVEPOINT}"))?;
                self.run(&format!("RELEASE SAVEPOINT {SAVEPOINT}"))?;
                return Err(e);
            }
        }
        self.run(&format!("RELEASE SAVEPOINT {SAVEPOINT}"))
    }

    /// Whether CREATE GRAPH `def` is yet to make its graph's property
    /// graph, and so its tables.
    ///
    /// # Errors
    ///
    /// As reading the definitions.
    pub(in crate::db) fn gql_graph_unmade(&self, def: &GqlGraphDefinition) -> Result<bool> {
        let st = self.gql_state()?;
        Ok(st.catalog.graph(&qualify(&def.name, &self.default_schema())).is_none())
    }
}

impl State {
    /// \[EXTENSION\] CREATE GRAPH TYPE `t` in schema `schema`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is a graph type of its name, two of its
    /// types have one name, or an edge type connects a node type it has
    /// not (GRF-0012).
    pub(in crate::db) fn create_graph_type(&mut self, schema: &str, t: &GraphTypeDefinition) -> Result<Defined> {
        let qn = qualify(&t.name, schema);
        if self.catalog.gql().graph_type(&qn).is_some() {
            if t.if_not_exists {
                return Ok(Defined::default());
            }
            return Err(rule(format!("[EXTENSION] there is a graph type {qn}")).or_at(t.name.at));
        }
        if !self.catalog.schema_exists(&qn.auth) {
            return Err(
                rule(format!("[EXTENSION] there is no schema {0}, which CREATE SCHEMA AUTHORIZATION {0} makes", qn.auth)).or_at(t.name.at)
            );
        }
        validate_type(t)?;
        let mut catalog = self.catalog.duplicate();
        catalog.gql_mut().put_graph_type(GraphTypeDesc { name: qn.clone(), definition: type_text(&qn, t) });
        self.catalog = std::sync::Arc::from(catalog);
        Ok(Defined { locks: in_schema(&qn, Vec::new()), ..Defined::default() })
    }

    /// \[EXTENSION\] CREATE GRAPH `def` in schema `schema`, its property
    /// graph and tables made: recorded.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if its property graph is not over the tables its
    /// typing makes: one defined otherwise.
    pub(in crate::db) fn record_gql_graph(&mut self, schema: &str, def: &GqlGraphDefinition) -> Result<Defined> {
        let qn = qualify(&def.name, schema);
        let graph_type = match &def.typing {
            GraphTyping::Typed(t) => Some(qualify(t, schema)),
            GraphTyping::Open => None,
        };
        let expected: Vec<QualifiedName> = match &graph_type {
            Some(t) => {
                let desc =
                    self.catalog.gql().graph_type(t).ok_or_else(|| rule(format!("[EXTENSION] GRF-0012 there is no graph type {t}")))?;
                let typed = graph_type_of(&desc.definition)?;
                typed.nodes.iter().map(|n| &n.name).chain(typed.edges.iter().map(|e| &e.name)).map(|n| graph_table(&qn, n)).collect()
            }
            None => vec![graph_table(&qn, NODES), graph_table(&qn, EDGES)],
        };
        let made = self
            .catalog
            .graph(&qn)
            .is_some_and(|g| g.elements.len() == expected.len() && g.elements.iter().all(|e| expected.contains(&e.table)));
        if !made {
            return Err(rule(format!("[EXTENSION] there is a property graph {qn} of other tables than a GQL graph's")).or_at(def.name.at));
        }
        let mut catalog = self.catalog.duplicate();
        catalog.gql_mut().put_gql_graph(GqlGraphDesc { name: qn.clone(), graph_type });
        self.catalog = std::sync::Arc::from(catalog);
        Ok(Defined { locks: in_schema(&qn, Vec::new()), ..Defined::default() })
    }

    /// \[EXTENSION\] Drops GQL graph `qn`: its property graph, tables and
    /// sequence, whichever are still there.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no GQL graph `qn`; as the drops'.
    pub(in crate::db) fn drop_gql_graph(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        let Some(g) = self.catalog.gql().gql_graph(qn).cloned() else {
            return Err(rule(format!("[EXTENSION] there is no GQL graph {qn}")));
        };
        let mut dropped = Dropped::default();
        let tables: Vec<QualifiedName> = match &g.graph_type {
            Some(t) => match self.catalog.gql().graph_type(t) {
                Some(d) => {
                    let def = graph_type_of(&d.definition)?;
                    def.edges.iter().map(|e| &e.name).chain(def.nodes.iter().map(|n| &n.name)).map(|n| graph_table(qn, n)).collect()
                }
                None => Vec::new(),
            },
            None => vec![graph_table(qn, EDGES), graph_table(qn, NODES)],
        };
        let mut catalog = self.catalog.duplicate();
        catalog.gql_mut().remove_gql_graph(qn);
        self.catalog = std::sync::Arc::from(catalog);
        if self.catalog.graph(qn).is_some() {
            dropped.absorb(self.drop_graph(qn, cascade)?);
        }
        for t in tables.iter().filter(|t| self.catalog.table(t).is_some()).collect::<Vec<_>>() {
            dropped.absorb(self.drop_table(t, cascade)?);
        }
        let seq = graph_table(qn, SEQUENCE);
        if self.catalog.sequence(&seq).is_some() {
            dropped.absorb(self.drop_sequence(&seq, cascade)?);
        }
        dropped.names.push(qn.clone());
        Ok(dropped)
    }

    /// \[EXTENSION\] Drops graph type `qn`, and with CASCADE the graphs of
    /// it.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is none, or, not `cascade`, a graph is of it.
    pub(in crate::db) fn drop_graph_type(&mut self, qn: &QualifiedName, cascade: bool) -> Result<Dropped> {
        if self.catalog.gql().graph_type(qn).is_none() {
            return Err(rule(format!("[EXTENSION] there is no graph type {qn}")));
        }
        let typed: Vec<QualifiedName> =
            self.catalog.gql().gql_graphs().into_iter().filter(|g| g.graph_type.as_ref() == Some(qn)).map(|g| g.name.clone()).collect();
        if let (Some(g), false) = (typed.first(), cascade) {
            return Err(rule(format!("[EXTENSION] graph {g} is of graph type {qn}, so it is dropped only with CASCADE")));
        }
        let mut dropped = Dropped::default();
        for g in &typed {
            dropped.absorb(self.drop_gql_graph(g, true)?);
        }
        let mut catalog = self.catalog.duplicate();
        catalog.gql_mut().remove_graph_type(qn);
        self.catalog = std::sync::Arc::from(catalog);
        dropped.names.push(qn.clone());
        Ok(dropped)
    }

    /// The graph types and GQL graphs of schema `auth`, dropped with it.
    ///
    /// # Errors
    ///
    /// As `drop_gql_graph` and `drop_graph_type`.
    pub(in crate::db) fn drop_schema_gql(&mut self, auth: &str) -> Result<Dropped> {
        let mut dropped = Dropped::default();
        let graphs: Vec<QualifiedName> =
            self.catalog.gql().gql_graphs().into_iter().filter(|g| g.name.auth == auth).map(|g| g.name.clone()).collect();
        for g in &graphs {
            dropped.absorb(self.drop_gql_graph(g, true)?);
        }
        let types: Vec<QualifiedName> =
            self.catalog.gql().graph_types().into_iter().filter(|t| t.name.auth == auth).map(|t| t.name.clone()).collect();
        for t in &types {
            dropped.absorb(self.drop_graph_type(t, true)?);
        }
        Ok(dropped)
    }
}

/// That graph type `t` is whole: its types' names distinct, each edge
/// type's endpoints its node types, and each type's properties distinct.
///
/// # Errors
///
/// SQLCODE -103 (GRF-0012) if not.
fn validate_type(t: &GraphTypeDefinition) -> Result<()> {
    let mut names: Vec<&str> = Vec::new();
    let all = t.nodes.iter().map(|n| (&n.name, &n.properties, n.at)).chain(t.edges.iter().map(|e| (&e.name, &e.properties, e.at)));
    for (name, props, at) in all {
        if names.contains(&name.as_str()) {
            return Err(SqlError::at(Code::SyntaxRule, at, format!("[EXTENSION] graph type {} has two types {name}", t.name.ident)));
        }
        names.push(name);
        if let Some((_, p)) = props.iter().enumerate().find(|(i, p)| props.iter().take(*i).any(|q| q.name == p.name)) {
            return Err(SqlError::at(Code::SyntaxRule, at, format!("[EXTENSION] type {name} has two properties {}", p.name)));
        }
    }
    for e in &t.edges {
        if let Some(end) = [&e.source, &e.destination].into_iter().find(|n| !t.nodes.iter().any(|x| x.name == **n)) {
            return Err(SqlError::at(
                Code::SyntaxRule,
                e.at,
                format!("[EXTENSION] GRF-0012 edge type {} connects {end}, no node type of the graph type", e.name),
            ));
        }
    }
    Ok(())
}

/// Graph type `t`, named `qn`, as its CREATE GRAPH TYPE statement.
fn type_text(qn: &QualifiedName, t: &GraphTypeDefinition) -> String {
    let props = |ps: &[PropertyTypeDefinition]| {
        if ps.is_empty() { String::new() } else { format!(" {{{}}}", ps.iter().map(column).collect::<Vec<_>>().join(", ")) }
    };
    let labels = |ls: &[String]| format!(" LABELS {}", ls.iter().map(|l| identifier(l)).collect::<Vec<_>>().join(" & "));
    let mut types: Vec<String> =
        t.nodes.iter().map(|n| format!("NODE TYPE {}{}{}", identifier(&n.name), labels(&n.labels), props(&n.properties))).collect();
    types.extend(t.edges.iter().map(|e| {
        format!(
            "{} EDGE TYPE {}{}{} CONNECTING ({} {} {})",
            if e.directed { "DIRECTED" } else { "UNDIRECTED" },
            identifier(&e.name),
            labels(&e.labels),
            props(&e.properties),
            identifier(&e.source),
            if e.directed { "->" } else { "~" },
            identifier(&e.destination)
        )
    }));
    format!("CREATE GRAPH TYPE {} {{{}}}", qualified_identifier(qn), types.join(", "))
}
