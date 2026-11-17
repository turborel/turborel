//! XD, the XML text of a mapping's data [ISO/IEC 9075-14:2006 draft
//! §9.11 GR12, §9.14 GR13, §9.17 GR13]: each table's rows [§9.10], in
//! the element of the table, mapped as an element [§9.10 GR7], of its
//! schema [§9.13] and of the catalog [§9.16]. Its root is the element
//! XDROWS, XDSCHEMA or XDCATALOG is, declaring `xmlns:xsi` (see
//! `map/documents.rs`). The text is at most `MAX_INPUT` octets, the most
//! XMLPARSE reads; past it, SQLCODE -900.

use std::fmt::Write;

use super::super::{Input, input, too_long};
use super::documents::{attribute, xml_name};
use super::{Source, Table, XmlMap};
use crate::error::{Code, Result, SqlError};
use crate::value::Value;
use crate::xml::{MAX_INPUT, XSI_NAMESPACE};

/// The text of a table's rows, as the set function of them gives it
/// (`map/rows.rs`); of the null value, of no rows, none.
fn rows_text(v: Option<&Value>) -> Result<String> {
    match input(v.unwrap_or(&Value::Null))? {
        None => Ok(String::new()),
        Some(Input::Text(text)) => Ok(text),
        Some(Input::Octets(_)) => Err(SqlError::new(Code::Api, "[DESIGN] XMLMAP was given a binary string, not a table's rows")),
    }
}

/// `text` appended to `out`, which is then at most `MAX_INPUT` octets.
///
/// # Errors
///
/// SQLCODE -900 if it is longer.
fn append(out: &mut String, text: &str) -> Result<()> {
    if out.len().saturating_add(text.len()) > MAX_INPUT {
        return Err(too_long());
    }
    out.push_str(text);
    Ok(())
}

impl XmlMap {
    /// XD's text, of `rows`, the text of each table's rows in order.
    pub(super) fn data_document(&self, rows: &[Value]) -> Result<String> {
        let mut rows = rows.iter();
        let mut out = String::new();
        match &self.source {
            Source::Table(t) => {
                // [§9.11 GR12c iii] Of a forest, XDROWS alone.
                if self.choices.forest {
                    append(&mut out, &rows_text(rows.next())?)?;
                    return Ok(out);
                }
                let name = xml_name(&t.name);
                self.open_root(&mut out, &name);
                append(&mut out, &rows_text(rows.next())?)?;
                append(&mut out, &format!("</{name}>"))?;
            }
            Source::Schema(s, tables) => {
                let name = xml_name(s);
                self.open_root(&mut out, &name);
                self.tables_text(&mut out, tables, &mut rows)?;
                append(&mut out, &format!("</{name}>"))?;
            }
            Source::Catalog(schemas) => {
                let name = xml_name(&self.catalog);
                self.open_root(&mut out, &name);
                for (s, tables) in schemas {
                    let schema = xml_name(s);
                    append(&mut out, &format!("<{schema}>"))?;
                    self.tables_text(&mut out, tables, &mut rows)?;
                    append(&mut out, &format!("</{schema}>"))?;
                }
                append(&mut out, &format!("</{name}>"))?;
            }
        }
        Ok(out)
    }

    /// The start tag of XD's root, of the element `name` [§9.10 GR7, §9.13
    /// GR6, §9.16 GR6]: declaring `xsi`'s namespace [§9.11 GR12c, §9.14
    /// GR13c, §9.17 GR13c], and the target namespace the default one.
    fn open_root(&self, out: &mut String, name: &str) {
        // Writing to a String cannot fail.
        let _ = write!(out, r#"<{name} xmlns:xsi="{XSI_NAMESPACE}""#);
        if !self.choices.target.is_empty() {
            let _ = write!(out, r#" xmlns="{}""#, attribute(&self.choices.target));
        }
        out.push('>');
    }

    /// [§9.13 GR5b] The elements of `tables` of a schema, each its rows'
    /// text from `rows`, mapped by §9.10 with TARGETNS the zero-length
    /// string: mapped as an element, in an element of the table's name
    /// [§9.10 GR7a]; as a forest, alone [GR7c].
    fn tables_text<'a>(&self, out: &mut String, tables: &[Table], rows: &mut impl Iterator<Item = &'a Value>) -> Result<()> {
        for t in tables {
            let text = rows_text(rows.next())?;
            if self.choices.forest {
                append(out, &text)?;
                continue;
            }
            let name = xml_name(&t.name);
            append(out, &format!("<{name}>"))?;
            append(out, &text)?;
            append(out, &format!("</{name}>"))?;
        }
        Ok(())
    }
}
