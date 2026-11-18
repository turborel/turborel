//! \[EXTENSION\] [ISO/IEC 9075-14:2006 draft §4.2.6] Registered XML Schemas
//! read into schemas (`xml::schema`): a document read with those of the
//! registered XML Schemas it may include, redefine or import, as its
//! library [Schema1 §4.2], and the registered XML Schemas it read; and
//! every registered XML Schema of a catalog decoded read again, each after
//! those it uses (RULES.md D86).

use std::sync::Arc;

use crate::catalog::{Catalog, QualifiedName, XmlSchemaDesc};
use crate::error::{Code, Result, SqlError};
use crate::xml::schema::permanent::components;
use crate::xml::schema::{Library, Schema, Source, read};
use crate::xml::{SQLXML_NAMESPACE, Syntax, Whitespace, parse};

/// The schema location of the SQL/XML XML Schema's document
/// (`catalog::xml_schemas::PERMANENT`).
const SQLXML_LOCATION: &str = "http://standards.iso.org/iso/9075/2003/sqlxml.xsd";

/// The schema `document`, of schema location `location`, is, read with
/// the documents of `registered` as its library [Schema1 §4.2], and that
/// of the permanently registered SQL/XML XML Schema [ISO/IEC 9075-14:2006
/// draft §22.1], which it may import too; and the names of those of
/// `registered` whose documents it read.
///
/// # Errors
///
/// SQLSTATE 2200M if `document` is no XML document; as `xml::schema::read`.
pub(in crate::db) fn read_registered(
    document: &str,
    location: &str,
    registered: &[&XmlSchemaDesc],
) -> Result<(Arc<Schema>, Vec<QualifiedName>)> {
    let tree = Arc::new(parse(document, Syntax::Document, Whitespace::Preserve)?);
    let mut sources = Vec::new();
    let mut names = Vec::new();
    for desc in registered {
        let Some(own) = desc.schema.as_ref().and_then(|s| s.documents.first()) else { continue };
        sources.push(Source { location: Some(desc.location.as_str().into()), tree: Arc::clone(&own.0) });
        names.push(desc.name.clone());
    }
    if let Some(own) = components(SQLXML_NAMESPACE).as_ref().and_then(|s| s.documents.first()) {
        sources.push(Source { location: Some(SQLXML_LOCATION.into()), tree: Arc::clone(&own.0) });
    }
    let schema = read(&Source { location: Some(location.into()), tree }, &Library::new(sources.clone()))?;
    let mut uses: Vec<QualifiedName> = schema
        .documents
        .iter()
        .skip(1)
        .filter_map(|(t, _)| sources.iter().position(|s| Arc::ptr_eq(&s.tree, t)).and_then(|i| names.get(i).cloned()))
        .collect();
    uses.sort_by_key(ToString::to_string);
    uses.dedup();
    Ok((Arc::new(schema), uses))
}

/// Every registered XML Schema of `catalog` whose schema is not read, read
/// from its document, each after those it uses, with their documents
/// alone as its library.
///
/// # Errors
///
/// SQLCODE -916 if one cannot be read again, or uses one that is not
/// registered or uses it back, which no catalog written by turborel holds.
pub(super) fn read_all(catalog: &mut dyn Catalog) -> Result<()> {
    let mut pending: Vec<XmlSchemaDesc> = catalog.xml_schemas().into_iter().filter(|d| d.schema.is_none()).cloned().collect();
    while !pending.is_empty() {
        let ready = pending.iter().position(|d| d.uses.iter().all(|u| catalog.xml_schema(u).is_some_and(|r| r.schema.is_some())));
        let Some(at) = ready else {
            return Err(corrupt("a registered XML Schema uses one that is not registered, or uses it back"));
        };
        let mut desc = pending.swap_remove(at);
        let used: Vec<XmlSchemaDesc> = desc.uses.iter().filter_map(|u| catalog.xml_schema(u).cloned()).collect();
        let (schema, _) = read_registered(&desc.document, &desc.location, &used.iter().collect::<Vec<_>>())
            .map_err(|e| corrupt(&format!("registered XML Schema {} is read no more: {e}", desc.name)))?;
        desc.schema = Some(schema);
        catalog.put_xml_schema(desc);
    }
    Ok(())
}

