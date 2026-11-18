//! \[EXTENSION\] The element tables of a GRAPH_TABLE's graph as its
//! search reads them [GS §4.1, §8]: each one's key, ends, labels and its
//! properties' expressions bound over a row of it, as the graph's owner
//! binds them (`binder/graph_properties.rs`), or the querier of a graph
//! WITH INVOKER RIGHTS.
//!
//! A base table is read where it is stored, a row by its id. A view, and
//! every element table of a GRAPH_TABLE read FOR SYSTEM_TIME, is read by
//! the plan of a query of it, as the FROM clause would read it, whose rows
//! the search holds for the statement.

use std::collections::BTreeSet;

use super::super::from::{Bounds, from_item, leaf};
use super::super::{Binder, Context, Scope};
use super::grf;
use crate::ast::{NamedTable, SystemTimeRead, TableName};
use crate::catalog::{ElementKind, ElementTableDesc, EndpointDesc, GraphDesc, Principal, TableDesc, owner_of};
use crate::error::Result;
use crate::logical::{EdgeShape, ElementRead, ElementSource, EndShape, Expr};
use crate::parser::Parser;
use crate::value::DataType;

/// The rule a set function in a property's expression breaks.
const ONE_ROW: &str = "[EXTENSION] a property is of one row of its element table, so its expression holds no set function (§4.1 R4.5)";

/// An element table as the binder sees it: its labels and properties.
pub(super) struct TableInfo {
    /// Whether it is an edge table.
    pub(super) edge: bool,
    /// Its labels' names.
    pub(super) labels: Vec<String>,
    /// Its properties: name and type.
    pub(super) properties: Vec<(String, DataType)>,
}

/// The context the element tables of `desc` are bound in: its owner's,
/// of the names and columns it was defined with; the querier's of a graph
/// WITH INVOKER RIGHTS.
pub(super) fn owner_context<'a>(cx: Context<'a>, desc: &'a GraphDesc) -> Context<'a> {
    if desc.invoker {
        return cx;
    }
    let owner = owner_of(&*cx.db.catalog, &desc.name.auth);
    let user = Principal::sole(cx.db.catalog.roles(), owner);
    Context { auth: owner, role: None, schema: &desc.names, user, horizon: &desc.horizon, ..cx }
}

/// The element tables of `desc`, numbered as it lists them, read FOR
/// SYSTEM_TIME as `read` says if written; and the graph's labels.
///
/// # Errors
///
/// SQLCODE -103 if an element table or a property expression no longer
/// binds; -104 if a privilege it needs is not held.
pub(super) fn element_tables(
    b: Bounds<'_>,
    desc: &GraphDesc,
    read: Option<&SystemTimeRead>,
) -> Result<(Vec<ElementSource>, Vec<TableInfo>, Vec<String>)> {
    let cx = owner_context(b.cx, desc);
    let mut labels: Vec<String> = Vec::new();
    for e in &desc.elements {
        for l in &e.labels {
            if !labels.contains(&l.name) {
                labels.push(l.name.clone());
            }
        }
    }
    let (mut sources, mut infos) = (Vec::new(), Vec::new());
    for e in &desc.elements {
        let (source, info) = element_table(Bounds { cx, clause: b.clause, outer: None }, desc, e, (read, &labels))?;
        sources.push(source);
        infos.push(info);
    }
    Ok((sources, infos, labels))
}

/// Element table `e` of `desc`.
fn element_table(
    b: Bounds<'_>,
    desc: &GraphDesc,
    e: &ElementTableDesc,
    (as_of, labels): (Option<&SystemTimeRead>, &[String]),
) -> Result<(ElementSource, TableInfo)> {
    let named = NamedTable {
        table: TableName { auth: Some(e.table.auth.clone()), ident: e.table.ident.clone(), at: 0 },
        only: false,
        correlation: None,
        columns: None,
        system_time: as_of.cloned().map(Box::new),
    };
    let item = from_item(b, &named, 0)?;
    let columns = item.desc.clone();
    let stored = as_of.is_none() && b.cx.db.catalog.table(&e.table).is_some() && item.family.is_empty();
    let read = if stored { ElementRead::Stored } else { held(b.cx, &item)? };
    let position = |name: &String| -> Result<usize> {
        columns
            .columns
            .iter()
            .position(|c| &c.name == name)
            .ok_or_else(|| grf("GRF-0005", format!("element table {} has no column {name}", e.alias)))
    };
    let key = e.key.iter().map(position).collect::<Result<Vec<_>>>()?;
    let ends = match &e.endpoints {
        Some(ends) => Some(EdgeShape {
            source: end_shape(b.cx, desc, &ends.source, &position)?,
            destination: end_shape(b.cx, desc, &ends.destination, &position)?,
            directed: !ends.undirected,
        }),
        None => None,
    };
    let (properties, info) = properties_of(b.cx, e, &columns)?;
    let ids = e.labels.iter().filter_map(|l| labels.iter().position(|n| n == &l.name)).filter_map(|i| u32::try_from(i).ok()).collect();
    let adjacency = match (&read, &ends) {
        (ElementRead::Stored, Some(ends)) => adjacency_index(b.cx, &e.table, ends),
        (ElementRead::Stored | ElementRead::Held(_), _) => None,
    };
    let width = columns.columns.len();
    let source =
        ElementSource { alias: e.alias.clone(), table: e.table.clone(), ends, key, labels: ids, properties, read, width, adjacency };
    Ok((source, info))
}

