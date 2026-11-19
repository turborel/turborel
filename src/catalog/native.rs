//! \[EXTENSION\] Native node and edge tables (RULES.md D143),
//! Mode B of the property graph implementation standard (§4.2): the hidden
//! pseudo-columns a table created AS NODE or AS EDGE has, what tells the
//! two apart, the identity text of a node or edge (R5.1), and a CONNECTION
//! constraint's description.
//!
//! A node table has `$NODE_ID`, an edge table `$EDGE_ID`, `$FROM_ID` and
//! `$TO_ID`, each after the columns its definition writes. The names are
//! the standard's `$node_id`, `$edge_id`, `$from_id` and `$to_id` as a
//! `<regular identifier>` is folded to upper case (R5.7). No `*` gives
//! them, and the Information Schema shows them hidden.

use std::fmt::Write as _;

use super::{QualifiedName, SequenceDesc, TableDesc};
use crate::json::{Keys, Scalar, parse};
use crate::value::{CharUnits, DataType, Value};

/// The pseudo-column of a node's identity (§4.2).
pub const NODE_ID: &str = "$NODE_ID";
/// The pseudo-column of an edge's identity (§4.2).
pub const EDGE_ID: &str = "$EDGE_ID";
/// The pseudo-column of the node an edge leaves (§4.2).
pub const FROM_ID: &str = "$FROM_ID";
/// The pseudo-column of the node an edge enters (§4.2).
pub const TO_ID: &str = "$TO_ID";
/// The pseudo-column names, which a `$` begins a `<regular identifier>` of
/// (`lexer/pseudo.rs`).
pub const PSEUDO_COLUMNS: [&str; 4] = [NODE_ID, EDGE_ID, FROM_ID, TO_ID];
/// The implicit property graph of a schema's node and edge tables (§4.2).
pub const SYS_GRAPH: &str = "SYS_GRAPH";
/// \[DESIGN\] The maximum length of an identity's text: two identifiers of
/// at most 128 characters, each escaped at worst to six, the digits of an
/// identity and the rest of the object.
pub const ID_LENGTH: u32 = 2048;

/// The key of an identity's kind, "node" or "edge".
const TYPE_KEY: &str = "type";
/// The key of the schema of the identity's table.
const SCHEMA_KEY: &str = "schema";
/// The key of the identity's table.
const TABLE_KEY: &str = "table";
/// The key of the identity's number in its table.
const ID_KEY: &str = "id";
/// The kind of a node's identity.
const NODE: &str = "node";
/// The kind of an edge's identity.
const EDGE: &str = "edge";
/// [RFC 8259 §7] The last of the control characters a JSON string escapes.
const LAST_CONTROL: char = '\u{1F}';

/// \[EXTENSION\] Whether a native table is of nodes or of edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeKind {
    /// Created AS NODE.
    Node,
    /// Created AS EDGE.
    Edge,
}

/// \[EXTENSION\] A pseudo-column of a native table, and how its values are
/// given.
#[derive(Debug, Clone, PartialEq)]
pub enum GraphColumn {
    /// `$NODE_ID`, of a node table: each row inserted is given the next
    /// value of the table's own generator, as an identity's text.
    Node(SequenceDesc),
    /// `$EDGE_ID`, of an edge table, given as `$NODE_ID` is.
    Edge(SequenceDesc),
    /// `$FROM_ID`, of an edge table: the identity of a node, given by the
    /// statement that writes the row.
    From,
    /// `$TO_ID`, as `$FROM_ID`.
    To,
}

impl GraphColumn {
    /// Its name.
    pub fn name(&self) -> &'static str {
        match self {
            GraphColumn::Node(_) => NODE_ID,
            GraphColumn::Edge(_) => EDGE_ID,
            GraphColumn::From => FROM_ID,
            GraphColumn::To => TO_ID,
        }
    }

    /// Of `$NODE_ID` or `$EDGE_ID`, the generator its values are drawn
    /// from.
    pub fn sequence(&self) -> Option<&SequenceDesc> {
        match self {
            GraphColumn::Node(s) | GraphColumn::Edge(s) => Some(s),
            GraphColumn::From | GraphColumn::To => None,
        }
    }
}

/// The type of every pseudo-column: CHARACTER VARYING of an identity's
/// text.
pub fn id_type() -> DataType {
    DataType::Varying { length: ID_LENGTH, units: CharUnits::Characters }
}

