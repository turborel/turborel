//! [ISO/IEC 9075-2:2023 §11.49 GR6, §11.50 GR1] The privileges an INSTEAD
//! OF trigger gives the owner of its view's schema, granted by the special
//! grantor `SYSTEM_GRANTOR`, grantable: INSERT, UPDATE or DELETE on the
//! view, and INSERT or UPDATE on each of its columns, as its event is. They
//! are revoked with CASCADE as the trigger is dropped.

use super::super::State;
use super::super::ddl::Dropped;
use crate::catalog::{Actions, Privilege, PrivilegeDesc, SYSTEM_GRANTOR, TriggerDesc, TriggerEvent};
use crate::error::Result;

impl State {
    /// [ISO/IEC 9075-2:2023 §11.49 GR6] Grants `owner` the privileges the
    /// INSTEAD OF trigger `t` gives.
    pub(in crate::db) fn grant_instead(&mut self, t: &TriggerDesc, owner: &str) {
        let actions = Actions::of(self.instead_privileges(t));
        let desc = PrivilegeDesc {
            grantor: SYSTEM_GRANTOR.to_owned(),
            grantee: Some(owner.to_owned()),
            table: t.table.clone(),
            actions,
            grantable: true,
        };
        let mut catalog = self.catalog.duplicate();
        catalog.grant(vec![desc]);
        self.catalog = std::sync::Arc::from(catalog);
    }

    /// [ISO/IEC 9075-2:2023 §11.50 GR1] Revokes, with CASCADE, the
    /// privileges the INSTEAD OF trigger `t`, dropped, gave: what that took
    /// away.
    ///
    /// # Errors
    ///
    /// As a drop's of what it abandons.
    pub(in crate::db) fn revoke_instead(&mut self, t: &TriggerDesc) -> Result<Dropped> {
        let owner = crate::catalog::owner_of(&*self.catalog, &t.name.auth).to_owned();
        let revoked = self.instead_privileges(t);
        self.revoke_system(&t.table, (&revoked, &owner))
    }

    /// The privileges on its view the INSTEAD OF trigger `t` gives.
    fn instead_privileges(&self, t: &TriggerDesc) -> Vec<Privilege> {
        let columns = self.catalog.view(&t.table).map_or(0, |v| v.columns.len());
        match t.event {
            TriggerEvent::Insert => (0..columns).map(Privilege::Insert).collect(),
            TriggerEvent::Update(_) => (0..columns).map(Privilege::Update).collect(),
            TriggerEvent::Delete => vec![Privilege::Delete],
        }
    }
}
