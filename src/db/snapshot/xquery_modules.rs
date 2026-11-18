//! \[EXTENSION\] Registered XQuery modules of a catalog decoded, each
//! parsed again from its text (RULES.md D106), as a registered XML Schema
//! is read again from its document (`snapshot/xml_schemas.rs`).

use std::sync::Arc;

use crate::catalog::Catalog;
use crate::catalog::xml_schemas::permanent;
use crate::error::{Code, Result, SqlError};
use crate::xml::Version;

/// Every registered XQuery module of `catalog` whose text is not parsed,
/// parsed, as a query's text is, by XML 1.1's lexical rules.
///
/// # Errors
///
/// SQLCODE -916 if one's text no longer parses, or it depends on a module
/// or a registered XML Schema that is not registered, which no catalog
/// written by turborel holds.
pub(super) fn parse_all(catalog: &mut dyn Catalog) -> Result<()> {
    let pending: Vec<_> = catalog.xquery_modules().into_iter().filter(|m| m.module.is_none()).cloned().collect();
    for mut desc in pending {
        let missing = desc.modules.iter().find(|m| catalog.xquery_module(m).is_none()).map(ToString::to_string);
        let schema = desc.xml_schemas.iter().find(|s| permanent(s).is_none() && catalog.xml_schema(s).is_none()).map(ToString::to_string);
        if let Some(gone) = missing.or(schema) {
            return Err(corrupt(&format!("registered XQuery module {} depends on {gone}, which is not registered", desc.name)));
        }
        let parsed = crate::xquery::parse(&desc.text, Version::V11)
            .map_err(|e| corrupt(&format!("registered XQuery module {} is parsed no more: {e}", desc.name)))?;
        desc.module = Some(Arc::new(parsed));
        catalog.put_xquery_module(desc);
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
    use crate::catalog::codec::{put_xquery_module, xquery_module};
    use crate::catalog::{MemoryCatalog, QualifiedName, XQueryModuleDesc};

    /// The registered XQuery module A.`name` of `text`, depending on the
    /// modules `modules`, its text not parsed.
    fn desc(name: &str, text: &str, modules: &[&str]) -> XQueryModuleDesc {
        let qualified = |n: &str| QualifiedName { auth: "A".into(), ident: n.into() };
        XQueryModuleDesc {
            name: qualified(name),
            target_namespace: "urn:m".into(),
            location: "m.xq".into(),
            text: text.into(),
            modules: modules.iter().map(|m| qualified(m)).collect(),
            xml_schemas: vec![QualifiedName { auth: "INFORMATION_SCHEMA".into(), ident: "XS".into() }],
            usage: Vec::new(),
            module: None,
        }
    }

    /// Verifies: [EXTENSION] RULES.md D106: a registered XQuery module's
    /// descriptor stores its text and what it depends on, not its parse,
    /// which a catalog decoded makes again.
    #[test]
    fn registered_modules_are_stored_as_their_text() {
        let m = desc("M", "module namespace m = 'urn:m'; declare function m:f() { 1 };", &[]);
        let mut bytes = Vec::new();
        put_xquery_module(&mut bytes, &m);
        let back = xquery_module(&mut Reader::new(&bytes, "XQuery module")).unwrap();
        assert_eq!(format!("{back:?}"), format!("{m:?}"));
        let mut catalog = MemoryCatalog::default();
        catalog.put_xquery_module(back);
        catalog.put_xquery_module(desc("U", "module namespace u = 'urn:u';", &["M"]));
        parse_all(&mut catalog).unwrap();
        assert!(catalog.xquery_modules().iter().all(|d| d.module.is_some()));
    }

    /// Verifies: [DESIGN] RULES.md D106: a catalog whose registered XQuery
    /// module depends on one not registered, or whose text no longer
    /// parses, is damaged, SQLCODE -916.
    #[test]
    fn a_damaged_catalog_is_refused() {
        let mut missing = MemoryCatalog::default();
        missing.put_xquery_module(desc("U", "module namespace u = 'urn:u';", &["GONE"]));
        assert_eq!(parse_all(&mut missing).unwrap_err().sqlcode(), -916_i32);
        let mut bad = MemoryCatalog::default();
        bad.put_xquery_module(desc("M", "module namespace m = ;", &[]));
        assert_eq!(parse_all(&mut bad).unwrap_err().sqlcode(), -916_i32);
    }
}
