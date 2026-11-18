//! The schema documents a schema is read from [Schema1 §4.2]: the one
//! given, and those it includes, redefines and imports, found in the
//! library and each read once [Schema1 §4.2.1 NOTE].
//!
//! \[DESIGN\] A schema location that names no document of the library,
//! and an import of a namespace no document of the library has, add no
//! components, as a reference that fails to resolve adds none [Schema1
//! §4.2.1, §4.2.3]; an import of the XML Schema namespace adds none, as
//! its components are built in. A `<redefine>` with definitions must
//! name a document [Schema1 §4.2.2 src-redefine.1].

use std::sync::Arc;

use super::super::component::Derivations;
use super::super::{MAX_DOCUMENTS, beyond, invalid};
use super::attrs::At;
use super::{Library, Source};
use crate::error::Result;
use crate::xml::{Kind, NodeId, Tree, XS_NAMESPACE};

/// [Schema1 §3.15.2] The document element of a schema document.
const SCHEMA: &str = "schema";
/// [Schema1 §4.2.1] An inclusion.
const INCLUDE: &str = "include";
/// [Schema1 §4.2.2] A redefinition.
const REDEFINE: &str = "redefine";
/// [Schema1 §4.2.3] An import.
const IMPORT: &str = "import";
/// [Schema1 §3.13.2] An annotation.
const ANNOTATION: &str = "annotation";
/// [Schema1 §3.2.2, §3.3.2] The value of a form of qualified names.
const QUALIFIED: &str = "qualified";
/// [Schema1 §3.2.2, §3.3.2] The value of a form of unqualified names.
const UNQUALIFIED: &str = "unqualified";

/// A schema document being read, and what its `<schema>` element says of
/// the components it represents [Schema1 §3.15.2].
#[derive(Debug, Clone)]
pub(super) struct Doc {
    /// Its tree.
    pub tree: Arc<Tree>,
    /// Its schema location.
    pub location: Option<Box<str>>,
    /// Its `<schema>` element.
    pub root: NodeId,
    /// The target namespace of its components: its targetNamespace, or
    /// that of the document including or redefining it if it has none
    /// [Schema1 §4.2.1 clause 3.2].
    pub tns: Option<Box<str>>,
    /// Whether it has no targetNamespace and is read into another's
    /// namespace, so an absent namespace name in its references is that
    /// namespace [Schema1 §4.2.1 clause 3.2, NOTE].
    pub chameleon: bool,
    /// attributeFormDefault is qualified.
    pub attributes_qualified: bool,
    /// elementFormDefault is qualified.
    pub elements_qualified: bool,
    /// blockDefault, of every method.
    pub block_default: Derivations,
    /// finalDefault, of every method.
    pub final_default: Derivations,
    /// The namespaces it imports [Schema1 §3.15.3 clause 4].
    pub imports: Vec<Option<Box<str>>>,
    /// The `<redefine>` elements of it, each with the place of the
    /// document it redefines.
    pub redefines: Vec<(NodeId, usize)>,
}

/// The target namespace of a document read: its own, or that of the
/// document including or redefining it.
#[derive(Debug, Clone)]
enum Reached {
    /// Its own targetNamespace.
    Own,
    /// The includer's, which it must have if it has one [Schema1 §4.2.1].
    Into(Option<Box<str>>),
}

/// A document to read, as it was reached: its source, and the target
/// namespace an including or redefining document gives it.
struct Pending<'a> {
    source: &'a Source,
    into: Reached,
    /// The document redefining it, and the `<redefine>` element.
    by: Option<(usize, NodeId)>,
}

