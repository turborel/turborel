//! \[DESIGN\] `MemoryCatalog`, the built-in catalog, held in memory.

use std::collections::HashMap;

use super::{
    Catalog, CollationDesc, ConstraintDesc, DomainDesc, IndexDesc, PrivilegeDesc, QualifiedName, Roles, SequenceDesc, SqlRoutineDesc,
    TableDesc, TriggerDesc, UserTypeDesc, ViewDesc,
};
use crate::hash::murmur::MurmurMap;

/// \[DESIGN\] The built-in catalog, held in memory; `Database::new` uses it.
#[derive(Default, Clone)]
pub struct MemoryCatalog {
    /// Each schema's name, and its owner, in the order created.
    schemas: Vec<(String, String)>,
    tables: MurmurMap<QualifiedName, TableDesc>,
    indexes: Vec<IndexDesc>,
    views: HashMap<QualifiedName, ViewDesc>,
    privileges: Vec<PrivilegeDesc>,
    domains: HashMap<QualifiedName, DomainDesc>,
    collations: HashMap<QualifiedName, CollationDesc>,
    sequences: HashMap<QualifiedName, SequenceDesc>,
    user_types: HashMap<QualifiedName, UserTypeDesc>,
    assertions: HashMap<QualifiedName, ConstraintDesc>,
    triggers: HashMap<QualifiedName, TriggerDesc>,
    /// Each SQL-invoked routine, by its specific name.
    routines: HashMap<QualifiedName, SqlRoutineDesc>,
    /// The roles and role authorizations.
    roles: Roles,
}

impl Catalog for MemoryCatalog {
    fn duplicate(&self) -> Box<dyn Catalog> {
        Box::new(self.clone())
    }
    fn roles(&self) -> &Roles {
        &self.roles
    }
    fn set_roles(&mut self, roles: Roles) {
        self.roles = roles;
    }
    fn schema_exists(&self, schema: &str) -> bool {
        self.schemas.iter().any(|(s, _)| s == schema)
    }
    fn schemas(&self) -> Vec<&str> {
        self.schemas.iter().map(|(s, _)| s.as_str()).collect()
    }
    fn schema_owner(&self, schema: &str) -> Option<&str> {
        self.schemas.iter().find(|(s, _)| s == schema).map(|(_, owner)| owner.as_str())
    }
    fn create_schema(&mut self, schema: &str, owner: &str, tables: Vec<TableDesc>) {
        self.schemas.push((schema.to_owned(), owner.to_owned()));
        for t in tables {
            self.tables.insert(t.name.clone(), t);
        }
    }
    fn remove_schema(&mut self, schema: &str) {
        self.schemas.retain(|(s, _)| s != schema);
    }
    fn add_table(&mut self, desc: TableDesc) {
        self.tables.insert(desc.name.clone(), desc);
    }
    fn table(&self, name: &QualifiedName) -> Option<&TableDesc> {
        self.tables.get(name)
    }
    fn add_index(&mut self, index: IndexDesc) {
        self.indexes.push(index);
    }
    fn index(&self, name: &str) -> Option<&IndexDesc> {
        self.indexes.iter().find(|i| i.name == name)
    }
    fn remove_index(&mut self, name: &str) {
        self.indexes.retain(|i| i.name != name);
    }
    fn indexes(&self, table: &QualifiedName) -> Vec<&IndexDesc> {
        self.indexes.iter().filter(|i| &i.table == table).collect()
    }
    fn tables(&self) -> Vec<&TableDesc> {
        let mut tables: Vec<&TableDesc> = self.tables.values().collect();
        tables.sort_by_key(|t| t.name.to_string());
        tables
    }
    fn temporary_tables(&self) -> Vec<&TableDesc> {
        self.tables.values().filter(|t| t.temporary.is_some()).collect()
    }
    fn add_views(&mut self, views: Vec<ViewDesc>) {
        for v in views {
            self.views.insert(v.name.clone(), v);
        }
    }
    fn view(&self, name: &QualifiedName) -> Option<&ViewDesc> {
        self.views.get(name)
    }
    fn grant(&mut self, privileges: Vec<PrivilegeDesc>) {
        self.privileges.extend(privileges);
    }
    fn privileges(&self, table: &QualifiedName) -> Vec<&PrivilegeDesc> {
        self.privileges.iter().filter(|p| &p.table == table).collect()
    }
    fn views(&self) -> Vec<&ViewDesc> {
        let mut views: Vec<&ViewDesc> = self.views.values().collect();
        views.sort_by_key(|v| v.name.to_string());
        views
    }
    fn replace_table(&mut self, desc: TableDesc) {
        self.tables.insert(desc.name.clone(), desc);
    }
    fn remove_table(&mut self, name: &QualifiedName) {
        self.tables.remove(name);
    }
    fn remove_view(&mut self, name: &QualifiedName) {
        self.views.remove(name);
    }
    fn revoke_all(&mut self, table: &QualifiedName) {
        self.privileges.retain(|p| &p.table != table);
    }
    fn set_privileges(&mut self, table: &QualifiedName, privileges: Vec<PrivilegeDesc>) {
        self.revoke_all(table);
        self.privileges.extend(privileges);
    }
    fn put_domain(&mut self, domain: DomainDesc) {
        self.domains.insert(domain.name.clone(), domain);
    }
    fn domain(&self, name: &QualifiedName) -> Option<&DomainDesc> {
        self.domains.get(name)
    }
    fn domains(&self) -> Vec<&DomainDesc> {
        let mut domains: Vec<&DomainDesc> = self.domains.values().collect();
        domains.sort_by_key(|d| d.name.to_string());
        domains
    }
    fn put_collation(&mut self, collation: CollationDesc) {
        self.collations.insert(collation.name.clone(), collation);
    }
    fn collation(&self, name: &QualifiedName) -> Option<&CollationDesc> {
        self.collations.get(name)
    }
    fn collations(&self) -> Vec<&CollationDesc> {
        let mut collations: Vec<&CollationDesc> = self.collations.values().collect();
        collations.sort_by_key(|c| c.name.to_string());
        collations
    }
    fn remove_collation(&mut self, name: &QualifiedName) {
        self.collations.remove(name);
    }
    fn put_sequence(&mut self, sequence: SequenceDesc) {
        self.sequences.insert(sequence.name.clone(), sequence);
    }
    fn sequence(&self, name: &QualifiedName) -> Option<&SequenceDesc> {
        self.sequences.get(name)
    }
    fn sequences(&self) -> Vec<&SequenceDesc> {
        let mut sequences: Vec<&SequenceDesc> = self.sequences.values().collect();
        sequences.sort_by_key(|s| s.name.to_string());
        sequences
    }
    fn remove_sequence(&mut self, name: &QualifiedName) {
        self.sequences.remove(name);
    }
    fn put_user_type(&mut self, user_type: UserTypeDesc) {
        self.user_types.insert(user_type.name.clone(), user_type);
    }
    fn user_type(&self, name: &QualifiedName) -> Option<&UserTypeDesc> {
        self.user_types.get(name)
    }
    fn user_types(&self) -> Vec<&UserTypeDesc> {
        let mut user_types: Vec<&UserTypeDesc> = self.user_types.values().collect();
        user_types.sort_by_key(|t| t.name.to_string());
        user_types
    }
    fn remove_user_type(&mut self, name: &QualifiedName) {
        self.user_types.remove(name);
    }
    fn remove_domain(&mut self, name: &QualifiedName) {
        self.domains.remove(name);
    }
    fn put_assertion(&mut self, assertion: ConstraintDesc) {
        self.assertions.insert(assertion.name.clone(), assertion);
    }
    fn assertion(&self, name: &QualifiedName) -> Option<&ConstraintDesc> {
        self.assertions.get(name)
    }
    fn assertions(&self) -> Vec<&ConstraintDesc> {
        let mut assertions: Vec<&ConstraintDesc> = self.assertions.values().collect();
        assertions.sort_by_key(|a| a.name.to_string());
        assertions
    }
    fn remove_assertion(&mut self, name: &QualifiedName) {
        self.assertions.remove(name);
    }
    fn put_trigger(&mut self, trigger: TriggerDesc) {
        self.triggers.insert(trigger.name.clone(), trigger);
    }
    fn trigger(&self, name: &QualifiedName) -> Option<&TriggerDesc> {
        self.triggers.get(name)
    }
    fn any_triggers(&self) -> bool {
        !self.triggers.is_empty()
    }

