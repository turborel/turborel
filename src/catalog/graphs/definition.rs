//! \[EXTENSION\] A property graph's definition as text (§20, DDL text
//! retrieval): the CREATE PROPERTY GRAPH statement that defines the graph
//! as its descriptor holds it now, ALTERs and all. Each name is written
//! qualified, delimited where a regular identifier cannot write it, and
//! each property's expression as kept, so the text defines the same graph
//! wherever it is run.

use std::fmt::Write as _;

use super::{ElementKind, ElementTableDesc, EndpointDesc, GraphDesc, GraphMode, LabelDesc, identifier, qualified_identifier};

impl GraphDesc {
    /// The CREATE PROPERTY GRAPH statement that defines it as it is.
    pub fn definition(&self) -> String {
        let mut text = format!("CREATE PROPERTY GRAPH {}", qualified_identifier(&self.name));
        for (kind, words) in [(ElementKind::Vertex, "VERTEX TABLES"), (ElementKind::Edge, "EDGE TABLES")] {
            let tables: Vec<String> = self.elements.iter().filter(|e| e.kind == kind).map(element_text).collect();
            if !tables.is_empty() {
                let _ = write!(text, " {words} ({})", tables.join(", "));
            }
        }
        let mode = match self.mode {
            GraphMode::Enforced => "ENFORCED",
            GraphMode::Trusted => "TRUSTED",
        };
        let _ = write!(text, " OPTIONS ({mode} MODE)");
        if self.invoker {
            text.push_str(" WITH INVOKER RIGHTS");
        }
        text
    }
}

/// The definition of element table `e`.
fn element_text(e: &ElementTableDesc) -> String {
    let mut text = e.from_text();
    if e.query.is_some() || e.alias != e.table.ident {
        let _ = write!(text, " AS {}", identifier(&e.alias));
    }
    let _ = write!(text, " KEY ({})", names(&e.key));
    if let Some(ends) = &e.endpoints {
        if ends.undirected {
            text.push_str(" UNDIRECTED");
        }
        let _ = write!(text, " SOURCE {} DESTINATION {}", endpoint_text(&ends.source), endpoint_text(&ends.destination));
    }
    for l in &e.labels {
        text.push(' ');
        text.push_str(&label_text(l));
    }
    text
}

/// The text of an endpoint, after SOURCE or DESTINATION.
fn endpoint_text(end: &EndpointDesc) -> String {
    format!("KEY ({}) REFERENCES {} ({})", names(&end.columns), identifier(&end.vertex), names(&end.references))
}

/// The text of label `l` and its properties: each property its expression
/// alone if that is its name as an identifier, else with AS.
fn label_text(l: &LabelDesc) -> String {
    let label = format!("LABEL {}", identifier(&l.name));
    if l.properties.is_empty() {
        return format!("{label} NO PROPERTIES");
    }
    let each = l.properties.iter().map(|p| {
        let name = identifier(&p.name);
        if p.expression == name { name } else { format!("{} AS {name}", p.expression) }
    });
    format!("{label} PROPERTIES ({})", each.collect::<Vec<_>>().join(", "))
}

/// `columns` as a `<column name list>` is written.
fn names(columns: &[String]) -> String {
    columns.iter().map(|c| identifier(c)).collect::<Vec<_>>().join(", ")
}
