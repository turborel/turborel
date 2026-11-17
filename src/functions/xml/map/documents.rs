//! The XML text of a mapping's two documents [ISO/IEC 9075-14:2006 draft
//! §9.11, §9.14, §9.17]: XS, the XML Schema document, of the types of the
//! mapped columns [§9.7] and of the tables [§9.9], schemas [§9.12] and
//! catalog [§9.15]; and XD, the data, of the tables' rows [§9.10] in the
//! elements of their schemas [§9.13] and catalog [§9.16].
//!
//! Readings of the draft, each so a document is what its rules mean it
//! to be, an XML Schema document valid of the data:
//!
//! * XD's root [§9.11 GR12c, §9.14 GR13c, §9.17 GR13c] is the element
//!   XDROWS, XDSCHEMA or XDCATALOG already is, with `xmlns:xsi` declared,
//!   not another of its name around it, which no element the schema
//!   declares would hold; and \[§9.11 GR12c ii\] of a target namespace its
//!   default namespace, as §9.10 GR7b writes it, not `xmlns:xs`.
//! * XSLA \[§9.11 GR11a\] is the zero-length string: XSR is no resource a
//!   URI identifies, the value holding it beside the data.
//! * Of a target namespace, the schema document declares it its default
//!   namespace too, so the names of the types it defines that it writes
//!   unprefixed resolve to them [Schema1 §3.15.3, QName resolution
//!   (Schema Document)]. Of an interval type it declares and imports
//!   `xdt`'s namespace, as it would `sqlxml`'s [§9.11 GR10c, GR10d].
//! * The schema documents of a schema and of the catalog define each
//!   table's row type and table type [§9.9], and the catalog's each
//!   schema's type [§9.12], which their types name [§9.12 GR5f, §9.15
//!   GR5e]; not the elements §9.9 GR10b declares, of tables of one name in
//!   two schemas.
//!
//! \[§9.11 GR10c\] implementation-defined: SLOC is the zero-length string;
//! no type uses `sqlxml` (`map/types.rs`), so XSDIMP and XSDECL are too.
//! \[§9.9 GR5f i 7, GR9, §9.12 GR8, §9.15 GR7\] no annotation is written.

use super::types::{Declared, XDT_NAMESPACE, XDT_PREFIX, declared_name, definitions};
use super::{Column, Source, Table, XmlMap};
use crate::xml::{Escape, Version, XS_NAMESPACE, is_ncname, name_of, name_of_parts};

/// [§9.10 GR6c i] The element of a row of a table mapped as an element.
pub const ROW: &str = "row";
/// [§9.9 GR6] The first part of the name [§9.2] of a table's type.
const TABLE_TYPE: &str = "TableType";
/// [§9.9 GR7] A row's type.
const ROW_TYPE: &str = "RowType";
/// [§9.12 GR6] A schema's type.
const SCHEMA_TYPE: &str = "SchemaType";
/// [§9.15 GR6] The catalog's type.
const CATALOG_TYPE: &str = "CatalogType";

/// A document's text as it is written, and whether a name in it is an XML
/// 1.1 name no XML 1.0 name is [§9.11 GR7a].
#[derive(Default)]
pub(super) struct Text {
    /// The text.
    pub(super) out: String,
    /// Whether a name of it is of XML 1.1 alone.
    pub(super) v11: bool,
}

impl Text {
    /// Writes `s`, markup.
    fn push(&mut self, s: &str) {
        self.out.push_str(s);
    }

    /// Writes `name`, an XML Name, noting whether it is of XML 1.0.
    fn name(&mut self, name: &str) {
        if !is_ncname(Version::V10, name) {
            self.v11 = true;
        }
        self.out.push_str(name);
    }

