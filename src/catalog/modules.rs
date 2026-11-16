//! The descriptor of an SQL-server module [ISO/IEC 9075-4:2006 draft §9.21
//! GR3] (Feature P001).
//!
//! A module's declared local temporary tables are tables of the catalog in
//! a schema of its own, which no `<schema name>` can name [§12.8 GR1b]: its
//! name holds a newline, which no `<delimited identifier>` does [ISO/IEC
//! 9075:1992 §5.2 SR3]. Within the module, MODULE.T names one of them
//! (`module_schema`).

use super::{Catalog, QualifiedName, SqlRoutineDesc, UsageDesc, Who};

/// What separates the parts of the name of a module's own schema.
const SEPARATOR: char = '\n';

/// [ISO/IEC 9075-4:2006 draft §9.21 GR3] An SQL-server module descriptor.
#[derive(Debug, Clone)]
pub struct ModuleDesc {
    /// Its `<SQL-server module name>`, of its schema \[GR3a\].
    pub name: QualifiedName,
    /// Its default schema name, of the names its routines write without a
    /// schema \[GR3c\].
    pub schema: String,
    /// The schemas of its SQL-path, in order \[GR3e\]: those written, else
    /// its own schema's alone, the implementation-defined path SR4 leaves.
    pub path: Vec<String>,
    /// Its `<SQL-server module path specification>` as written, if it was.
    pub path_text: Option<String>,
    /// The names of its declared local temporary tables, in its own
    /// schema \[GR3f\].
    pub temporaries: Vec<QualifiedName>,
    /// Its `<SQL-server module definition>` as written \[GR3h\].
    pub text: String,
    /// The EXECUTE privileges on it [§10.2], its owner's by `_SYSTEM`
    /// \[GR2\].
    pub execute: Vec<UsageDesc>,
    /// \[DESIGN\] Its timestamp of creation, as an order: one more than the
    /// greatest of the modules there were as it was created.
    pub created: u64,
}

impl ModuleDesc {
    /// [ISO/IEC 9075-4:2006 draft §10.2] Whether `who` holds EXECUTE on it,
    /// and whether grantably: as granted an identifier it enables or
    /// PUBLIC, its owner by the grant of `_SYSTEM` its definition made.
    pub fn execute_of(&self, who: Who<'_>) -> (bool, bool) {
        let granted = self.execute.iter().filter(|u| who.holds(u.grantee.as_deref()));
        granted.fold((false, false), |(_, grantable), u| (true, grantable || u.grantable))
    }

    /// The name of its own schema, of its declared local temporary tables.
    pub fn own_schema(&self) -> String {
        module_schema(&self.name)
    }
}

/// [ISO/IEC 9075-4:2006 draft §12.8 GR1b] The name of the schema of the
/// declared local temporary tables of the module `module`, which MODULE.T
/// names within it.
pub fn module_schema(module: &QualifiedName) -> String {
    format!("MODULE{SEPARATOR}{}{SEPARATOR}{}", module.auth, module.ident)
}

/// The module whose own schema is `schema`, if it is one's.
pub fn module_of_schema(schema: &str) -> Option<QualifiedName> {
    let rest = schema.strip_prefix("MODULE")?.strip_prefix(SEPARATOR)?;
    let (auth, ident) = rest.split_once(SEPARATOR)?;
    (!ident.contains(SEPARATOR)).then(|| QualifiedName { auth: auth.to_owned(), ident: ident.to_owned() })
}

/// [ISO/IEC 9075-4:2006 draft §9.21 SR6] The SQL-path of the body of the
/// routine `r`: its module's, if it is of one; else none, its names of
/// routines of its own schema.
pub fn routine_path(catalog: &dyn Catalog, r: &SqlRoutineDesc) -> Vec<String> {
    r.module_name().and_then(|m| catalog.module(&m)).map(|m| m.path.clone()).unwrap_or_default()
}
