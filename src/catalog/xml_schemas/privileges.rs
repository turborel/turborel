//! USAGE on a registered XML Schema, as an Access Rule asks for it
//! [ISO/IEC 9075-14:2006 draft §4.7.1, §11.6 AR1].
//!
//! Every reference to a registered XML Schema is an `<XML valid according
//! to what>`: of an `<XML valid according to clause>`, in XMLVALIDATE and
//! IS VALID [§6.20, §8.5], or of a `<secondary XML type modifier>` of
//! XMLSCHEMA [§6.1 SR4], which indicates its registered XML Schema as
//! §11.6 does; so each is checked as §11.6 AR1 says. Which cell's
//! privileges are checked is the caller's to say, as for a domain or a
//! user-defined type: within an `<SQL schema statement>`, not under an
//! `<SQL routine spec>` of SQL SECURITY INVOKER, the owner of the
//! containing schema's alone \[AR1a\]; otherwise the current privileges
//! \[AR1b\].

use super::super::{Catalog, QualifiedName, Who, owner_of};
use super::permanent::permanent;
use crate::error::{Code, Result, SqlError};

/// [ISO/IEC 9075-14:2006 draft §11.6 AR1] That `who` may use the registered
/// XML Schema `name`: it holds USAGE on it, as granted, as its schema's
/// owner, or as everyone holds it on a permanently registered one
/// [§4.2.6 Table 1].
///
/// `who` is the cell whose privileges the Access Rule reads: of a
/// reference within an `<SQL schema statement>`, not under an `<SQL
/// routine spec>` of SQL SECURITY INVOKER, the containing schema's owner
/// alone, `catalog::sole(catalog, catalog::owner_of(catalog, schema))`
/// \[AR1a\]; otherwise the current privileges' cell \[AR1b\].
///
/// # Errors
///
/// SQLCODE -103 if `name` names no registered XML Schema [§11.6 SR1,
/// SR2c]; -104 if `who` holds no USAGE on it \[AR1\].
pub fn may_use(catalog: &dyn Catalog, name: &QualifiedName, who: Who<'_>) -> Result<()> {
    if permanent(name).is_some() {
        return Ok(());
    }
    let Some(desc) = catalog.xml_schema(name) else {
        let msg = format!("[ISO/IEC 9075-14:2006 draft §11.6 SR1] there is no registered XML Schema {name}");
        return Err(SqlError::new(Code::SyntaxRule, msg));
    };
    if desc.usage_of(who, owner_of(catalog, &name.auth)).0 {
        return Ok(());
    }
    let msg = format!("[ISO/IEC 9075-14:2006 draft §11.6 AR1] {} holds no USAGE on registered XML Schema {name}", who.auth);
    Err(SqlError::new(Code::Privilege, msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{MemoryCatalog, Roles, UsageDesc, XmlSchemaDesc, sole};

    /// A catalog of schema A, owned by A, holding the registered XML
    /// Schema A.S, USAGE on it granted to `grants`, each a grantee, none
    /// for PUBLIC.
    fn registered(grants: &[Option<&str>]) -> MemoryCatalog {
        let mut catalog = MemoryCatalog::default();
        catalog.create_schema("A", "A", Vec::new());
        let usage = grants.iter().map(|g| UsageDesc { grantor: "A".into(), grantee: g.map(str::to_owned), grantable: false }).collect();
        catalog.put_xml_schema(XmlSchemaDesc {
            name: QualifiedName { auth: "A".into(), ident: "S".into() },
            target_namespace: "urn:s".into(),
            location: "s.xsd".into(),
            document: String::new(),
            uses: Vec::new(),
            usage,
            schema: None,
        });
        catalog
    }

    /// The name of A.S, or of `auth`.`ident`.
    fn named(auth: &str, ident: &str) -> QualifiedName {
        QualifiedName { auth: auth.into(), ident: ident.into() }
    }

    /// Verifies: [ISO/IEC 9075-14:2006 draft §11.6 AR1] USAGE on a
    /// registered XML Schema is held by its schema's owner, by a grantee,
    /// and by everyone once granted to PUBLIC; another holds none, SQLCODE
    /// -104.
    #[test]
    fn usage_is_the_owners_and_the_grantees() {
        let catalog = registered(&[Some("B")]);
        let s = named("A", "S");
        assert!(may_use(&catalog, &s, sole(&catalog, "A")).is_ok());
        assert!(may_use(&catalog, &s, sole(&catalog, "B")).is_ok());
        assert_eq!(may_use(&catalog, &s, sole(&catalog, "C")).unwrap_err().sqlcode(), -104_i32);
        let public = registered(&[None]);
        assert!(may_use(&public, &s, sole(&public, "C")).is_ok());
    }

    /// Verifies: [ISO/IEC 9075-14:2006 draft §11.6 SR1] a name of no
    /// registered XML Schema is SQLCODE -103, whoever asks; [§4.2.6 Table
    /// 1] each permanently registered one is everyone's.
    #[test]
    fn permanent_schemas_are_everyones_and_others_must_be_registered() {
        let catalog = registered(&[]);
        let roles = Roles::default();
        for name in ["XS", "XSI", "SQLXML"] {
            assert!(may_use(&catalog, &named("INFORMATION_SCHEMA", name), Who::sole(&roles, "C")).is_ok(), "{name}");
        }
        assert_eq!(may_use(&catalog, &named("A", "T"), sole(&catalog, "A")).unwrap_err().sqlcode(), -103_i32);
        assert_eq!(may_use(&catalog, &named("INFORMATION_SCHEMA", "XQ"), sole(&catalog, "A")).unwrap_err().sqlcode(), -103_i32);
    }
}
