//! \[EXTENSION\] Registered XQuery modules, as the catalog describes them
//! (RULES.md D106): library modules [XQuery §4.2] a query's module import
//! imports [XQuery §4.11], which \[XQuery\] lets the implementation find as
//! it chooses [XQuery §5.2.5]. A descriptor holds the module's name, target
//! namespace and location, its text, the registered XQuery modules its
//! module imports imported and the registered XML Schemas it imported or
//! validates by as it was registered, which it depends on, the USAGE
//! granted on it, and its creation and last-altered timestamps, as an
//! SQL-server module's descriptor keeps them (Feature T011).
//!
//! \[DESIGN\] RULES.md D106: the text is kept, and its parse
//! (`xquery::Module`) is made again from it when the catalog is read, so
//! the catalog stores nothing derived. USAGE is held and checked as on a
//! registered XML Schema (`catalog::xml_schemas::may_use`).

use std::sync::Arc;

use super::domains::{UsageDesc, usage_held};
use super::{Catalog, QualifiedName, Who, owner_of};
use crate::error::{Code, Result, SqlError};

/// \[EXTENSION\] A registered XQuery module descriptor.
#[derive(Clone)]
pub struct XQueryModuleDesc {
    /// Its name, a `<schema qualified name>`.
    pub name: QualifiedName,
    /// Its target namespace URI [XQuery §4.2], of nonzero length.
    pub target_namespace: String,
    /// Its location URI, the zero-length string if none was written.
    pub location: String,
    /// The text of the library module registered.
    pub text: String,
    /// The registered XQuery modules its module imports imported as it was
    /// registered [XQuery §4.11], which it depends on and imports again
    /// whenever it is imported.
    pub modules: Vec<QualifiedName>,
    /// The registered XML Schemas its schema imports imported [XQuery
    /// §4.10], and, if it has a validate expression, its in-scope schema
    /// definitions [ISO/IEC 9075-14:2006 draft §10.20 SR2e], as it was
    /// registered; it depends on them.
    pub xml_schemas: Vec<QualifiedName>,
    /// USAGE on it: each grant.
    pub usage: Vec<UsageDesc>,
    /// Its creation and last-altered timestamps: of CREATE XQUERY MODULE,
    /// and of the last ALTER XQUERY MODULE, as a routine's are [ISO/IEC
    /// 9075-2:2023 §11.60 GR5, §11.61 GR4].
    pub stamps: super::Stamps,
    /// Its text, parsed; none until parsed, as of a descriptor the catalog
    /// has just decoded.
    pub module: Option<Arc<crate::xquery::Module>>,
}

impl std::fmt::Debug for XQueryModuleDesc {
    /// Its stored fields: its parse, derived from them, is not shown.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XQueryModuleDesc")
            .field("name", &self.name)
            .field("target_namespace", &self.target_namespace)
            .field("location", &self.location)
            .field("text", &self.text)
            .field("modules", &self.modules)
            .field("xml_schemas", &self.xml_schemas)
            .field("usage", &self.usage)
            .field("stamps", &self.stamps)
            .finish_non_exhaustive()
    }
}

impl XQueryModuleDesc {
    /// Whether `who` holds USAGE on it, and whether grantably, as of a
    /// domain (`DomainDesc::usage_of`); its schema's owner `owner` holds
    /// it, grantably.
    pub fn usage_of(&self, who: Who<'_>, owner: &str) -> (bool, bool) {
        usage_held(&self.usage, who, owner)
    }
}

/// \[EXTENSION\] That `who` may use the registered XQuery module `name`:
/// it holds USAGE on it, as granted or as its schema's owner, as of a
/// registered XML Schema [ISO/IEC 9075-14:2006 draft §11.6 AR1].
///
/// `who` is the cell whose privileges are read: of a module a definition
/// or a module registered imports, its schema's owner alone; otherwise the
/// current privileges' cell.
///
/// # Errors
///
/// SQLCODE -103 if `name` names no registered XQuery module; -104 if
/// `who` holds no USAGE on it.
pub fn may_use(catalog: &dyn Catalog, name: &QualifiedName, who: Who<'_>) -> Result<()> {
    let Some(desc) = catalog.xquery_module(name) else {
        return Err(SqlError::new(Code::SyntaxRule, format!("[EXTENSION] there is no registered XQuery module {name}")));
    };
    if desc.usage_of(who, owner_of(catalog, &name.auth)).0 {
        return Ok(());
    }
    let msg = format!("[EXTENSION] {} holds no USAGE on registered XQuery module {name}", who.auth);
    Err(SqlError::new(Code::Privilege, msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{MemoryCatalog, sole};

    /// A catalog of schema A, owned by A, holding the registered XQuery
    /// module A.M, USAGE on it granted to `grants`, each a grantee, none
    /// for PUBLIC.
    fn registered(grants: &[Option<&str>]) -> MemoryCatalog {
        let mut catalog = MemoryCatalog::default();
        catalog.create_schema("A", "A", Vec::new());
        let usage = grants.iter().map(|g| UsageDesc { grantor: "A".into(), grantee: g.map(str::to_owned), grantable: false }).collect();
        catalog.put_xquery_module(XQueryModuleDesc {
            name: QualifiedName { auth: "A".into(), ident: "M".into() },
            target_namespace: "urn:m".into(),
            location: String::new(),
            text: "module namespace m = 'urn:m';".into(),
            modules: Vec::new(),
            xml_schemas: Vec::new(),
            usage,
            stamps: crate::catalog::Stamps::default(),
            module: None,
        });
        catalog
    }

    /// Verifies: [EXTENSION] RULES.md D106: USAGE on a registered XQuery
    /// module is held by its schema's owner, by a grantee, and by everyone
    /// once granted to PUBLIC; another holds none, SQLCODE -104; a name of
    /// none is -103.
    #[test]
    fn usage_is_the_owners_and_the_grantees() {
        let catalog = registered(&[Some("B")]);
        let m = QualifiedName { auth: "A".into(), ident: "M".into() };
        assert!(may_use(&catalog, &m, sole(&catalog, "A")).is_ok());
        assert!(may_use(&catalog, &m, sole(&catalog, "B")).is_ok());
        assert_eq!(may_use(&catalog, &m, sole(&catalog, "C")).unwrap_err().sqlcode(), -104_i32);
        let public = registered(&[None]);
        assert!(may_use(&public, &m, sole(&public, "C")).is_ok());
        let none = QualifiedName { auth: "A".into(), ident: "N".into() };
        assert_eq!(may_use(&catalog, &none, sole(&catalog, "A")).unwrap_err().sqlcode(), -103_i32);
    }
}
