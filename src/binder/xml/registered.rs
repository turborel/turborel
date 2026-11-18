//! The registered XML Schemas and XQuery modules a query's static context
//! takes [ISO/IEC 9075-14:2006 draft §10.20 SR2e] [XQuery §4.10, §4.11]:
//! those a cell may use, or those a definition kept, as a statement is
//! bound or a registered XQuery module is analysed; and of each registered
//! XQuery module, what it was registered with (`xquery::LibraryModule`).
//!
//! \[DESIGN\] Implementation-defined, RULES.md D106: of the registered XML
//! Schemas, in the order of their names, then the permanently registered
//! ones; of the registered XQuery modules, in the order of their names,
//! and those they import at any remove, each as it was registered, found
//! by a walk of their imports, each once, so it ends with the catalog's
//! modules.

use crate::catalog::xml_schemas::{PERMANENT, may_use};
use crate::catalog::{Catalog, QualifiedName, Who, XQueryModuleDesc, xquery_modules};
use crate::xml::schema::permanent::components;
use crate::xquery::{InScopeSchema, LibraryModule};

/// What a query's static context takes of the registered XML Schemas and
/// XQuery modules, each with its name.
pub(crate) struct Registered {
    /// The registered XML Schemas a schema import may import, as
    /// `Environment::importable` takes them.
    pub(crate) importable: Vec<(QualifiedName, InScopeSchema)>,
    /// The registered XQuery modules a module import of the main module
    /// may import, as `Environment::modules` takes them.
    pub(crate) modules: Vec<(QualifiedName, LibraryModule)>,
    /// Those they import, at any remove, as `Environment::linked` takes
    /// them.
    pub(crate) linked: Vec<LibraryModule>,
}

/// What `catalog` gives a query's static context bound as the cell `who`
/// [ISO/IEC 9075-14:2006 draft §10.20 SR2e] [XQuery §4.10, §4.11]: the
/// registered XML Schemas and XQuery modules `who` may use, of `pins`, the
/// schemas and modules a definition kept, alone if given.
pub(crate) fn registered(catalog: &dyn Catalog, who: Who<'_>, pins: Option<(&[QualifiedName], &[QualifiedName])>) -> Registered {
    let schemas = |name: &QualifiedName| pins.is_none_or(|(kept, _)| kept.contains(name)) && may_use(catalog, name, who).is_ok();
    let importable = importable_schemas(catalog, &schemas);
    let usable =
        |name: &QualifiedName| pins.is_none_or(|(_, kept)| kept.contains(name)) && xquery_modules::may_use(catalog, name, who).is_ok();
    let modules: Vec<(QualifiedName, LibraryModule)> = catalog
        .xquery_modules()
        .into_iter()
        .filter(|m| usable(&m.name))
        .filter_map(|m| Some((m.name.clone(), library_module(catalog, m)?)))
        .collect();
    let linked = linked(catalog, modules.iter().map(|(n, _)| n));
    Registered { importable, modules, linked }
}

/// The registered XML Schemas of `catalog` `kept` takes, each with its
/// name: of the catalog's, in the order of their names, then the
/// permanently registered ones.
pub(crate) fn importable_schemas(catalog: &dyn Catalog, kept: &dyn Fn(&QualifiedName) -> bool) -> Vec<(QualifiedName, InScopeSchema)> {
    let mut usable: Vec<_> = catalog.xml_schemas().into_iter().filter(|d| kept(&d.name)).collect();
    usable.sort_by(|a, b| a.name.cmp(&b.name));
    let held = usable.into_iter().filter_map(|d| {
        let schema = d.schema.clone()?;
        let read = InScopeSchema { namespace: d.target_namespace.clone(), name: d.name.to_string(), location: d.location.clone(), schema };
        Some((d.name.clone(), read))
    });
    let lasting = PERMANENT.iter().filter_map(|p| {
        let name = p.qualified();
        let schema = components(p.target_namespace).filter(|_| kept(&name))?;
        let read =
            InScopeSchema { namespace: p.target_namespace.to_owned(), name: name.to_string(), location: p.location.to_owned(), schema };
        Some((name, read))
    });
    held.chain(lasting).collect()
}

/// [§10.20 SR2e] The in-scope schema definitions, implementation-defined
/// (RULES.md D106): of each target namespace, the first of `importable`,
/// so the registered XML Schema of the least name the current user may
/// use, as XMLVALIDATE's data-driven validation chooses, or the
/// permanently registered one of a namespace none of those has.
pub(crate) fn in_scope(importable: &[(QualifiedName, InScopeSchema)]) -> Vec<(QualifiedName, InScopeSchema)> {
    let mut out: Vec<(QualifiedName, InScopeSchema)> = Vec::new();
    for (name, schema) in importable {
        if !out.iter().any(|(_, s)| s.namespace == schema.namespace) {
            out.push((name.clone(), schema.clone()));
        }
    }
    out
}

/// The registered XQuery module `desc` of `catalog`, as a module import
/// finds it: its parse, and the modules and schemas it kept as it was
/// registered; none of one not parsed, which no catalog read holds.
fn library_module(catalog: &dyn Catalog, desc: &XQueryModuleDesc) -> Option<LibraryModule> {
    let importable = importable_schemas(catalog, &|name| desc.xml_schemas.contains(name));
    let schemas = in_scope(&importable).into_iter().map(|(_, s)| s).collect();
    Some(LibraryModule {
        name: desc.name.to_string(),
        ident: desc.name.ident.clone(),
        location: desc.location.clone(),
        module: desc.module.clone()?,
        modules: desc.modules.iter().map(ToString::to_string).collect(),
        importable: importable.into_iter().map(|(_, s)| s).collect(),
        schemas,
    })
}

/// The registered XQuery modules of `catalog` those named `from` import,
/// at any remove, each once.
fn linked<'a>(catalog: &dyn Catalog, from: impl Iterator<Item = &'a QualifiedName>) -> Vec<LibraryModule> {
    let mut names: Vec<QualifiedName> = Vec::new();
    let mut pending: Vec<QualifiedName> = from.cloned().collect();
    while let Some(name) = pending.pop() {
        let Some(desc) = catalog.xquery_module(&name) else { continue };
        for imported in &desc.modules {
            if !names.contains(imported) {
                names.push(imported.clone());
                pending.push(imported.clone());
            }
        }
    }
    names.iter().filter_map(|n| catalog.xquery_module(n)).filter_map(|d| library_module(catalog, d)).collect()
}
