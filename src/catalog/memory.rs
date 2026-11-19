//! \[DESIGN\] `MemoryCatalog`, the built-in catalog, held in memory.

use std::collections::HashMap;

use super::{
    CastDesc, Catalog, CollationDesc, ConstraintDesc, DomainDesc, Embeddings, GqlCatalog, GraphDesc, IndexDesc, ModuleDesc, OrderingDesc,
    Partitioning, PrivilegeDesc, QualifiedName, Roles, SequenceDesc, SpatialCatalog, SqlRoutineDesc, StructuredDesc, TableDesc,
    TransformGroupDesc, TriggerDesc, UserTypeDesc, ViewDesc, XQueryModuleDesc, XmlSchemaDesc,
};
use crate::hash::murmur::MurmurMap;
use crate::value::DataType;

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
    /// Each registered XML Schema, by its name.
    xml_schemas: HashMap<QualifiedName, XmlSchemaDesc>,
    /// Each registered XQuery module, by its name.
    xquery_modules: HashMap<QualifiedName, XQueryModuleDesc>,
    sequences: HashMap<QualifiedName, SequenceDesc>,
    user_types: HashMap<QualifiedName, UserTypeDesc>,
    /// Each structured type, by its name.
    structured_types: HashMap<QualifiedName, StructuredDesc>,
    /// The user-defined casts, in the order made.
    casts: Vec<CastDesc>,
    /// Each user-defined ordering, by its type's name.
    orderings: HashMap<QualifiedName, OrderingDesc>,
    /// Each type's transform groups, by its name.
    transforms: HashMap<QualifiedName, Vec<TransformGroupDesc>>,
    assertions: HashMap<QualifiedName, ConstraintDesc>,
    triggers: HashMap<QualifiedName, TriggerDesc>,
    /// Each SQL-invoked routine, by its specific name.
    routines: HashMap<QualifiedName, SqlRoutineDesc>,
    /// Each SQL-server module, by its name.
    modules: HashMap<QualifiedName, ModuleDesc>,
    /// Each property graph, by its name.
    graphs: HashMap<QualifiedName, GraphDesc>,
    /// The graph types and GQL graphs.
    gql: GqlCatalog,
    /// The roles and role authorizations.
    roles: Roles,
    /// The embedding models and vector pipelines.
    embeddings: Embeddings,
    /// The spatial reference systems defined and the geometry columns
    /// registered.
    spatial: SpatialCatalog,
    /// The partitioned tables and partitions.
    partitioning: Partitioning,
    /// The extensions created.
    extensions: Vec<super::ExtensionDesc>,
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
    fn embeddings(&self) -> &Embeddings {
        &self.embeddings
    }
    fn set_embeddings(&mut self, embeddings: Embeddings) {
        self.embeddings = embeddings;
    }
    fn extensions(&self) -> &[super::ExtensionDesc] {
        &self.extensions
    }
    fn set_extensions(&mut self, extensions: Vec<super::ExtensionDesc>) {
        self.extensions = extensions;
    }
    fn spatial(&self) -> &SpatialCatalog {
        &self.spatial
    }
    fn set_spatial(&mut self, spatial: SpatialCatalog) {
        self.spatial = spatial;
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
        // [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §20.2 COLUMN_EXISTS]
        let kept: Vec<&str> = desc.columns.iter().map(|c| c.name.as_str()).collect();
        self.spatial.forget_columns(&desc.name, &kept);
        self.tables.insert(desc.name.clone(), desc);
    }
    fn remove_table(&mut self, name: &QualifiedName) {
        self.spatial.forget_columns(name, &[]);
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
    fn put_xml_schema(&mut self, xml_schema: XmlSchemaDesc) {
        self.xml_schemas.insert(xml_schema.name.clone(), xml_schema);
    }
    fn xml_schema(&self, name: &QualifiedName) -> Option<&XmlSchemaDesc> {
        self.xml_schemas.get(name)
    }
    fn xml_schemas(&self) -> Vec<&XmlSchemaDesc> {
        let mut all: Vec<&XmlSchemaDesc> = self.xml_schemas.values().collect();
        all.sort_by_key(|x| x.name.to_string());
        all
    }
    fn remove_xml_schema(&mut self, name: &QualifiedName) {
        self.xml_schemas.remove(name);
    }
    fn put_xquery_module(&mut self, module: XQueryModuleDesc) {
        self.xquery_modules.insert(module.name.clone(), module);
    }
    fn xquery_module(&self, name: &QualifiedName) -> Option<&XQueryModuleDesc> {
        self.xquery_modules.get(name)
    }
    fn xquery_modules(&self) -> Vec<&XQueryModuleDesc> {
        let mut all: Vec<&XQueryModuleDesc> = self.xquery_modules.values().collect();
        all.sort_by_key(|m| m.name.to_string());
        all
    }
    fn remove_xquery_module(&mut self, name: &QualifiedName) {
        self.xquery_modules.remove(name);
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
    fn put_structured_type(&mut self, structured: StructuredDesc) {
        self.structured_types.insert(structured.name.clone(), structured);
    }
    fn structured_type(&self, name: &QualifiedName) -> Option<&StructuredDesc> {
        self.structured_types.get(name)
    }
    fn structured_types(&self) -> Vec<&StructuredDesc> {
        let mut all: Vec<&StructuredDesc> = self.structured_types.values().collect();
        all.sort_by_key(|t| t.name.to_string());
        all
    }
    fn remove_structured_type(&mut self, name: &QualifiedName) {
        self.structured_types.remove(name);
    }
    fn put_cast(&mut self, cast: CastDesc) {
        self.remove_cast(&cast.source, &cast.target);
        self.casts.push(cast);
    }
    fn casts(&self) -> Vec<&CastDesc> {
        self.casts.iter().collect()
    }
    fn remove_cast(&mut self, source: &DataType, target: &DataType) {
        self.casts.retain(|c| c.source != *source || c.target != *target);
    }
    fn put_ordering(&mut self, ordering: OrderingDesc) {
        self.orderings.insert(ordering.udt.clone(), ordering);
    }
    fn ordering(&self, udt: &QualifiedName) -> Option<&OrderingDesc> {
        self.orderings.get(udt)
    }
    fn orderings(&self) -> Vec<&OrderingDesc> {
        let mut all: Vec<&OrderingDesc> = self.orderings.values().collect();
        all.sort_by_key(|o| o.udt.to_string());
        all
    }
    fn remove_ordering(&mut self, udt: &QualifiedName) {
        self.orderings.remove(udt);
    }
    fn put_transforms(&mut self, udt: &QualifiedName, groups: Vec<TransformGroupDesc>) {
        if groups.is_empty() {
            self.transforms.remove(udt);
        } else {
            self.transforms.insert(udt.clone(), groups);
        }
    }
    fn transforms(&self, udt: &QualifiedName) -> &[TransformGroupDesc] {
        self.transforms.get(udt).map_or(&[], Vec::as_slice)
    }
    fn all_transforms(&self) -> Vec<(&QualifiedName, &[TransformGroupDesc])> {
        let mut all: Vec<(&QualifiedName, &[TransformGroupDesc])> = self.transforms.iter().map(|(n, g)| (n, g.as_slice())).collect();
        all.sort_by_key(|(n, _)| n.to_string());
        all
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
    fn put_module(&mut self, module: ModuleDesc) {
        self.modules.insert(module.name.clone(), module);
    }
    fn module(&self, name: &QualifiedName) -> Option<&ModuleDesc> {
        self.modules.get(name)
    }
    fn modules(&self) -> Vec<&ModuleDesc> {
        let mut modules: Vec<&ModuleDesc> = self.modules.values().collect();
        modules.sort_by_key(|m| m.created);
        modules
    }
    fn remove_module(&mut self, name: &QualifiedName) {
        self.modules.remove(name);
    }
    fn put_graph(&mut self, graph: GraphDesc) {
        self.graphs.insert(graph.name.clone(), graph);
    }
    fn graph(&self, name: &QualifiedName) -> Option<&GraphDesc> {
        self.graphs.get(name)
    }
    fn graphs(&self) -> Vec<&GraphDesc> {
        let mut graphs: Vec<&GraphDesc> = self.graphs.values().collect();
        graphs.sort_by(|a, b| a.name.cmp(&b.name));
        graphs
    }
    fn remove_graph(&mut self, name: &QualifiedName) {
        self.graphs.remove(name);
    }
    fn gql(&self) -> &GqlCatalog {
        &self.gql
    }
    fn gql_mut(&mut self) -> &mut GqlCatalog {
        &mut self.gql
    }
    fn partitioning(&self) -> &Partitioning {
        &self.partitioning
    }
    fn partitioning_mut(&mut self) -> &mut Partitioning {
        &mut self.partitioning
    }
}
