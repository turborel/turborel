//! The components of the permanently registered XML Schemas [ISO/IEC
//! 9075-14:2006 draft §4.2.6 Table 1], read once from the schema documents
//! kept beside this file (`permanent/`), so validation by them works:
//!
//! - The XML Schema namespace's: the Schema for Schemas as [Schema1
//!   Appendix A] prints it (`XMLSchema.xsd`), with the schema of the
//!   datatypes it includes as [Schema2 Appendix A] prints it
//!   (`datatypes.xsd`), and the schema of the XML namespace it imports
//!   (`xml.xsd`), which none of those texts prints, so turborel's own,
//!   of `xml:lang` and `xml:space` as XML 1.0 defines them [XML 1.0
//!   §2.10, §2.12]. Their type definitions of the built-in names are the
//!   built-in ones they represent [Schema1 §3.4.7, §3.14.7]. \[DESIGN\]
//!   Three of its restrictions, `xs:all`, `xs:complexRestrictionType` and
//!   `xs:simpleRestrictionType`, have content models Particle Valid
//!   (Restriction) refuses by its letter [Schema1 §3.9.6], so its
//!   restrictions' content models are trusted, not checked, as the text
//!   makes the schema normative; those of every registered XML Schema are
//!   checked.
//! - The XML Schema instance namespace's: none of its own, as an attribute
//!   declaration of that namespace is not allowed [Schema1 §3.2.6
//!   xsi: Not Allowed]; its four attribute declarations are in every
//!   schema by definition [Schema1 §3.2.7].
//! - SQL/XML's: the XML Schema [ISO/IEC 9075-14:2006 draft §22.1] prints
//!   (`sqlxml.xsd`).

use std::sync::{Arc, OnceLock};

use super::check::Restrictions;
use super::component::Schema;
use super::read::{Library, Source, read_checking};
use crate::error::Result;
use crate::xml::{SQLXML_NAMESPACE, Syntax, Whitespace, XS_NAMESPACE, XSI_NAMESPACE, parse};

/// The Schema for Schemas [Schema1 Appendix A].
const SCHEMA_FOR_SCHEMAS: &str = include_str!("permanent/XMLSchema.xsd");
/// The schema of the datatypes [Schema2 Appendix A], which the Schema for
/// Schemas includes from this location.
const DATATYPES: (&str, &str) = ("datatypes.xsd", include_str!("permanent/datatypes.xsd"));
/// The schema of the XML namespace, which the Schema for Schemas imports
/// from this location.
const XML: (&str, &str) = ("http://www.w3.org/2001/xml.xsd", include_str!("permanent/xml.xsd"));
/// The SQL/XML XML Schema [ISO/IEC 9075-14:2006 draft §22.1].
const SQL_XML: &str = include_str!("permanent/sqlxml.xsd");
/// A schema document of the XML Schema instance namespace and no
/// component, as [Schema1 §3.2.6] allows none.
const INSTANCE: &str =
    r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="http://www.w3.org/2001/XMLSchema-instance"/>"#;

/// Each permanently registered XML Schema's components, read when first
/// asked for.
static READ: [OnceLock<Option<Arc<Schema>>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];

/// The components of the permanently registered XML Schema of target
/// namespace `namespace`; none of another namespace.
#[must_use]
pub fn components(namespace: &str) -> Option<Arc<Schema>> {
    let (at, reading): (usize, fn() -> Result<Schema>) = match namespace {
        XS_NAMESPACE => (0, schema_for_schemas),
        XSI_NAMESPACE => (1, || read_text(INSTANCE, &Library::default(), Restrictions::Checked)),
        SQLXML_NAMESPACE => (2, || read_text(SQL_XML, &Library::default(), Restrictions::Checked)),
        _ => return None,
    };
    READ.get(at)?.get_or_init(|| reading().ok().map(Arc::new)).clone()
}

/// The Schema for Schemas, with what it includes and imports.
///
/// # Errors
///
/// As `read_text`.
fn schema_for_schemas() -> Result<Schema> {
    let library = Library::new(vec![source(DATATYPES.1, Some(DATATYPES.0))?, source(XML.1, Some(XML.0))?]);
    read_text(SCHEMA_FOR_SCHEMAS, &library, Restrictions::Trusted)
}

/// The schema of the schema document `text`, of `library`, its
/// restrictions checked or trusted as `restrictions` says.
///
/// # Errors
///
/// As `parse` and `read`.
fn read_text(text: &str, library: &Library, restrictions: Restrictions) -> Result<Schema> {
    read_checking(&source(text, None)?, library, restrictions)
}

/// The schema document `text`, at `location`.
///
/// # Errors
///
/// As `parse`.
fn source(text: &str, location: Option<&str>) -> Result<Source> {
    Ok(Source { location: location.map(Into::into), tree: Arc::new(parse(text, Syntax::Document, Whitespace::Preserve)?) })
}

#[cfg(test)]
mod tests;
