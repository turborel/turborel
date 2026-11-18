//! USAGE on a registered XML Schema, as an Access Rule asks for it
//! [ISO/IEC 9075-14:2006 draft §4.7.1, §11.6 AR1].
//!
//! Which cell's privileges are checked is the caller's to say, as for a
//! domain or a user-defined type: within an `<SQL schema statement>`, not
//! under an `<SQL routine spec>` of SQL SECURITY INVOKER, the owner of the
//! containing schema's alone \[AR1a\]; otherwise the current privileges
//! \[AR1b\]. The owner of a registered XML Schema's SQL schema holds USAGE
//! on it, and everyone on a permanently registered one; USAGE granted to
//! others comes with GRANT USAGE ON XML SCHEMA, which completes this check
//! with the grants of the descriptor.

use super::super::{Catalog, QualifiedName, Who, owner_of};
use super::permanent::permanent;
use crate::error::{Code, Result, SqlError};

/// [ISO/IEC 9075-14:2006 draft §11.6 AR1] That `who` may use the registered
/// XML Schema `name`: it is permanently registered [§4.2.6 Table 1], or
/// `who` enables the owner of its schema.
///
/// # Errors
///
/// SQLCODE -103 if `name` names no registered XML Schema [§11.6 SR1];
/// -104 if `who` may not use it \[AR1\].
pub fn may_use(catalog: &dyn Catalog, name: &QualifiedName, who: Who<'_>) -> Result<()> {
    if permanent(name).is_some() {
        return Ok(());
    }
    if catalog.xml_schema(name).is_none() {
        let msg = format!("[ISO/IEC 9075-14:2006 draft §11.6 SR1] there is no registered XML Schema {name}");
        return Err(SqlError::new(Code::SyntaxRule, msg));
    }
    if who.includes(owner_of(catalog, &name.auth)) {
        return Ok(());
    }
    let msg = format!("[ISO/IEC 9075-14:2006 draft §11.6 AR1] {} holds no USAGE on registered XML Schema {name}", who.auth);
    Err(SqlError::new(Code::Privilege, msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{MemoryCatalog, XmlSchemaDesc, sole};
    use crate::system::INFORMATION_SCHEMA;

    /// Verifies: [ISO/IEC 9075-14:2006 draft §4.2.6 Table 1, §11.6 SR1,
    /// AR1] RULES.md D86: the owner of a registered XML Schema's schema may
    /// use it, as everyone may a permanently registered one; another may
    /// not, SQLCODE -104; a name of none is SQLCODE -103.
    #[test]
    fn the_owner_of_its_schema_may_use_a_registered_schema() {
        let mut catalog = MemoryCatalog::default();
        catalog.create_schema("A", "A", Vec::new());
        let name = |auth: &str, ident: &str| QualifiedName { auth: auth.into(), ident: ident.into() };
        let desc = XmlSchemaDesc {
            name: name("A", "S"),
            target_namespace: String::new(),
            location: String::new(),
            document: String::new(),
            uses: Vec::new(),
            usage: Vec::new(),
            schema: None,
        };
        catalog.put_xml_schema(desc);
        assert!(may_use(&catalog, &name("A", "S"), sole(&catalog, "A")).is_ok());
        assert!(may_use(&catalog, &name(INFORMATION_SCHEMA, "XSI"), sole(&catalog, "B")).is_ok());
        assert_eq!(may_use(&catalog, &name("A", "S"), sole(&catalog, "B")).unwrap_err().sqlcode(), -104_i32);
        assert_eq!(may_use(&catalog, &name("A", "NONE"), sole(&catalog, "A")).unwrap_err().sqlcode(), -103_i32);
    }
}