    fn triggers(&self) -> Vec<&TriggerDesc> {
        let mut triggers: Vec<&TriggerDesc> = self.triggers.values().collect();
        triggers.sort_by_key(|t| t.created);
        triggers
    }
    fn triggers_on(&self, table: &QualifiedName) -> Vec<&TriggerDesc> {
        // No trigger, the common case, costs a statement nothing more.
        if self.triggers.is_empty() {
            return Vec::new();
        }
        let mut triggers: Vec<&TriggerDesc> = self.triggers.values().filter(|t| t.table == *table).collect();
        triggers.sort_by_key(|t| t.created);
        triggers
    }
    fn remove_trigger(&mut self, name: &QualifiedName) {
        self.triggers.remove(name);
    }
    fn put_routine(&mut self, routine: SqlRoutineDesc) {
        self.routines.insert(routine.specific.clone(), routine);
    }
    fn routine(&self, specific: &QualifiedName) -> Option<&SqlRoutineDesc> {
        self.routines.get(specific)
    }
    fn any_routines(&self) -> bool {
        !self.routines.is_empty()
    }
    fn routines(&self) -> Vec<&SqlRoutineDesc> {
        let mut routines: Vec<&SqlRoutineDesc> = self.routines.values().collect();
        routines.sort_by_key(|r| r.created);
        routines
    }
    fn routines_named(&self, name: &QualifiedName) -> Vec<&SqlRoutineDesc> {
        // No routine, the common case, costs a function call nothing more.
        if self.routines.is_empty() {
            return Vec::new();
        }
        let mut routines: Vec<&SqlRoutineDesc> = self.routines.values().filter(|r| r.name == *name).collect();
        routines.sort_by_key(|r| r.created);
        routines
    }
    fn remove_routine(&mut self, specific: &QualifiedName) {
        self.routines.remove(specific);
    }
}
