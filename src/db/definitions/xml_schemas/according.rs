//! The registered XML Schema an `<XML valid according to what>` indicates,
//! and the namespace and global element declaration its `<XML valid
//! element clause>` indicates [ISO/IEC 9075-14:2006 draft §11.6], as an
//! XML type of XMLSCHEMA [§6.1 SR4], XMLVALIDATE [§6.20 SR2] and IS VALID
//! [§8.5 SR4] name one.
//!
//! \[DESIGN\] Implementation-defined choices, RULES.md D86:
//!
//! - Registered XML Schemas are identified by their names [§4.2.6], not by
//!   target namespace and schema location, so `<XML valid according to
//!   URI>` takes SR2b ii: of the registered XML Schemas of the target
//!   namespace that the user may use, and of the schema location if one
//!   is written, the one of the least name, schema then identifier,
//!   which is repeatable as the rule asks [SR2b ii 1]. The permanently
//!   registered ones come last, as turborel holds no components of them.
//! - The registered XML Schema shall have the namespace the element clause
//!   indicates, and the global element declaration it names [SR4b, SR4d
//!   iii]: one that has not raises, as the type or clause is bound, the
//!   exception GR1 would raise of it, SQLSTATE 2201M or 2201N, as no
//!   registered XML Schema changes once registered.

use std::sync::Arc;

use super::super::super::State;
use crate::ast::{XmlValidAccording, XmlValidWhat};
use crate::binder::qualify;
use crate::catalog::xml_schemas::{may_use, permanent};
use crate::catalog::{QualifiedName, Who, XmlSchemaDesc};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::types::XmlSchemaNamed;
use crate::value::xml::{Typing, XmlType};
use crate::xml::Syntax;
use crate::xml::schema::{Expanded, Schema};

/// [§11.6 SR3 to SR4] What an `<XML valid according to what>` and its
/// `<XML valid element clause>` indicate.
#[derive(Debug, Clone)]
pub struct Indicated {
    /// The indicated registered XML Schema's name \[SR3\].
    pub name: QualifiedName,
    /// Its schema, read from its document.
    pub schema: Arc<Schema>,
    /// The indicated XML namespace, ENSURI, of an element clause \[SR4c\].
    pub namespace: Option<String>,
    /// The indicated global element declaration's NCName, EN \[SR4d\].
    pub element: Option<String>,
}

impl Indicated {
    /// What an XML type's descriptor includes of it [§6.1 GR1d to GR1f].
    #[must_use]
    pub fn named(&self) -> XmlSchemaNamed {
        XmlSchemaNamed {
            schema: self.name.auth.clone(),
            name: self.name.ident.clone(),
            namespace: self.namespace.clone(),
            element: self.element.clone(),
        }
    }
}