/// The error of a catalog that is not as written.
fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] the catalog is damaged: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::Reader;
    use crate::catalog::MemoryCatalog;
    use crate::catalog::codec::{put_xml_schema, xml_schema};

    /// A registered XML Schema named `name`, of document `document` at
    /// `location`, using `uses`, its schema not read.
    fn desc(name: &str, (location, document): (&str, &str), uses: &[&str]) -> XmlSchemaDesc {
        let qualified = |n: &str| QualifiedName { auth: "A".into(), ident: n.into() };
        XmlSchemaDesc {
            name: qualified(name),
            target_namespace: String::new(),
            location: location.into(),
            document: document.into(),
            uses: uses.iter().map(|u| qualified(u)).collect(),
            usage: Vec::new(),
            schema: None,
        }
    }

    /// The schema document of target namespace `tns` and `body`.
    fn doc(tns: &str, body: &str) -> String {
        format!(r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="{tns}" xmlns:t="{tns}">{body}</xs:schema>"#)
    }

    /// Verifies: [EXTENSION] [ISO/IEC 9075-14:2006 draft §4.2.6] RULES.md
    /// D86 [Schema1 §4.2.3]: a registered XML Schema read with the others
    /// as its library records those whose documents it imported; its
    /// descriptor stores as its text and uses, not its schema; and a
    /// catalog decoded has each read again, after those it uses.
    #[test]
    fn registered_schemas_are_read_with_those_they_use() {
        let mut catalog = MemoryCatalog::default();
        let base = doc("urn:b", r#"<xs:complexType name="bt"/>"#);
        let (schema, uses) = read_registered(&base, "b.xsd", &[]).unwrap();
        assert_eq!(uses, Vec::<QualifiedName>::new());
        let mut b = desc("B", ("b.xsd", &base), &[]);
        b.schema = Some(schema);
        let main =
            doc("urn:m", r#"<xs:import namespace="urn:b" schemaLocation="b.xsd"/><xs:element name="e" xmlns:b="urn:b" type="b:bt"/>"#);
        let (_, imported) = read_registered(&main, "m.xsd", &[&b]).unwrap();
        assert_eq!(imported, vec![b.name.clone()]);
        let m = desc("M", ("m.xsd", &main), &["B"]);
        let mut bytes = Vec::new();
        put_xml_schema(&mut bytes, &m);
        let back = xml_schema(&mut Reader::new(&bytes, "XML Schema")).unwrap();
        assert_eq!((back.uses.clone(), back.document.clone(), back.schema.is_none()), (m.uses.clone(), m.document.clone(), true));
        catalog.put_xml_schema(back);
        catalog.put_xml_schema(desc("B", ("b.xsd", &base), &[]));
        read_all(&mut catalog).unwrap();
        assert!(catalog.xml_schemas().iter().all(|d| d.schema.is_some()));
        let e = catalog.xml_schema(&m.name).and_then(|d| d.schema.clone()).unwrap();
        assert!(e.element_names.keys().any(|n| &*n.local == "e"));
    }

    /// Verifies: [DESIGN] RULES.md D86: a catalog whose registered XML
    /// Schemas use one not registered, or one another, is damaged, SQLCODE
    /// -916, as is one whose document no longer reads.
    #[test]
    fn a_damaged_catalog_is_refused() {
        let mut missing = MemoryCatalog::default();
        missing.put_xml_schema(desc("M", ("m.xsd", &doc("urn:m", "")), &["GONE"]));
        assert_eq!(read_all(&mut missing).unwrap_err().sqlcode(), -916_i32);
        let mut bad = MemoryCatalog::default();
        bad.put_xml_schema(desc("M", ("m.xsd", "<notschema/>"), &[]));
        assert_eq!(read_all(&mut bad).unwrap_err().sqlcode(), -916_i32);
    }
}
