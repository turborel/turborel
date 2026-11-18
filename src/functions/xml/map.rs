//! The mappings of SQL to XML and XML Schema [ISO/IEC 9075-14:2006 draft
//! §9.4 to §9.17]: SQL data types and domains, each to its XML Name and
//! to the XML Schema data type an XML Schema document defines of it
//! (`map/types.rs`, `map/durations.rs`); and \[EXTENSION\] XMLMAP's
//! functions (RULES.md D86), a table, a schema or the catalog mapped to an
//! XML Schema document and to XML [§9.11, §9.14, §9.17]: each row written
//! by a function of its values, the data of all the rows by a set
//! function (`map/rows.rs`, `map/data.rs`), and the XML Schema document by
//! the function of the data (`map/documents.rs`), each parsed as the
//! draft's XMLPARSE parses them.

mod composites;
mod data;
mod documents;
mod durations;
mod rows;
mod types;

pub use data::{DataShape, DataWriter};
pub use rows::{RowShape, XmlMapRow, XmlMapRows, rows_type};
pub use types::{Declared, XDT_NAMESPACE, XDT_PREFIX, declared_name, definitions, mappable, named_type, type_name};

use super::super::{BinaryEncoding, ScalarFunction};
use super::{Input, input, too_long};
use crate::error::{Code, Result, SqlError};
use crate::value::xml::{Typing, XmlType, XmlValue};
use crate::value::{DataType, Value};
use crate::xml::{Item, MAX_INPUT, Node, Sequence, Syntax, Whitespace, parse};

/// \[EXTENSION\] XMLMAP's name, of its function and of the set function
/// of its tables' rows.
pub const XMLMAP: &str = "XMLMAP";

/// [§9.11 GR9] XMLDECL of a document of XML 1.1, of no encoding, UTF-8
/// being the encoding \[GR8\]: implementation-defined.
const DECLARATION_11: &str = "<?xml version='1.1'?>";
/// The text declaration of content of XML 1.1, which names its encoding
/// [XML 1.1 §4.3.1 \[77\]], so a parse of CONTENT reads it as XML 1.1.
const TEXT_DECLARATION_11: &str = "<?xml version='1.1' encoding='UTF-8'?>";

/// [§9.10 GR1] NULLS: whether a null value is mapped to an absent element
/// or to one nilled, `xsi:nil="true"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nulls {
    /// absent.
    Absent,
    /// nil.
    Nil,
}

/// The parameters of a mapping [§9.11 SR1, §9.14 SR1, §9.17 SR1].
#[derive(Debug, Clone)]
pub struct Choices {
    /// NULLS.
    pub nulls: Nulls,
    /// TABLEFOREST: a table mapped to a sequence of elements (true), or to
    /// one element (false).
    pub forest: bool,
    /// TARGETNS, the zero-length string for no namespace.
    pub target: String,
    /// DATA: whether the mapping gives the data.
    pub data: bool,
    /// METADATA: whether it gives the XML Schema document.
    pub metadata: bool,
    /// ENCODING.
    pub encoding: BinaryEncoding,
    /// The SQL-session's time zone displacement, in minutes, in which a
    /// datetime without time zone is written [§9.8 GR7].
    pub zone: i16,
}

/// An XML visible column [§4.10.8] a mapping writes.
#[derive(Debug, Clone)]
pub struct Column {
    /// Its `<column name>`.
    pub name: String,
    /// What it is declared of [§9.7].
    pub declared: Declared,
    /// Whether it is not known not nullable [§9.9 GR5e].
    pub nullable: bool,
    /// The XML Name of what it is declared of [§9.9 GR5d, §9.4].
    pub type_name: String,
}

/// An XML visible table [§4.10.8] a mapping writes.
#[derive(Debug, Clone)]
pub struct Table {
    /// The `<unqualified schema name>` of its schema.
    pub schema: String,
    /// Its `<qualified identifier>`.
    pub name: String,
    /// Its XML visible columns, in order of their ordinal positions [§9.9
    /// GR5a].
    pub columns: Vec<Column>,
}

