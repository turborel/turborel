//! XD, the XML text of a mapping's data [ISO/IEC 9075-14:2006 draft
//! §9.11 GR12, §9.14 GR13, §9.17 GR13], written as the rows of its tables
//! come: each table's rows [§9.10], in the element of the table, mapped as
//! an element [§9.10 GR7], of its schema [§9.13] and of the catalog
//! [§9.16]. Its root is the element XDROWS, XDSCHEMA or XDCATALOG is,
//! declaring `xmlns:xsi` (see `map/documents.rs`).
//!
//! The text is held as it grows, the whole mapping's, at most `MAX_INPUT`
//! octets, the most XMLPARSE reads: past it, SQLCODE -900, as soon as a
//! row takes it there, so no mapping of many tables holds more (RULES.md
//! D18, D86).

use std::fmt::Write;

use super::super::too_long;
use super::documents::{attribute, xml_name};
use super::{Choices, Source};
use crate::error::{Code, Result, SqlError};
use crate::xml::{MAX_INPUT, XSI_NAMESPACE};

/// An element's start tag and end tag, or two zero-length strings of no
/// element.
type Tags = (String, String);

/// Where each table's rows go in a mapping's data: the root's tags; of
/// each table, in order, the schema it is of among `schemas`, if the data
/// has an element of each schema, and the tags of its element, if it has
/// one; the tags of each schema's element; the most octets of its text.
#[derive(Debug)]
pub struct DataShape {
    root: Tags,
    tables: Vec<(Option<usize>, Tags)>,
    schemas: Vec<Tags>,
    limit: usize,
}

/// The tags of the element `name`, with `attributes` in its start tag.
fn tags(name: &str, attributes: &str) -> Tags {
    (format!("<{name}{attributes}>"), format!("</{name}>"))
}

/// No element's tags.
fn none() -> Tags {
    (String::new(), String::new())
}

impl DataShape {
    /// The data of `source` mapped by `choices`, of the catalog
    /// `catalog`: its root declaring `xsi`'s namespace [§9.11 GR12c,
    /// §9.14 GR13c, §9.17 GR13c], and the target namespace the default
    /// one; none of a table mapped as a forest [§9.11 GR12c iii]. A table's
    /// element of a schema's mapping is that §9.10 GR7a makes, with
    /// TARGETNS the zero-length string [§9.13 GR5b]; as a forest, none.
    #[must_use]
    pub fn of(source: &Source, choices: &Choices, catalog: &str) -> DataShape {
        let mut attributes = format!(r#" xmlns:xsi="{XSI_NAMESPACE}""#);
        if !choices.target.is_empty() {
            // Writing to a String cannot fail.
            let _ = write!(attributes, r#" xmlns="{}""#, attribute(&choices.target));
        }
        let element = |name: &str| if choices.forest { none() } else { tags(&xml_name(name), "") };
        let (root, tables, schemas) = match source {
            Source::Table(t) => {
                let root = if choices.forest { none() } else { tags(&xml_name(&t.name), &attributes) };
                (root, vec![(None, none())], Vec::new())
            }
            Source::Schema(s, tables) => {
                (tags(&xml_name(s), &attributes), tables.iter().map(|t| (None, element(&t.name))).collect(), Vec::new())
            }
            Source::Catalog(schemas) => {
                let tables =
                    schemas.iter().enumerate().flat_map(|(i, (_, tables))| tables.iter().map(move |t| (Some(i), element(&t.name))));
                let elements = schemas.iter().map(|(s, _)| tags(&xml_name(s), "")).collect();
                (tags(&xml_name(catalog), &attributes), tables.collect(), elements)
            }
        };
        DataShape { root, tables, schemas, limit: MAX_INPUT }
    }

    /// Itself, its text at most `limit` octets, so a test can reach it.
    #[must_use]
    pub fn limited(self, limit: usize) -> DataShape {
        DataShape { limit, ..self }
    }
}

/// A mapping's data as it is written: its text, how many tables' elements
/// are begun, and the schema whose element is open, if any.
#[derive(Debug, Clone)]
pub struct DataWriter {
    text: String,
    begun: usize,
    schema: Option<usize>,
}

impl DataWriter {
    /// The data of `shape` begun: its root's start tag.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 past `shape`'s limit.
    pub fn new(shape: &DataShape) -> Result<DataWriter> {
        let mut writer = DataWriter { text: String::new(), begun: 0, schema: None };
        writer.append(shape, &shape.root.0)?;
        Ok(writer)
    }

    /// `text` appended, the whole then at most `shape`'s limit.
    fn append(&mut self, shape: &DataShape, text: &str) -> Result<()> {
        if self.text.len().saturating_add(text.len()) > shape.limit {
            return Err(too_long());
        }
        self.text.push_str(text);
        Ok(())
    }

    /// The next table's element begun, after the one before it is ended,
    /// in its schema's element.
    fn begin_next(&mut self, shape: &DataShape) -> Result<()> {
        self.end_last(shape)?;
        let (schema, (start, _)) = shape.tables.get(self.begun).ok_or_else(|| broken("a table it does not map"))?;
        if *schema != self.schema {
            self.end_schema(shape)?;
            if let Some((open, _)) = schema.and_then(|s| shape.schemas.get(s)) {
                self.append(shape, open)?;
            }
            self.schema = *schema;
        }
        self.append(shape, start)?;
        self.begun = self.begun.saturating_add(1);
        Ok(())
    }

    /// The element of the last table begun ended, if one is.
    fn end_last(&mut self, shape: &DataShape) -> Result<()> {
        let Some((_, (_, end))) = self.begun.checked_sub(1).and_then(|last| shape.tables.get(last)) else { return Ok(()) };
        self.append(shape, end)
    }

    /// The element of the schema open ended, if one is.
    fn end_schema(&mut self, shape: &DataShape) -> Result<()> {
        let Some((_, end)) = self.schema.and_then(|s| shape.schemas.get(s)) else { return Ok(()) };
        self.append(shape, end)
    }

    /// [§9.10 GR6] A row's text `row` of the table at `index` among
    /// `shape`'s, the tables before it begun, and ended, first. Rows come
    /// in the order of their tables, as the plan reads them one table
    /// after another.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 past `shape`'s limit; -916 of a row of a table before
    /// the last begun, which the plan never gives.
    pub fn row(&mut self, shape: &DataShape, index: usize, row: &str) -> Result<()> {
        if index.saturating_add(1) < self.begun {
            return Err(broken("a row of a table after those of the next"));
        }
        while self.begun <= index {
            self.begin_next(shape)?;
        }
        self.append(shape, row)
    }

    /// The whole text: each table not begun begun, the last ended, then
    /// its schema's element and the root ended.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 past `shape`'s limit.
    pub fn finish(mut self, shape: &DataShape) -> Result<String> {
        while self.begun < shape.tables.len() {
            self.begin_next(shape)?;
        }
        self.end_last(shape)?;
        self.end_schema(shape)?;
        self.append(shape, &shape.root.1)?;
        Ok(self.text)
    }
}

/// The error of rows not as the plan gives them.
fn broken(what: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] XMLMAP was given {what}"))
}