/// The documents of the schema `primary` corresponds to [Schema1 §4.2]:
/// it first, then each it reaches, each once.
///
/// # Errors
///
/// SQLSTATE 22000 if one is no schema document, or an inclusion,
/// redefinition or import breaks its constraints [Schema1 §4.2.1,
/// §4.2.2, §4.2.3]; SQLCODE -900 past `MAX_DOCUMENTS`.
pub(super) fn gather(primary: &Source, library: &Library) -> Result<Vec<Doc>> {
    let mut docs: Vec<Doc> = Vec::new();
    let mut seen: Vec<(Arc<Tree>, Option<Box<str>>)> = Vec::new();
    let mut queue = vec![Pending { source: primary, into: Reached::Own, by: None }];
    while let Some(next) = queue.pop() {
        let mut doc = open(next.source, next.into)?;
        let found = seen.iter().position(|(tree, tns)| Arc::ptr_eq(tree, &doc.tree) && *tns == doc.tns);
        let place = if let Some(place) = found {
            place
        } else {
            {
                if docs.len() >= MAX_DOCUMENTS {
                    return Err(beyond("it is read from too many schema documents"));
                }
                seen.push((Arc::clone(&doc.tree), doc.tns.clone()));
                let place = docs.len();
                queue.extend(reached(&mut doc, place, library)?);
                docs.push(doc);
                place
            }
        };
        if let Some((by, node)) = next.by {
            docs.get_mut(by).into_iter().for_each(|d| d.redefines.push((node, place)));
        }
    }
    Ok(docs)
}

/// `source` read as a schema document, its components of namespace `into`
/// if it is included in or redefined by another.
///
/// # Errors
///
/// SQLSTATE 22000 if its document element is no `<schema>`, its
/// targetNamespace is empty [Schema1 §3.15.2], or it has a targetNamespace
/// other than `into` [Schema1 §4.2.1 src-include.2.1].
fn open(source: &Source, into: Reached) -> Result<Doc> {
    let tree = &source.tree;
    let root = tree.children(NodeId::ROOT).find(|&c| tree.kind(c) == Some(Kind::Element)).unwrap_or(NodeId::ROOT);
    let at = At::new(tree, root);
    if at.xs_name() != Some(SCHEMA) {
        return Err(invalid("[Schema1 §3.15.2]", "a schema document's element is a <schema> of the XML Schema namespace"));
    }
    let own: Option<Box<str>> = at.attr("targetNamespace").map(Into::into);
    if own.as_deref() == Some("") {
        return Err(invalid("[Schema1 §3.15.2]", "a targetNamespace is never empty"));
    }
    if let (Some(own), Reached::Into(into)) = (&own, &into)
        && into.as_deref() != Some(&**own)
    {
        return Err(invalid(
            "[Schema1 §4.2.1 src-include.2.1]",
            "an included or redefined document has the target namespace of its includer",
        ));
    }
    let into = match into {
        Reached::Own => None,
        Reached::Into(into) => into,
    };
    let chameleon = own.is_none() && into.is_some();
    let tns = own.or(into);
    Ok(Doc {
        location: source.location.clone(),
        attributes_qualified: form(&at, "attributeFormDefault")?,
        elements_qualified: form(&at, "elementFormDefault")?,
        block_default: at.derivations(
            "blockDefault",
            Derivations::EXTENSION.with(Derivations::RESTRICTION).with(Derivations::SUBSTITUTION),
            Derivations::NONE,
        )?,
        final_default: at.derivations_all(
            "finalDefault",
            (Derivations::EXTENSION.with(Derivations::RESTRICTION), Derivations::SIMPLE_FINAL),
            Derivations::NONE,
        )?,
        tree: Arc::clone(tree),
        root,
        tns,
        chameleon,
        imports: Vec::new(),
        redefines: Vec::new(),
    })
}

/// Whether the form attribute `local` of `at` is qualified [Schema1
/// §3.15.2]: unqualified if absent.
///
/// # Errors
///
/// SQLSTATE 22000 if it is neither form.
pub(super) fn form(at: &At<'_>, local: &str) -> Result<bool> {
    match at.token(local).as_deref() {
        None | Some(UNQUALIFIED) => Ok(false),
        Some(QUALIFIED) => Ok(true),
        Some(other) => Err(at.bad(local, other)),
    }
}