/// The plan of the rows of element table `item`, read into the
/// statement: of a view, or of a table read FOR SYSTEM_TIME.
fn held(cx: Context<'_>, item: &super::super::FromItem) -> Result<ElementRead> {
    let width = item.desc.columns.len();
    let visible = (0..width).map(|k| (0, k)).collect();
    let binder = Binder::new(cx, None, (vec![item.clone()], item.row_names(), visible), BTreeSet::new());
    binder.used.borrow_mut().extend(0..width);
    Ok(ElementRead::Held(leaf(&binder, item)?))
}

/// The properties of element table `e` of columns `columns`, each once,
/// and what the binder knows of it.
fn properties_of(cx: Context<'_>, e: &ElementTableDesc, columns: &TableDesc) -> Result<(Vec<(String, Expr)>, TableInfo)> {
    let mut properties: Vec<(String, Expr)> = Vec::new();
    let labels = e.labels.iter().map(|l| l.name.clone()).collect();
    let mut info = TableInfo { edge: e.kind == ElementKind::Edge, labels, properties: Vec::new() };
    for p in e.properties() {
        if properties.iter().any(|(n, _)| n == &p.name) {
            continue;
        }
        let (expr, t) = property(cx, columns, &p.expression)
            .map_err(|err| grf("GRF-0005", format!("property {} of {}: {}", p.name, e.alias, err.msg)))?;
        info.properties.push((p.name.clone(), t));
        properties.push((p.name.clone(), expr));
    }
    Ok((properties, info))
}

/// An end of an edge table: its vertex table and the columns matched.
fn end_shape(cx: Context<'_>, desc: &GraphDesc, end: &EndpointDesc, position: &dyn Fn(&String) -> Result<usize>) -> Result<EndShape> {
    let table = desc
        .elements
        .iter()
        .position(|v| v.alias == end.vertex)
        .ok_or_else(|| grf("GRF-0005", format!("there is no vertex table {}", end.vertex)))?;
    let vertex = desc.elements.get(table).ok_or_else(|| grf("GRF-0005", format!("there is no vertex table {}", end.vertex)))?;
    let cols = end.columns.iter().map(position).collect::<Result<Vec<_>>>()?;
    let vertex_columns = super::super::graph_properties::element_columns(cx.db, cx.auth, &vertex.table)?;
    let references = end
        .references
        .iter()
        .map(|r| {
            vertex_columns
                .columns
                .iter()
                .position(|c| &c.name == r)
                .ok_or_else(|| grf("GRF-0005", format!("vertex table {} has no column {r}", end.vertex)))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(EndShape { table, columns: cols, references })
}

/// Property expression `text` over a row of `columns`, bound in `cx`.
fn property(cx: Context<'_>, columns: &TableDesc, text: &str) -> Result<(Expr, DataType)> {
    let value = Parser::for_stored(text)?.generation_expression()?;
    let item = super::super::FromItem::alone(columns.name.clone(), 0, (columns.clone(), super::super::views::Source::Table));
    let visible = (0..columns.columns.len()).map(|k| (0, k)).collect();
    let names = item.row_names();
    let binder = Binder::new(cx, None, (vec![item], names, visible), BTreeSet::new());
    binder.value_expression(&value, Scope::Rows(ONE_ROW))
}

/// The option of an adjacency index giving its source's columns.
const SOURCE_COLUMNS: &str = "ADJACENCY.SOURCE_COLUMNS";

/// The adjacency index of edge table `table` of ends `ends`, if one keeps
/// their columns, its source's first.
fn adjacency_index(cx: Context<'_>, table: &crate::catalog::QualifiedName, ends: &EdgeShape) -> Option<String> {
    let columns: Vec<usize> = ends.source.columns.iter().chain(&ends.destination.columns).copied().collect();
    let fits = |ix: &&crate::catalog::IndexDesc| {
        let source = ix.options.iter().find(|(n, _)| n == SOURCE_COLUMNS).map_or(1, |(_, v)| v.to_string().parse::<usize>().unwrap_or(0));
        ix.method == crate::index::ADJACENCY_METHOD && ix.columns.starts_with(&columns) && source == ends.source.columns.len()
    };
    cx.db.catalog.indexes(table).into_iter().find(fits).map(|ix| ix.name.clone())
}
