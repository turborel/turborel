//! SQL/XML [ISO/IEC 9075-14:2006 draft §4.2]: XML values, which are
//! XQuery sequences [§4.2.3], and what makes them. Part 14 is cited by its
//! 2006 draft, and the W3C texts it names by its own reference keys
//! [§2.2], at the editions it names: [XML 1.0] Third Edition, [XML 1.1],
//! [Namespaces 1.0] of 1999 and [Namespaces 1.1], \[Infoset\] Second
//! Edition, \[XQueryDM\] the data model of XQuery 1.0 and XPath 2.0 as a
//! Candidate Recommendation of 3 November 2005. \[XML\] and \[Namespaces\]
//! cite what both versions say alike.
//!
//! An XML text is parsed (`xml/parse.rs`) [§10.16] by the characters and
//! names of its version (`xml/chars.rs`) into a tree of the data model's
//! nodes (`xml/tree.rs`) [XQueryDM §6], built in document order
//! (`xml/tree/build.rs`). XMLDOCUMENT's document node is built by
//! XQuery's computed document constructor (`xml/construct.rs`) [§6.13].
//! Atomic values are cast as XQuery casts them (`xml/atomic.rs`) [§6.6].
//!
//! \[DESIGN\] RULES.md D86: a tree is an arena of its nodes in document
//! order, element and attribute names tokenized into a table of the tree,
//! text in one buffer. Every walk of a tree follows its links, so none
//! recurses, and elements nest at most `MAX_NESTING` deep.

mod atomic;
mod chars;
mod construct;
mod element;
mod identifiers;
mod parse;
mod publish;
pub mod schema;
mod sequence;
mod serialize;
mod stored;
mod tree;

pub use atomic::{
    Exact, LAST_YEAR, Moment, Ordered, Primitive, Production, Shape, WhiteSpace, base64, base64_text, canonical, cast, compare,
    day_time_text, digits, double, duration_units, duration_value, equal, from_lexical, hex, hex_text, length, normalize, real_text,
    xquery_error, year_month_text,
};
pub use chars::{
    COLON, VERSION_10, VERSION_11, Version, is_char, is_literal, is_name, is_name_char, is_name_start, is_ncname, is_nmtoken,
    is_pubid_char, is_restricted, is_space, qname_parts,
};
pub use construct::document;
pub use element::element;
pub use identifiers::{Escape, identifier_of, name_of, name_of_parts};
pub use parse::{Syntax, Whitespace, parse, parse_bytes};
pub(crate) use parse::{normalized, predefined};
pub use publish::{comment, concatenate, processing_instruction, text};
pub use schema::{BuiltInList, Facet, FacetKind, FacetSpec, Facets, Held, MAX_TYPE_NESTING, Pattern, SimpleType, Variety, applicable};
pub use sequence::{Atomic, AtomicType, Item, Node, Sequence};
pub use serialize::{Parameters, serialize};
pub use stored::{Event, Events, FORMAT_VERSION, decode, encode};
pub use tree::{
    Binding, Builder, Copying, ElementStart, Kind, NodeId, QName, SQLXML_NAMESPACE, SQLXML_PREFIX, Specified, Tree, XML_NAMESPACE,
    XML_PREFIX, XMLNS_NAMESPACE, XS_NAMESPACE, XS_PREFIX, XSI_NAMESPACE, XSI_PREFIX, copied_attribute, is_xml_id,
};

use crate::error::{Code, SqlError};

/// \[DESIGN\] How deep elements nest at most in a tree, so a walk of one
/// that keeps a stack of its ancestors keeps a bounded one (RULES.md
/// R-CF-6, D86). [XML 1.0] sets no limit, so a text nested deeper is
/// SQLCODE -900 (D18). The value is no measurement: it is a bound past
/// the nesting of the documents and messages an application stores.
pub const MAX_NESTING: usize = 512;

/// \[DESIGN\] How many octets of text a tree holds at most, its names, its
/// namespace bindings and the characters of its text, comment, processing
/// instruction and attribute nodes, and how many characters entities'
/// replacement texts add to a parse, so a value is bounded in memory and
/// entities that expand to more are refused rather than read for ever
/// (RULES.md D18, D86). It is the bound geometry's readers have,
/// `geometry::MAX_BYTES` (D85).
pub const MAX_TEXT: usize = 16 * 1024 * 1024;

/// \[DESIGN\] How many nodes and namespace bindings a tree has at most, so
/// a text of little but markup, which `MAX_TEXT` does not bound, makes a
/// tree of bounded size (RULES.md D18, D86). The value is no measurement:
/// it is the nodes of the largest documents an application stores, each
/// element a few nodes.
pub const MAX_NODES: usize = 1 << 20;

/// \[DESIGN\] How many octets of text XMLPARSE reads at most, so a string
/// of a large object is read whole within a bound (RULES.md D18, D86); a
/// text within it makes a tree within `MAX_TEXT` and `MAX_NODES`, or is
/// refused by them.
pub const MAX_INPUT: usize = 64 * 1024 * 1024;

/// \[DESIGN\] How many octets a value's stored form has at most (RULES.md
/// D86). A value whose form is longer is refused as it is stored, SQLCODE
/// -900, so every value stored is read back whole, and stored bytes longer
/// are corrupt. One tree within `MAX_TEXT` and `MAX_NODES` stores in less.
pub const MAX_STORED: usize = 64 * 1024 * 1024;

/// The error of an XML value beyond turborel's limits, `why` (RULES.md
/// D18).
fn beyond(why: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[DESIGN] the XML value is beyond turborel's limits: {why}"))
}

#[cfg(test)]
mod tests;