/// What a mapping maps.
#[derive(Debug, Clone)]
pub enum Source {
    /// A table [§9.11].
    Table(Table),
    /// A schema, of its name, and its XML visible tables in their
    /// repeatable order [§9.14].
    Schema(String, Vec<Table>),
    /// The catalog, of each XML visible schema and its XML visible tables,
    /// in their repeatable order [§9.17].
    Catalog(Vec<(String, Vec<Table>)>),
}

/// \[EXTENSION\] XMLMAP's function (RULES.md D86): of the data the set
/// function of its tables' rows wrote, if DATA and a table is read, else
/// of nothing, the mapping's XML Schema document, its data, or both.
#[derive(Debug)]
pub struct XmlMap {
    /// The `<catalog name>` of the catalog [§9.11 GR1].
    pub catalog: String,
    /// What is mapped.
    pub source: Source,
    /// The mapping's parameters.
    pub choices: Choices,
}

impl XmlMap {
    /// The tables it maps, in order.
    fn tables(&self) -> Vec<&Table> {
        match &self.source {
            Source::Table(t) => vec![t],
            Source::Schema(_, tables) => tables.iter().collect(),
            Source::Catalog(schemas) => schemas.iter().flat_map(|(_, tables)| tables).collect(),
        }
    }

    /// Whether its data is content, not a document: of a table mapped as a
    /// forest [§9.11 GR12d i].
    fn content(&self) -> bool {
        self.choices.forest && matches!(self.source, Source::Table(_))
    }

    /// The type of its value: of METADATA the document's; of DATA the
    /// data's, a table's as a forest content, else a document; of both
    /// a sequence of two document nodes.
    #[must_use]
    pub fn result_type(&self) -> DataType {
        let t = if self.choices.data && self.choices.metadata {
            XmlType::Sequence
        } else if self.choices.data && self.content() {
            XmlType::Content(Typing::Untyped)
        } else {
            XmlType::Document(Typing::Untyped)
        };
        DataType::Xml(t)
    }
}

/// The document node of `text` parsed as `syntax` says, white space kept,
/// as the draft's XMLPARSE parses XS and XD [§9.11 GR10f, GR12d].
fn parsed(text: &str, syntax: Syntax) -> Result<Item> {
    Ok(Item::Node(Node::root(parse(text, syntax, Whitespace::Preserve)?)))
}

impl ScalarFunction for XmlMap {
    fn name(&self) -> &str {
        XMLMAP
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.result_type())
    }

    /// [§9.11 GR10 to GR13, §9.14 GR11 to GR14, §9.17 GR11 to GR14] XS
    /// parsed as a document, if METADATA; then XD, the text the set
    /// function of its rows wrote `args`, or, of no table read, that of no
    /// row, parsed as a document, or as content of a table mapped as a
    /// forest, if DATA: each declared of XML 1.1 if a name XS writes is
    /// \[GR7a\], else \[GR7b\] implementation-defined, of XML 1.0. Past
    /// `MAX_INPUT` octets, SQLCODE -900.
    fn call(&self, args: &[Value]) -> Result<Value> {
        let schema = self.schema_document()?;
        if schema.out.len() > MAX_INPUT {
            return Err(too_long());
        }
        let declaration = if schema.v11 { DECLARATION_11 } else { "" };
        let mut items = Vec::with_capacity(2);
        if self.choices.metadata {
            items.push(parsed(&format!("{declaration}{}", schema.out), Syntax::Document)?);
        }
        if self.choices.data {
            let data = match args.first().map(input).transpose()?.flatten() {
                Some(Input::Text(text)) => text,
                Some(Input::Octets(_)) => return Err(SqlError::new(Code::Api, "[DESIGN] XMLMAP was given a binary string, not its data")),
                None => {
                    let shape = DataShape::of(&self.source, &self.choices, &self.catalog);
                    DataWriter::new(&shape)?.finish(&shape)?
                }
            };
            items.push(if self.content() {
                let text_declaration = if schema.v11 { TEXT_DECLARATION_11 } else { "" };
                parsed(&format!("{text_declaration}{data}"), Syntax::Content)?
            } else {
                parsed(&format!("{declaration}{data}"), Syntax::Document)?
            });
        }
        Ok(Value::Xml(XmlValue::of(Sequence::new(items))))
    }
}

#[cfg(test)]
mod tests;
