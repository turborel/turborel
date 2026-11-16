//! [ISO/IEC 9075-11:2006 draft §6.34] The Definition Schema's base table
//! ROLE_AUTHORIZATION_DESCRIPTORS: one row of each role authorization
//! descriptor [ISO/IEC 9075-2:2023 §4.42.3], which the Information
//! Schema's views of roles read (`system/information/roles.rs`).

use std::sync::Arc;

use super::privileges::PUBLIC;
use super::{Columns, defined};
use crate::catalog::ColumnDesc;
use crate::storage::Row;
use crate::system::{Rows, Seen, SystemTable, text, yes};

/// The tables.
pub(super) fn tables() -> Vec<Arc<dyn SystemTable>> {
    vec![defined("ROLE_AUTHORIZATION_DESCRIPTORS", role_authorizations_columns, Rows::Infallible(role_authorizations))]
}

/// [§6.34] Its columns, each of its primary key's, and IS_GRANTABLE, NOT
/// NULL.
fn role_authorizations_columns() -> Vec<ColumnDesc> {
    Columns::of().id("ROLE_NAME", true).id("GRANTEE", true).id("GRANTOR", true).data("IS_GRANTABLE", true).done()
}

/// [§6.34 Description 1 to 4] Each role authorization descriptor: the
/// role, its grantee, PUBLIC of one granted to all, its grantor, "_SYSTEM"
/// of the one a `<role definition>` creates [ISO/IEC 9075-2:2023 §12.4
/// GR2], and whether it is grantable, WITH ADMIN OPTION.
fn role_authorizations(seen: &Seen<'_>) -> Vec<Row> {
    let roles = seen.state.catalog.roles();
    roles
        .authorizations
        .iter()
        .map(|a| vec![text(&a.role), text(a.grantee.as_deref().unwrap_or(PUBLIC)), text(&a.grantor), yes(a.grantable)])
        .collect()
}