/// The documents `doc`, at `place`, includes, redefines and imports from
/// `library` [Schema1 §4.2], and the namespaces it imports noted.
///
/// # Errors
///
/// As `include`, `redefine` and `import`.
fn reached<'a>(doc: &mut Doc, place: usize, library: &'a Library) -> Result<Vec<Pending<'a>>> {
    let tree = Arc::clone(&doc.tree);
    let at = At::new(&tree, doc.root);
    let mut out = Vec::new();
    for (child, local) in at.children()? {
        match local {
            INCLUDE => out.extend(include(&child, doc, library)?),
            REDEFINE => out.extend(redefine(&child, doc, place, library)?),
            IMPORT => {
                let (namespace, found) = import(&child, at.attr("targetNamespace"), library)?;
                doc.imports.push(namespace);
                out.extend(found);
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The schema location of `<include>` or `<redefine>` `at`, which it must
/// have [Schema1 §4.2.1, Appendix A].
///
/// # Errors
///
/// SQLSTATE 22000 if it has none.
fn location<'t>(at: &At<'t>) -> Result<&'t str> {
    at.attr("schemaLocation").ok_or_else(|| invalid("[Schema1 Appendix A]", &format!("<{}> has a schemaLocation", at.local())))
}

/// The document `<include>` `at` of `doc` names, if the library has it,
/// read into `doc`'s namespace [Schema1 §4.2.1].
///
/// # Errors
///
/// As `location`.
fn include<'a>(at: &At<'_>, doc: &Doc, library: &'a Library) -> Result<Option<Pending<'a>>> {
    let found = library.at(location(at)?);
    Ok(found.map(|source| Pending { source, into: Reached::Into(doc.tns.clone()), by: None }))
}

/// The document `<redefine>` `at` of `doc`, at `place`, names, read into
/// `doc`'s namespace [Schema1 §4.2.2].
///
/// # Errors
///
/// SQLSTATE 22000 if it has a definition but names no document of the
/// library [Schema1 §4.2.2 src-redefine.1].
fn redefine<'a>(at: &At<'_>, doc: &Doc, place: usize, library: &'a Library) -> Result<Option<Pending<'a>>> {
    let found = library.at(location(at)?);
    let defines = at.children()?.iter().any(|(_, local)| *local != ANNOTATION);
    if found.is_none() && defines {
        return Err(invalid("[Schema1 §4.2.2 src-redefine.1]", "a <redefine> with definitions names a registered schema document"));
    }
    Ok(found.map(|source| Pending { source, into: Reached::Into(doc.tns.clone()), by: Some((place, at.node)) }))
}

/// The target namespace a schema document `source` declares, if any.
fn declared_namespace(source: &Source) -> Option<&str> {
    tree_namespace(&source.tree)
}

/// The target namespace the schema document `tree` declares, if any.
pub(in crate::xml::schema) fn tree_namespace(tree: &Tree) -> Option<&str> {
    tree.children(NodeId::ROOT).find(|&c| tree.kind(c) == Some(Kind::Element)).and_then(|c| At::new(tree, c).attr("targetNamespace"))
}

/// The namespace `<import>` `at` imports into a document of target
/// namespace `own`, and the document of the library it finds for it
/// [Schema1 §4.2.3]: the one at its schema location if that has the
/// namespace, else the first that has; none of the XML Schema namespace.
///
/// # Errors
///
/// SQLSTATE 22000 if it imports the document's own namespace [Schema1
/// §4.2.3 src-import.1.1], no namespace into a document of none \[1.2\],
/// or its schema location names a document of another namespace \[3.1,
/// 3.2\].
fn import<'a>(at: &At<'_>, own: Option<&str>, library: &'a Library) -> Result<(Option<Box<str>>, Option<Pending<'a>>)> {
    let namespace = at.attr("namespace");
    if namespace == own {
        let clause = if own.is_some() { "src-import.1.1" } else { "src-import.1.2" };
        return Err(invalid(&format!("[Schema1 §4.2.3 {clause}]"), "a document imports a namespace other than its own"));
    }
    if let Some(source) = at.attr("schemaLocation").and_then(|l| library.at(l))
        && declared_namespace(source) != namespace
    {
        return Err(invalid("[Schema1 §4.2.3 src-import.3.1]", "an imported document has the namespace imported"));
    }
    let found = if namespace == Some(XS_NAMESPACE) {
        None
    } else {
        let at_location = at.attr("schemaLocation").and_then(|l| library.at(l));
        at_location.or_else(|| library.sources().iter().find(|s| declared_namespace(s) == namespace))
    };
    Ok((namespace.map(Into::into), found.map(|source| Pending { source, into: Reached::Own, by: None })))
}
