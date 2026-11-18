//! Schema documents read into a schema [Schema1 §3.x.2, §4.2]: the
//! documents gathered by `<include>`, `<redefine>` and `<import>`
//! (`read/documents.rs`), their top-level definitions and declarations
//! indexed by name (`read/index.rs`), then each component built as it is
//! first needed (`read/build.rs`), so a reference may come before what it
//! names [Schema1 §3.1.3], and every one built in the end. The components
//! are checked once all are built (`schema/check.rs`).
//!
//! \[DESIGN\] Implementation-defined choices: a schema document is a
//! `<schema>` element that is the document element of its tree; a
//! reference that resolves to no component is an error as the schema is
//! read, rather than a missing sub-component found as an item is
//! validated [Schema1 §5.3], so a schema read is complete; components of
//! the XML Schema namespace are its built-in ones alone, and may be named
//! without an `<import>` of it [Schema1 §3.14.7, §3.15.3 clause 4].

mod attrs;
mod build;
mod complex;
mod documents;
mod element;
mod identity;
mod index;
mod particle;
mod shape;

use std::sync::Arc;

use super::component::Schema;
use crate::error::Result;
use crate::xml::Tree;

/// A schema document and its schema location, as a schema is read with
/// it [Schema1 §4.2.1, §4.3.2].
#[derive(Debug, Clone)]
pub struct Source {
    /// Its schema location URI, the name an `<include>`, `<redefine>` or
    /// `<import>` finds it by; none if it has none.
    pub location: Option<Box<str>>,
    /// Its tree: a document node whose element is a `<schema>`.
    pub tree: Arc<Tree>,
}

/// The schema documents a schema being read may include, redefine or
/// import [Schema1 §4.2] (`Source`): \[DESIGN\] those of the registered
/// XML Schemas, so no resource outside the database is read [Schema1
/// §4.3.2 clause 2, 3].
#[derive(Debug, Clone, Default)]
pub struct Library {
    sources: Vec<Source>,
}

impl Library {
    /// The library of `sources`.
    #[must_use]
    pub fn new(sources: Vec<Source>) -> Library {
        Library { sources }
    }

    /// The document whose schema location is `location` [Schema1 §4.3.2
    /// clause 2], the first of them if several are.
    fn at(&self, location: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.location.as_deref() == Some(location))
    }

    /// The documents of the library, in order.
    fn sources(&self) -> &[Source] {
        &self.sources
    }
}

/// The schema `primary` corresponds to [Schema1 §3.15.2], with the
/// components of what it includes, redefines and imports from `library`
/// [Schema1 §4.2], checked against every schema component and schema
/// representation constraint [Schema1 §5.1].
///
/// # Errors
///
/// SQLSTATE 22000 if a document is no schema document or the components
/// are no schema, naming the constraint broken; SQLCODE -900 past
/// `MAX_DEPTH`, `MAX_CHAIN`, `MAX_DOCUMENTS`, `MAX_POSITIONS` or
/// `MAX_STATES`.
pub fn read(primary: &Source, library: &Library) -> Result<Schema> {
    let docs = documents::gather(primary, library)?;
    docs.iter().try_for_each(shape::check)?;
    let index = index::Index::of(&docs)?;
    let schema = build::Builder::new(&docs, &index).build()?;
    super::check::schema(schema)
}
