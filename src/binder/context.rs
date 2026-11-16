//! What a statement's query is bound in, made for a statement
//! (`binder::Context`).

use super::{Context, Purpose};
use crate::catalog::{Principal, WHOLE, Who};
use crate::db::State;
use crate::params::Params;
use crate::settings;

impl<'a> Context<'a> {
    /// [ISO/IEC 9075-2:2023 §4.42.1.2] The cell whose privileges it is
    /// bound with: `auth`, and `role`.
    pub fn who(&self) -> Who<'a> {
        Who { auth: self.auth, role: self.role, roles: self.db.catalog.roles() }
    }

    /// A statement's query, run under `auth` with `params`.
    pub fn new(db: &'a State, auth: &'a str, params: &'a Params) -> Context<'a> {
        let (purpose, own) = (Purpose::Query, &settings::NONE);
        Context {
            now: db.now(own).into(),
            db,
            auth,
            role: None,
            schema: auth,
            user: Principal::sole(db.catalog.roles(), auth),
            params,
            purpose,
            collation: None,
            horizon: &WHOLE,
            own,
            definitions: false,
            uses: None,
            selected: None,
            referencing: None,
            draws: false,
            queries: None,
            invoked: None,
            drawn: None,
            hosts: false,
            compile: None,
        }
    }
}
