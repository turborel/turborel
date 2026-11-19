//! \[EXTENSION\] The descriptor of a property graph defined over tables
//! (RULES.md D140), Mode A of the property graph implementation
//! standard (§4.1): its vertex and edge element tables, each with its key,
//! labels and properties, an edge's source and destination key
//! references, its mode, and what it reads, as a view's descriptor keeps
//! them. A graph stores no data (R4.7).
//!
//! It is also the read API a query of a graph binds through: the element
//! tables by alias, the tables a label is on, a property's expression
//! text and type, and the endpoint key mappings. A property's expression
//! is kept as text, over the columns of its element table, and bound again
//! where it is read, as a generated column's is (`Parser::for_stored`).

mod definition;
#[cfg(test)]
mod tests;

use super::{Catalog, Horizon, QualifiedName, UsageDesc, Who, applicable, owner_of};
use crate::error::{Code, Result, SqlError};
use crate::lexer::KEY_WORDS;
use crate::value::DataType;

/// The condition of the property graph implementation standard (§22) a
/// query of a graph marked INVALID raises.
pub const GRAPH_INVALID: &str = "GRF-0005";

/// \[EXTENSION\] (§4.1 R4.3) How a graph's keys and endpoint references
/// are held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphMode {
    /// Each key is a declared PRIMARY KEY or UNIQUE constraint of NOT NULL
    /// columns, and each endpoint reference a declared FOREIGN KEY.
    Enforced,
    /// Keys and endpoint references are taken as written; an edge whose
    /// endpoint matches no vertex is left out as the graph is read.
    Trusted,
}

/// \[EXTENSION\] Whether an element table is of vertices or of edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementKind {
    /// A vertex table.
    Vertex,
    /// An edge table.
    Edge,
}

/// \[EXTENSION\] A property of a label of an element table (§4.1 R4.5).
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyDesc {
    /// Its name.
    pub name: String,
    /// Its value expression, over the columns of its element table, as
    /// text; a column alone is its delimited name.
    pub expression: String,
    /// Its declared type, as the expression bound as the graph was last
    /// defined, altered or compiled.
    pub data_type: DataType,
}

/// \[EXTENSION\] A label of an element table and its properties.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelDesc {
    /// Its name (§4.1 R4.6: the table's name of DEFAULT LABEL).
    pub name: String,
    /// Its properties, in order.
    pub properties: Vec<PropertyDesc>,
}

/// \[EXTENSION\] The source or destination of an edge table: the vertex
/// table it references, and the columns of each that match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointDesc {
    /// The alias of the vertex table referenced.
    pub vertex: String,
    /// The columns of the edge table, in order.
    pub columns: Vec<String>,
    /// The columns of the vertex table each matches, in order.
    pub references: Vec<String>,
}

/// \[EXTENSION\] An edge table's two endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// Its source.
    pub source: EndpointDesc,
    /// Its destination.
    pub destination: EndpointDesc,
    /// Whether its edges are undirected (§4.1, extension A.1): each
    /// joins its two vertices with no direction.
    pub undirected: bool,
}

/// \[EXTENSION\] An element table of a graph.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementTableDesc {
    /// Its alias, unique among the graph's element tables.
    pub alias: String,
    /// The base table or view it is.
    pub table: QualifiedName,
    /// Of vertices or of edges.
    pub kind: ElementKind,
    /// Its key columns, in order (§4.1 R4.2).
    pub key: Vec<String>,
    /// Its labels, in order; one at least.
    pub labels: Vec<LabelDesc>,
    /// Of an edge table, its endpoints.
    pub endpoints: Option<Endpoints>,
    /// \[EXTENSION\] [GS §11.3] Of an element table of a query's rows, the
    /// query as written; its `table` is then its alias in no schema, no
    /// table's name.
    pub query: Option<String>,
}

impl ElementTableDesc {
    /// Whether it has the label `name`.
    pub fn has_label(&self, name: &str) -> bool {
        self.labels.iter().any(|l| l.name == name)
    }

