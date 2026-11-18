//! SQL row, array and multiset types mapped to XML Names [ISO/IEC
//! 9075-14:2006 draft §9.4 GR19, GR21, GR22] and to the XML Schema
//! complex types an XML Schema document defines of them [§9.5 GR8n,
//! GR8p, GR8q, §9.6 GR6b ii], of no annotation.
//!
//! \[§9.4 GR19\] implementation-dependent: IDI, a row type's identifier,
//! is the multi-part name [§9.2] of each field's name and its type's XML
//! Name in turn, so two row types of other fields, names or types have
//! two, and two alike one.

use std::fmt::Write;

use super::Nulls;
use super::types::named;
use crate::value::DataType;
use crate::value::types::{ArrayType, RowType};
use crate::xml::{Escape, name_of, name_of_parts};

/// [§9.4 GR19] The first part of a row type's XML Name.
const ROW_PART: &str = "Row";
/// [§9.4 GR22] The first part of a multiset type's XML Name.
const MULTISET_PART: &str = "Multiset";
/// [§9.5 GR8p v, GR8q iv] The element of each element of a collection.
const ELEMENT: &str = "element";

/// [§9.4 GR19] Row.IDI, of the row type `r` of the catalog `catalog`,
/// inside `depth` types; none if a field's type has no XML Name.
pub(super) fn row_name(r: &RowType, catalog: &str, depth: usize) -> Option<String> {
    let mut parts = Vec::with_capacity(r.fields.len().saturating_mul(2));
    for f in &r.fields {
        parts.push(f.name.clone());
        parts.push(named(&f.data_type, catalog, depth.saturating_add(1))?);
    }
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    Some(format!("{ROW_PART}.{}", name_of_parts(&parts)))
}

/// [§9.4 GR21] Array_MLIT.XMLET, of the array type `a`.
pub(super) fn array_name(a: &ArrayType, catalog: &str, depth: usize) -> Option<String> {
    Some(format!("Array_{}.{}", a.max, named(&a.element, catalog, depth.saturating_add(1))?))
}

/// [§9.4 GR22] Multiset.XMLET, of the multiset type of `element`.
pub(super) fn multiset_name(element: &DataType, catalog: &str, depth: usize) -> Option<String> {
    Some(format!("{MULTISET_PART}.{}", named(element, catalog, depth.saturating_add(1))?))
}

/// Whether `t` maps to a complex type: a row, array or multiset type, or
/// an XML type.
pub(super) fn complex(t: &DataType) -> bool {
    match t.represented() {
        DataType::Row(_) | DataType::Array(_) | DataType::Multiset(_) | DataType::Xml(_) => true,
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Distinct(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Extension(_) => false,
    }
}

/// [§9.5 GR8n, GR8p, GR8q, §9.6 GR6b ii] The complex type named `name` of
/// `t`, a row, array or multiset type: a sequence of an element of each
/// field, named as §9.1 maps its name fully escaped, of its type, left
/// out of a null value, NULLS absent, or nilled, NULLS nil \[GR2, GR8n\];
/// or of an element `element` of each element, nilled of a null one, at
/// most the array's maximum cardinality \[GR8p\], or unbounded \[GR8q\].
pub(super) fn complex_type(t: &DataType, name: &str, catalog: &str, nulls: Nulls) -> Option<String> {
    let mut out = format!(r#"<xs:complexType name="{name}"><xs:sequence>"#);
    // Writing to a String cannot fail.
    match t {
        DataType::Row(r) => {
            let xmlnulls = match nulls {
                Nulls::Absent => r#"minOccurs="0""#,
                Nulls::Nil => r#"nillable="true""#,
            };
            for f in &r.fields {
                let field = name_of(&f.name, Escape::Full);
                let _ = write!(out, r#"<xs:element name="{field}" type="{}" {xmlnulls}/>"#, named(&f.data_type, catalog, 0)?);
            }
        }
        DataType::Array(a) => elements(&mut out, &named(&a.element, catalog, 0)?, &a.max.to_string()),
        DataType::Multiset(m) => elements(&mut out, &named(&m.element, catalog, 0)?, "unbounded"),
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Distinct(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Xml(_)
        | DataType::Extension(_) => return None,
    }
    out.push_str("</xs:sequence></xs:complexType>");
    Some(out)
}

/// [§9.5 GR8p v, GR8q iv] The element `element` of a collection's elements,
/// of the type `element_type`, at most `most` of them.
fn elements(out: &mut String, element_type: &str, most: &str) {
    // Writing to a String cannot fail.
    let _ = write!(out, r#"<xs:element name="{ELEMENT}" minOccurs="0" maxOccurs="{most}" nillable="true" type="{element_type}"/>"#);
}

/// [§9.7 GR3b, GR3d] The types `t` is made of: of a row type, its fields'
/// types; of a collection type, its element type; of no other, none.
pub(super) fn parts_of(t: &DataType) -> Vec<DataType> {
    match t {
        DataType::Row(r) => r.fields.iter().map(|f| f.data_type.clone()).collect(),
        DataType::Array(a) => vec![a.element.clone()],
        DataType::Multiset(m) => vec![m.element.clone()],
        DataType::Distinct(d) => vec![d.source.clone()],
        DataType::Character { .. }
        | DataType::Varying { .. }
        | DataType::Exact { .. }
        | DataType::Approximate { .. }
        | DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Boolean
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Xml(_)
        | DataType::Extension(_) => Vec::new(),
    }
}
