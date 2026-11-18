//! The computed document constructor of XQuery, `document { $EXPR }`
//! [XQuery §3.7.3.3], as XMLDOCUMENT evaluates it [ISO/IEC 9075-14:2006
//! draft §6.13 GR8].
//!
//! \[DESIGN\] RULES.md D86: the content sequence is built as it is read,
//! into a tree of its own (`Builder`), each node copied by the tree's
//! links, so nothing recurses.

use super::sequence::{Item, Sequence};
use super::tree::{Builder, Copying, Kind};
use crate::error::{Code, Result, SqlError, SqlState};

/// The one separator XQuery puts between adjacent atomic values made a
/// text node [XQuery §3.7.1.3].
const SEPARATOR: char = ' ';

/// The error of `document { $EXPR }` of a sequence holding an attribute
/// [XQuery §3.7.3.3, err:XPTY0004].
fn attribute_in_content() -> SqlError {
    SqlError::new(
        Code::Data,
        "[ISO/IEC 9075-14:2006 draft §6.13 GR8] XQuery error: a document node's content holds an attribute [XQuery §3.7.3.3] err:XPTY0004",
    )
    .with_state(SqlState::XQUERY_ERROR)
}

/// The document node `document { $EXPR }` makes of `value`, the value of
/// $EXPR [XQuery §3.7.3.3]: each run of adjacent atomic values a text
/// node of their lexical forms a space apart, which casting each to
/// `xs:string` gives [XQueryFO §17.1.2]; each node copied, a document node
/// replaced by its children; adjacent text one text node, and no text node
/// empty [XQuery §3.7.1.3] [XQueryDM §6.7.3].
///
/// # Errors
///
/// SQLSTATE 10000, XQuery error, if `value` holds an attribute
/// \[err:XPTY0004\]; SQLCODE -900 if the tree would be beyond turborel's
/// limits.
pub fn document(value: &Sequence) -> Result<Sequence> {
    document_as(value, Copying::PRESERVE)
}

/// As `document`, each node copied by `mode`: `Copying::UNVALIDATED` makes
/// an unvalidated document node, as XMLDOCUMENT of XML(CONTENT(UNTYPED))
/// and CAST BY VALUE to an UNTYPED type do [ISO/IEC 9075-14:2006 draft
/// §10.19, §6.13 GR9a, §6.5 GR1b i 3].
///
/// # Errors
///
/// As `document`.
pub fn document_as(value: &Sequence, mode: Copying) -> Result<Sequence> {
    let mut builder = Builder::document();
    let mut after_atomic = false;
    for item in value.items() {
        match item {
            Item::Atomic(atomic) => {
                if after_atomic {
                    builder.text(&SEPARATOR.to_string())?;
                }
                builder.text(&atomic.lexical)?;
                after_atomic = true;
            }
            Item::Node(node) => {
                if node.kind() == Some(Kind::Attribute) {
                    return Err(attribute_in_content());
                }
                builder.copy_as(node.tree(), node.id(), mode)?;
                after_atomic = false;
            }
        }
    }
    Ok(Sequence::document(builder.finish()?))
}

#[cfg(test)]
mod tests;