    /// Its label `name`, if it has it.
    pub fn label(&self, name: &str) -> Option<&LabelDesc> {
        self.labels.iter().find(|l| l.name == name)
    }

    /// Its properties, of all its labels, each once, in the order first
    /// met: a property of two labels has one expression (§4.1 R4.4).
    pub fn properties(&self) -> Vec<&PropertyDesc> {
        let mut all: Vec<&PropertyDesc> = Vec::new();
        for p in self.labels.iter().flat_map(|l| &l.properties) {
            if !all.iter().any(|q| q.name == p.name) {
                all.push(p);
            }
        }
        all
    }

    /// Its property `name`, of any label, if it has it.
    pub fn property(&self, name: &str) -> Option<&PropertyDesc> {
        self.labels.iter().flat_map(|l| &l.properties).find(|p| p.name == name)
    }

    /// Of an edge table, its source.
    pub fn source(&self) -> Option<&EndpointDesc> {
        self.endpoints.as_ref().map(|e| &e.source)
    }

    /// Of an edge table, its destination.
    pub fn destination(&self) -> Option<&EndpointDesc> {
        self.endpoints.as_ref().map(|e| &e.destination)
    }

    /// What a FROM clause reads it from: its query in parentheses, or its
    /// table's name.
    pub fn from_text(&self) -> String {
        self.query.as_ref().map_or_else(|| qualified_identifier(&self.table), |q| format!("({q})"))
    }

    /// Whether it names the column `column` of its table as a key, an
    /// endpoint's column or a property that is the column alone.
    pub fn names_column(&self, column: &str) -> bool {
        let endpoint = |e: &EndpointDesc| e.columns.iter().any(|c| c == column);
        self.key.iter().any(|c| c == column)
            || self.endpoints.as_ref().is_some_and(|e| endpoint(&e.source) || endpoint(&e.destination))
            || self.properties().iter().any(|p| p.expression == identifier(column))
    }
}

/// \[EXTENSION\] A property graph's descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphDesc {
    /// Its name, of the schema it is in.
    pub name: QualifiedName,
    /// Its vertex tables, then its edge tables, each in the order defined.
    pub elements: Vec<ElementTableDesc>,
    /// How its keys and endpoints are held.
    pub mode: GraphMode,
    /// WITH INVOKER RIGHTS: a query of it needs SELECT on its element
    /// tables too, as its user holds them.
    pub invoker: bool,
    /// The schema of the names in its expressions written without one:
    /// the session's default schema as it was defined.
    pub names: String,
    /// The tables and views its element tables are, and those their
    /// properties' expressions read.
    pub reads: Vec<QualifiedName>,
    /// What its properties' expressions see, as a view's query does
    /// (`Horizon`): the routines they invoke among them.
    pub horizon: Horizon,
    /// Whether it is valid: false once a change of what it reads broke it,
    /// until ALTER PROPERTY GRAPH ... COMPILE finds it holds (§4.1 R4.7).
    pub valid: bool,
    /// The SELECT privileges granted on it, each by a grantor; its owner's
    /// is not kept but found from its element tables (`select_of`).
    pub select: Vec<UsageDesc>,
}

impl GraphDesc {
    /// Its element table of alias `alias`, if any.
    pub fn element(&self, alias: &str) -> Option<&ElementTableDesc> {
        self.elements.iter().find(|e| e.alias == alias)
    }

    /// Its vertex tables, in order.
    pub fn vertices(&self) -> impl Iterator<Item = &ElementTableDesc> {
        self.elements.iter().filter(|e| e.kind == ElementKind::Vertex)
    }

    /// Its edge tables, in order.
    pub fn edges(&self) -> impl Iterator<Item = &ElementTableDesc> {
        self.elements.iter().filter(|e| e.kind == ElementKind::Edge)
    }

    /// Every label of its element tables of kind `kind`, each once, in the
    /// order first met; of both kinds if none.
    pub fn labels(&self, kind: Option<ElementKind>) -> Vec<&str> {
        let mut all: Vec<&str> = Vec::new();
        for l in self.elements.iter().filter(|e| kind.is_none_or(|k| e.kind == k)).flat_map(|e| &e.labels) {
            if !all.contains(&l.name.as_str()) {
                all.push(&l.name);
            }
        }
        all
    }