/// \[EXTENSION\] A CONNECTION constraint (§4.2, R12.2): the pairs of node
/// tables an edge of its table may join, each its source then its
/// destination, and whether a node's delete deletes the edges that join it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    /// Each `<node table> TO <node table>` clause, in order.
    pub clauses: Vec<(QualifiedName, QualifiedName)>,
    /// ON DELETE CASCADE, rather than NO ACTION.
    pub cascade: bool,
}

impl Connection {
    /// Whether an edge from a node of `from` to one of `to` is one of its
    /// clauses.
    pub fn allows(&self, from: &QualifiedName, to: &QualifiedName) -> bool {
        self.clauses.iter().any(|(f, t)| f == from && t == to)
    }

    /// Whether a clause names node table `node` at either end.
    pub fn names(&self, node: &QualifiedName) -> bool {
        self.clauses.iter().any(|(f, t)| f == node || t == node)
    }
}

/// \[EXTENSION\] The identity of a node or edge (R5.1): its table and its
/// number there, written as the JSON object `{"type":"node","schema":S,
/// "table":T,"id":N}`, the form §4.2 keeps for compatibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementId {
    /// Of a node, rather than an edge.
    pub node: bool,
    /// Its table.
    pub table: QualifiedName,
    /// Its number in its table.
    pub id: i128,
}

impl ElementId {
    /// Its text, its members in the order of the type's documentation, with
    /// no white space: one text for one identity, so two are equal as
    /// strings exactly when they are one identity.
    pub fn text(&self) -> String {
        let kind = if self.node { NODE } else { EDGE };
        let mut out = String::new();
        // Writing to a String fails never.
        let _infallible = write!(
            out,
            "{{\"{TYPE_KEY}\":\"{kind}\",\"{SCHEMA_KEY}\":{},\"{TABLE_KEY}\":{},\"{ID_KEY}\":{}}}",
            quoted(&self.table.auth),
            quoted(&self.table.ident),
            self.id
        );
        out
    }

    /// The identity `text` is, written as any JSON text of the object's
    /// four members, if it is one.
    pub fn parse(text: &str) -> Option<ElementId> {
        let item = parse(text, Keys::Unique).ok()?;
        let members = item.as_object()?;
        if members.len() != PSEUDO_COLUMNS.len() {
            return None;
        }
        let member = |key: &str| members.iter().find(|m| m.key == key).and_then(|m| m.value.as_scalar()).map(Scalar::value);
        let string = |key: &str| if let Some(Value::Char(s)) = member(key) { Some(s.clone()) } else { None };
        let node = match string(TYPE_KEY)?.as_str() {
            NODE => true,
            EDGE => false,
            _ => return None,
        };
        let Some(Value::Exact(d)) = member(ID_KEY) else { return None };
        let id = (d.scale == 0).then_some(d.units)?;
        Some(ElementId { node, table: QualifiedName { auth: string(SCHEMA_KEY)?, ident: string(TABLE_KEY)? }, id })
    }
}

/// [RFC 8259 §7] `s` as a JSON string: quoted, its quotation marks and
/// reverse solidi escaped, and its control characters as `\u` escapes.
fn quoted(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
            out.push(c);
        } else if c <= LAST_CONTROL {
            // Writing to a String fails never.
            let _infallible = write!(out, "\\u{:04x}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out.push('"');
    out
}

impl TableDesc {
    /// Whether it was created AS NODE or AS EDGE, if either.
    pub fn native(&self) -> Option<NativeKind> {
        self.columns.iter().find_map(|c| match c.graph_column()? {
            GraphColumn::Node(_) => Some(NativeKind::Node),
            GraphColumn::Edge(_) => Some(NativeKind::Edge),
            GraphColumn::From | GraphColumn::To => None,
        })
    }

    /// The position of its pseudo-column `name`, if it has it.
    pub fn pseudo_column(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.graph_column().is_some_and(|g| g.name() == name))
    }

    /// Its CONNECTION constraints, with their names.
    pub fn connections(&self) -> impl Iterator<Item = (&QualifiedName, &Connection)> {
        self.constraints.iter().filter_map(|c| match &c.kind {
            super::ConstraintKind::Connection(k) => Some((&c.name, k)),
            super::ConstraintKind::NotNull(_)
            | super::ConstraintKind::Unique { .. }
            | super::ConstraintKind::ForeignKey(_)
            | super::ConstraintKind::Check { .. } => None,
        })
    }
}
