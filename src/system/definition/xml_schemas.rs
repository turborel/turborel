//! [ISO/IEC 9075-14:2006 draft §21.4] The rows of USAGE_PRIVILEGES of
//! registered XML Schemas, of OBJECT_TYPE XML SCHEMA: those of the
//! catalog (`catalog::XmlSchemaDesc`) and the permanently registered ones
//! of Table 1 [§4.2.6] (`catalog::xml_schemas::PERMANENT`).

use super::privileges::PUBLIC;
use super::{catalog_name, ids, text, yes};
use crate::catalog::xml_schemas::PERMANENT;
use crate::catalog::{QualifiedName, owner_of};
use crate::storage::Row;
use crate::system::{SYSTEM_GRANTOR, Seen};

/// [ISO/IEC 9075-14:2006 draft §21.4] The OBJECT_TYPE of USAGE on a
/// registered XML Schema.
const XML_SCHEMA_OBJECT: &str = "XML SCHEMA";

/// [§21.4] The rows of USAGE_PRIVILEGES of registered XML Schemas: on each
/// permanently registered one, PUBLIC's, of the grantor "_SYSTEM", not
/// grantable; on each the catalog holds, its schema's owner's, of
/// "_SYSTEM", grantable, and each granted.
pub(super) fn usage(seen: &Seen<'_>) -> Vec<Row> {
    let catalog = catalog_name(seen);
    let row = |name: &QualifiedName, (grantor, grantee): (&str, &str), grantable: bool| -> Row {
        let kind = [text(XML_SCHEMA_OBJECT), yes(grantable)];
        [vec![text(grantor), text(grantee)], ids(&catalog, name).to_vec(), kind.to_vec()].concat()
    };
    let mut rows: Vec<Row> = PERMANENT.iter().map(|p| row(&p.qualified(), (SYSTEM_GRANTOR, PUBLIC), false)).collect();
    for x in seen.state.catalog.xml_schemas() {
        rows.push(row(&x.name, (SYSTEM_GRANTOR, owner_of(&*seen.state.catalog, &x.name.auth)), true));
        rows.extend(x.usage.iter().map(|u| row(&x.name, (&u.grantor, u.grantee.as_deref().unwrap_or(PUBLIC)), u.grantable)));
    }
    rows
}