    /// Its element tables that have the label `label`, in order.
    pub fn labeled(&self, label: &str) -> Vec<&ElementTableDesc> {
        self.elements.iter().filter(|e| e.has_label(label)).collect()
    }

    /// Its element tables that are the table or view `table`.
    pub fn of_table(&self, table: &QualifiedName) -> Vec<&ElementTableDesc> {
        self.elements.iter().filter(|e| e.table == *table).collect()
    }

    /// That it may be queried: valid.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 (GRF-0005) if it is INVALID.
    pub fn usable(&self) -> Result<()> {
        if self.valid {
            return Ok(());
        }
        Err(SqlError::new(
            Code::SyntaxRule,
            format!(
                "[EXTENSION] {GRAPH_INVALID} property graph {} is INVALID since a table it reads changed, until ALTER PROPERTY GRAPH {} COMPILE",
                self.name, self.name
            ),
        ))
    }

    /// That `who` may query it (§20): it is valid, `who` holds SELECT on
    /// it, and, of a graph WITH INVOKER RIGHTS, SELECT on each table and
    /// view it reads too. Else, as a view is, it is read with its owner's
    /// rights.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 (GRF-0005) if it is INVALID; -104 if a privilege is
    /// not held.
    pub fn queryable(&self, catalog: &dyn Catalog, who: Who<'_>) -> Result<()> {
        self.usable()?;
        if !self.select_of(catalog, who).0 {
            let msg = format!("[EXTENSION] {} holds no SELECT on property graph {} (§20)", who.auth, self.name);
            return Err(SqlError::new(Code::Privilege, msg));
        }
        if let Some(t) = self.reads.iter().find(|t| self.invoker && !applicable(catalog, who, t).0.select) {
            let msg =
                format!("[EXTENSION] property graph {} is WITH INVOKER RIGHTS, and {} holds no SELECT on {t} (§20)", self.name, who.auth);
            return Err(SqlError::new(Code::Privilege, msg));
        }
        Ok(())
    }

    /// The declared types of property `name` of label `label`, of each
    /// element table whose label has it: the type it is read as is
    /// their union's (`binder::property_type`, R5.6).
    pub fn property_types(&self, label: &str, name: &str) -> Vec<&DataType> {
        let labels = self.elements.iter().filter_map(|e| e.label(label));
        labels.filter_map(|l| l.properties.iter().find(|p| p.name == name)).map(|p| &p.data_type).collect()
    }

    /// Whether `who` holds SELECT on it, and whether grantably: its owner
    /// does, grantably if it holds SELECT grantably on every table it
    /// reads, as the owner of a view holds SELECT on it [ISO/IEC 9075:1992
    /// §11.19 GR4]; another as granted an identifier it enables or
    /// PUBLIC.
    pub fn select_of(&self, catalog: &dyn Catalog, who: Who<'_>) -> (bool, bool) {
        let owner = owner_of(catalog, &self.name.auth);
        if who.includes(owner) {
            let sole = Who::sole(who.roles, owner);
            return (true, self.reads.iter().all(|t| applicable(catalog, sole, t).1.select));
        }
        let granted = self.select.iter().filter(|u| who.holds(u.grantee.as_deref()));
        granted.fold((false, false), |(_, grantable), u| (true, grantable || u.grantable))
    }
}

/// `name` as an `<identifier>` is written: a `<regular identifier>` if it
/// can be one, else delimited, its quotes doubled.
pub fn identifier(name: &str) -> String {
    let mut chars = name.chars();
    let regular = chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !KEY_WORDS.contains(&name);
    if regular { name.to_owned() } else { format!("\"{}\"", name.replace('"', "\"\"")) }
}

/// The name `qn` as a `<table name>` is written, its schema too.
pub fn qualified_identifier(qn: &QualifiedName) -> String {
    format!("{}.{}", identifier(&qn.auth), identifier(&qn.ident))
}
