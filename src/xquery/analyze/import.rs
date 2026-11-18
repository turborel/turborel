//! Schema imports [XQuery §4.10]: a registered XML Schema imported by its
//! target namespace into the in-scope schema definitions, and the schema
//! types, element declarations and attribute declarations it brings, by
//! which a sequence type's names are resolved [XQuery §2.5.4.3 to
//! §2.5.4.6].
//!
//! \[DESIGN\] Implementation-defined, RULES.md D106: the schema of a target
//! namespace is the registered XML Schema of it the current user may use
//! whose schema location is a location hint, else the one of the least
//! name, a permanently registered one last; a schema imported brings the
//! components of every namespace it has. A library module imports among
//! those it kept as it was registered (`LibraryModule::importable`).

use std::sync::Arc;

use super::super::ast::{QName as Lexical, SchemaImport, SchemaPrefix};
use super::super::error::{ErrorCode, error};
use super::super::types::{AttributeKind, Defined, ElementKind, Imports, SchemaType};
use super::Analysis;
use super::scope::Unprefixed;
use super::types::undeclared;
use crate::error::Result;
use crate::xml::QName;
use crate::xml::schema::{ElementId, Expanded, MAX_CHAIN, Schema, TypeDef, annotation_name, defined_twice};

impl Analysis<'_> {
    /// [XQuery §4.10] `import` applied: the registered XML Schema of its
    /// target namespace imported, its prefix bound to that namespace or
    /// that namespace made the default element/type namespace.
    ///
    /// # Errors
    ///
    /// err:XQST0058 if the prolog imported the namespace before;
    /// err:XQST0059 if no registered XML Schema the module may import has
    /// it; err:XQST0035 if an imported schema defines a name of it from
    /// other schema documents than this one does.
    pub(super) fn import_schema(&mut self, import: &SchemaImport) -> Result<()> {
        let uri = import.uri.as_str();
        if self.imported.iter().any(|i| i.namespace == uri) {
            return Err(error(
                ErrorCode::XQST0058,
                &format!("the prolog imports the schema of target namespace '{uri}' twice [XQuery §4.10]"),
            ));
        }
        let importable = self.own.importable;
        let of = || importable.iter().filter(|s| s.namespace == uri);
        let Some(found) = of().find(|s| import.at.contains(&s.location)).or_else(|| of().next()).cloned() else {
            let why = format!("no registered XML Schema the current user may use has the target namespace '{uri}' [XQuery §4.10]");
            return Err(error(ErrorCode::XQST0059, &why));
        };
        if let Some((other, name)) = self.imported.iter().find_map(|i| defined_twice(&i.schema, &found.schema).map(|n| (&i.name, n))) {
            let why = format!("{other} and {} both define {}, from different schema documents [XQuery §4.10]", found.name, name.text());
            return Err(error(ErrorCode::XQST0035, &why));
        }
        match &import.prefix {
            Some(SchemaPrefix::Namespace(prefix)) => self.scope.names.prefixes.push((prefix.as_str().into(), uri.into())),
            Some(SchemaPrefix::DefaultElement) => self.scope.names.default_element = (!uri.is_empty()).then(|| uri.into()),
            None => {}
        }
        self.imported.push(found);
        Ok(())
    }

    /// The schemas imported, against which a node's type annotation is
    /// resolved.
    pub(super) fn imports(&self) -> Imports {
        Imports(self.imported.iter().map(|i| Arc::clone(&i.schema)).collect())
    }

    /// The type definition an imported schema names `name`, if one does.
    pub(super) fn imported_type(&self, name: &QName) -> Option<SchemaType> {
        let key = Expanded::new(name.uri.as_deref(), &name.local);
        self.imported.iter().find_map(|i| i.schema.type_names.get(&key).map(|def| schema_type(&i.schema, *def)))
    }

    /// [XQuery §2.5.4.4] The schema element test of `name`: of the global
    /// element declaration of that name of an imported schema, its name and
    /// those of the substitution group it heads, its type, and its
    /// nillable.
    ///
    /// # Errors
    ///
    /// err:XPST0008 if no imported schema declares it.
    pub(super) fn schema_element(&self, name: &Lexical) -> Result<ElementKind> {
        let expanded = self.scope.name(name, Unprefixed::Element)?;
        let key = Expanded::new(expanded.uri.as_deref(), &expanded.local);
        for i in &self.imported {
            let Some(&id) = i.schema.element_names.get(&key) else { continue };
            let Some(decl) = i.schema.element(id) else { continue };
            let members = i.schema.element_names.values().filter(|&&e| heads(&i.schema, e, id)).filter_map(|&e| i.schema.element(e));
            let substitutes = members.map(|d| QName { prefix: None, uri: d.name.uri.clone(), local: d.name.local.clone() }).collect();
            let annotation = Some(schema_type(&i.schema, decl.type_def));
            return Ok(ElementKind { name: Some(expanded), substitutes, annotation, nillable: decl.nillable, imports: self.imports() });
        }
        Err(undeclared(name, "element", "§2.5.4.4"))
    }

    /// [XQuery §2.5.4.6] The schema attribute test of `name`: of the
    /// global attribute declaration of that name of an imported schema, its
    /// name and its type.
    ///
    /// # Errors
    ///
    /// err:XPST0008 if no imported schema declares it.
    pub(super) fn schema_attribute(&self, name: &Lexical) -> Result<AttributeKind> {
        let expanded = self.scope.name(name, Unprefixed::None)?;
        let key = Expanded::new(expanded.uri.as_deref(), &expanded.local);
        let found = self.imported.iter().find_map(|i| {
            let decl = i.schema.attribute_names.get(&key).and_then(|&a| i.schema.attribute(a))?;
            Some(schema_type(&i.schema, TypeDef::Simple(decl.type_def)))
        });
        let annotation = found.ok_or_else(|| undeclared(name, "attribute", "§2.5.4.6"))?;
        Ok(AttributeKind { name: Some(expanded), annotation: Some(annotation), imports: self.imports() })
    }
}

/// The schema type of `def` of `schema`: a built-in one, by its name, else
/// the type definition of the schema imported.
pub(in crate::xquery) fn schema_type(schema: &Arc<Schema>, def: TypeDef) -> SchemaType {
    let name = annotation_name(schema, def);
    SchemaType::named(name.uri.as_deref(), &name.local)
        .unwrap_or_else(|| SchemaType::Defined(Arc::new(Defined { name, schema: Arc::clone(schema), def })))
}

/// Whether element declaration `e` of `schema` is in the substitution
/// group `head` heads [Schema1 §3.3.6]: `head` reached by its substitution
/// group affiliations, within `MAX_CHAIN` of them.
pub(in crate::xquery) fn heads(schema: &Schema, e: ElementId, head: ElementId) -> bool {
    let mut at = Some(e);
    for _ in 0..MAX_CHAIN {
        at = at.and_then(|x| schema.element(x)).and_then(|d| d.substitution_group);
        if at == Some(head) || at.is_none() {
            return at.is_some();
        }
    }
    false
}