impl State {
    /// [§6.1 SR4, GR1] The XML type `syntax`(XMLSCHEMA `according`), its
    /// names in schema `schema`, for the cell `who`.
    ///
    /// # Errors
    ///
    /// As `indicated`.
    pub(crate) fn xml_schema_type(&self, syntax: Syntax, according: &XmlValidAccording, (schema, who): (&str, Who<'_>)) -> Result<XmlType> {
        let typing = Typing::Schema(Arc::new(self.indicated(according, (schema, who))?.named()));
        Ok(match syntax {
            Syntax::Document => XmlType::Document(typing),
            Syntax::Content => XmlType::Content(typing),
        })
    }

    /// [§11.6] What `according` indicates, its `<registered XML Schema
    /// name>` in schema `schema` if it names none, for the cell `who`, as
    /// AR1 says which. Of a definition bound again, its URI was made the
    /// ID of the one it indicated as it was created [§4.2.6]
    /// (`binder/xml/pinned.rs`).
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it indicates no registered XML Schema [SR1, SR2c];
    /// -104 if `who` may not use it \[AR1\]; -900 if it is permanently
    /// registered; SQLSTATE 2201M if it has no namespace ENSURI, 2201N no
    /// global element declaration EN [SR4b, SR4d iii, GR1].
    pub(crate) fn indicated(&self, according: &XmlValidAccording, (schema, who): (&str, Who<'_>)) -> Result<Indicated> {
        let name = match &according.what {
            XmlValidWhat::Id(written) => qualify(written, schema),
            XmlValidWhat::Uri { namespace, location } => self.of_namespace((namespace, location.as_deref()), who)?,
        };
        may_use(&*self.catalog, &name, who)?;
        if permanent(&name).is_some() {
            return Err(SqlError::new(
                Code::Unsupported,
                format!(
                    "[ISO/IEC 9075-14:2006 draft §4.2.6] {name} is permanently registered, and turborel holds no components of it to validate by"
                ),
            ));
        }
        let desc = self.catalog.xml_schema(&name).ok_or_else(|| missing(&name))?;
        let read =
            desc.schema.clone().ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] registered XML Schema {name} is not read")))?;
        let (namespace, element) = element_clause(according, desc, &read)?;
        Ok(Indicated { name, schema: read, namespace, element })
    }

    /// [§11.6 SR2b ii] The registered XML Schema of target namespace
    /// `namespace`, and of schema location `location` if written, that
    /// `who` may use, of the least name, the permanently registered last;
    fn of_namespace(&self, (namespace, location): (&str, Option<&str>), who: Who<'_>) -> Result<QualifiedName> {
        let of = |target: &str, at: &str| target == namespace && location.is_none_or(|l| l == at);
        let catalog = &*self.catalog;
        let mut found: Vec<QualifiedName> = catalog
            .xml_schemas()
            .into_iter()
            .filter(|d| of(&d.target_namespace, &d.location))
            .filter(|d| may_use(catalog, &d.name, who).is_ok())
            .map(|d| d.name.clone())
            .collect();
        found.sort();
        let lasting = crate::catalog::xml_schemas::PERMANENT
            .iter()
            .filter(|p| of(p.target_namespace, p.location))
            .map(crate::catalog::xml_schemas::Permanent::qualified);
        found.extend(lasting);
        found.into_iter().next().ok_or_else(|| {
            let at = location.map(|l| format!(" and schema location '{l}'")).unwrap_or_default();
            SqlError::new(
                Code::SyntaxRule,
                format!("[ISO/IEC 9075-14:2006 draft §11.6 SR2b ii] there is no registered XML Schema of target namespace '{namespace}'{at} that {} may use", who.auth),
            )
        })
    }
}

/// The error of `name`, which names no registered XML Schema [§11.6 SR1].
fn missing(name: &QualifiedName) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-14:2006 draft §11.6 SR1] there is no registered XML Schema {name}"))
}

/// [§11.6 SR4] The namespace ENSURI and NCName EN `according`'s element
/// clause indicates of `desc`, read as `read`: ENSURI the clause's, else
/// the target namespace NSURI \[SR4a\]; each that RXS has, as turborel
/// requires \[SR4b, SR4d iii\], else the exception of GR1.
fn element_clause(according: &XmlValidAccording, desc: &XmlSchemaDesc, read: &Schema) -> Result<(Option<String>, Option<String>)> {
    if !according.has_element_clause() {
        return Ok((None, None));
    }
    let ensuri = according.namespace.clone().unwrap_or_else(|| desc.target_namespace.clone());
    let uri = Some(ensuri.as_str()).filter(|u| !u.is_empty());
    let declared = desc.target_namespace == ensuri || read.namespaces().contains(&uri);
    if !declared {
        return Err(SqlError::new(
            Code::Data,
            format!("[ISO/IEC 9075-14:2006 draft §11.6 GR1a] registered XML Schema {} has no namespace '{ensuri}'", desc.name),
        )
        .with_state(SqlState::ELEMENT_NAMESPACE_NOT_DECLARED));
    }
    if let Some(en) = &according.element
        && !read.element_names.contains_key(&Expanded::new(uri, en))
    {
        return Err(SqlError::new(
            Code::Data,
            format!(
                "[ISO/IEC 9075-14:2006 draft §11.6 GR1b] registered XML Schema {} declares no global element {} of namespace '{ensuri}'",
                desc.name, en
            ),
        )
        .with_state(SqlState::GLOBAL_ELEMENT_NOT_DECLARED));
    }
    Ok((Some(ensuri), according.element.clone()))
}