    /// Writes `<xs:element name="NAME" type="TYPE"` then `rest`.
    fn element(&mut self, name: &str, type_name: &str, rest: &str) {
        self.push(r#"<xs:element name=""#);
        self.name(name);
        self.push(r#"" type=""#);
        self.name(type_name);
        self.push(&format!(r#""{rest}/>"#));
    }
}

/// `value` as an attribute's value between double quotes: `&`, `<` and
/// `"` written as references [XML §2.3 \[10\]].
pub(super) fn attribute(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;")
}

/// [§9.1] The XML Name of an SQL identifier, fully escaped.
pub(super) fn xml_name(identifier: &str) -> String {
    name_of(identifier, Escape::Full)
}

impl XmlMap {
    /// [§9.11 GR10e, §9.14 GR11e, §9.17 GR11e] XS: the XML Schema
    /// document, XSCT the types of the columns [§9.7], then XST.
    pub(super) fn schema_document(&self) -> Text {
        let mut text = Text::default();
        let declared: Vec<Declared> = self.tables().into_iter().flat_map(|t| t.columns.iter().map(|c| c.declared.clone())).collect();
        let (xsct, xdt) = definitions(&declared, &self.catalog, self.choices.encoding);
        for d in &declared {
            if let Some(name) = declared_name(d, &self.catalog) {
                text.v11 |= !is_ncname(Version::V10, &name);
            }
        }
        text.push(&format!(r#"<xs:schema xmlns:xs="{XS_NAMESPACE}""#));
        if xdt {
            text.push(&format!(r#" xmlns:{XDT_PREFIX}="{XDT_NAMESPACE}""#));
        }
        let target = attribute(&self.choices.target);
        if !target.is_empty() {
            text.push(&format!(r#" xmlns="{target}" targetNamespace="{target}" elementFormDefault="qualified""#));
        }
        text.push(">");
        if xdt {
            text.push(&format!(r#"<xs:import namespace="{XDT_NAMESPACE}"/>"#));
        }
        text.push(&xsct);
        self.structure(&mut text);
        text.push("</xs:schema>");
        text
    }

    /// XST, and the element of the document's root [§9.11 GR10e, §9.14
    /// GR11e, §9.17 GR11e].
    fn structure(&self, text: &mut Text) {
        let forest = self.choices.forest;
        match &self.source {
            Source::Table(t) => {
                self.table_types(text, t);
                let n = xml_name(&t.name);
                if forest {
                    text.element(&n, &self.row_type(t), "");
                } else {
                    text.element(&n, &self.table_type(t), "");
                }
            }
            Source::Schema(s, tables) => {
                self.schema_types(text, s, tables);
                text.element(&xml_name(s), &name_of_parts(&[SCHEMA_TYPE, &self.catalog, s]), "");
            }
            Source::Catalog(schemas) => {
                for (s, tables) in schemas {
                    self.schema_types(text, s, tables);
                }
                self.catalog_type(text, schemas);
                text.element(&xml_name(&self.catalog), &name_of_parts(&[CATALOG_TYPE, &self.catalog]), "");
            }
        }
    }

    /// [§9.9 GR7] XMLROWN, the name of `t`'s row type.
    fn row_type(&self, t: &Table) -> String {
        name_of_parts(&[ROW_TYPE, &self.catalog, &t.schema, &t.name])
    }

    /// [§9.9 GR6] XMLTYPEN, the name of `t`'s table type.
    fn table_type(&self, t: &Table) -> String {
        name_of_parts(&[TABLE_TYPE, &self.catalog, &t.schema, &t.name])
    }

    /// [§9.9 GR10] `t`'s row type, of an element of each XML visible
    /// column \[GR5g\], and, mapped as an element, its table type of a
    /// sequence of rows.
    fn table_types(&self, text: &mut Text, t: &Table) {
        text.push(r#"<xs:complexType name=""#);
        text.name(&self.row_type(t));
        text.push(r#""><xs:sequence>"#);
        for c in &t.columns {
            self.column_element(text, c);
        }
        text.push("</xs:sequence></xs:complexType>");
        if !self.choices.forest {
            text.push(r#"<xs:complexType name=""#);
            text.name(&self.table_type(t));
            text.push(r#""><xs:sequence>"#);
            text.element(ROW, &self.row_type(t), r#" minOccurs="0" maxOccurs="unbounded""#);
            text.push("</xs:sequence></xs:complexType>");
        }
    }

    /// [§9.9 GR5g] XMLCEi, the element of column `c`: named as it is
    /// \[GR5c\], of its type's \[GR5d\], left out \[GR5e ii 1\] or nilled
    /// \[GR5e ii 2\] of a null value unless known not nullable \[GR5e i\].
    fn column_element(&self, text: &mut Text, c: &Column) {
        let nulls = match (c.nullable, self.choices.nulls) {
            (false, _) => "",
            (true, super::Nulls::Absent) => r#" minOccurs="0""#,
            (true, super::Nulls::Nil) => r#" nillable="true""#,
        };
        let type_name = declared_name(&c.declared, &self.catalog).unwrap_or_default();
        text.element(&xml_name(&c.name), &type_name, nulls);
    }

    /// [§9.12] A schema's type, of the element of each XML visible table
    /// \[GR5f\], all of them, or, mapped as forests, a sequence of each's
    /// rows \[GR9\]; after each table's types [§9.9].
    fn schema_types(&self, text: &mut Text, schema: &str, tables: &[Table]) {
        for t in tables {
            self.table_types(text, t);
        }
        let group = if self.choices.forest { "sequence" } else { "all" };
        text.push(r#"<xs:complexType name=""#);
        text.name(&name_of_parts(&[SCHEMA_TYPE, &self.catalog, schema]));
        text.push(&format!(r#""><xs:{group}>"#));
        for t in tables {
            if self.choices.forest {
                text.element(&xml_name(&t.name), &self.row_type(t), r#" minOccurs="0" maxOccurs="unbounded""#);
            } else {
                text.element(&xml_name(&t.name), &self.table_type(t), "");
            }
        }
        text.push(&format!("</xs:{group}></xs:complexType>"));
    }

    /// [§9.15 GR8] The catalog's type, of the element of each XML visible
    /// schema \[GR5e\].
    fn catalog_type(&self, text: &mut Text, schemas: &[(String, Vec<Table>)]) {
        text.push(r#"<xs:complexType name=""#);
        text.name(&name_of_parts(&[CATALOG_TYPE, &self.catalog]));
        text.push(r#""><xs:all>"#);
        for (s, _) in schemas {
            text.element(&xml_name(s), &name_of_parts(&[SCHEMA_TYPE, &self.catalog, s]), "");
        }
        text.push("</xs:all></xs:complexType>");
    }
}
