//! \[EXTENSION\] Label and reachability indexes of a property graph's
//! element tables (RULES.md D149, D150): CREATE LABEL INDEX, an index of
//! the labels method of an element table's key, or of each base element
//! table's; and CREATE REACHABILITY INDEX, one of the reachability method
//! of an edge table's ends, its source's first. Each is the CREATE INDEX
//! of its method, as CREATE ADJACENCY INDEX is (`graphs/adjacency.rs`).

use super::graph_rule;
use crate::ast::{IndexStatement, LabelIndex, Literal, OptionValue, ReachabilityIndex, TableName, TableOptionValue, TableStorage};
use crate::binder::qualify;
use crate::catalog::{ElementKind, ElementTableDesc, GraphDesc, QualifiedName};
use crate::db::{Session, State};
use crate::error::{At, Result};
use crate::index::reachability::SOURCE_COLUMNS_OPTION;
use crate::index::{LABELS_METHOD, REACHABILITY_METHOD};
use crate::value::Decimal;

/// The suffix of the name of an element table's label index.
const LABELS_SUFFIX: &str = "LABELS";
/// The suffix of the name of an edge table's reachability index.
const REACHABILITY_SUFFIX: &str = "REACHABILITY";

/// The CREATE INDEX of `method` named `name` on element table `element`'s
/// columns `columns`, of options `options`.
fn created(
    name: (String, usize),
    element: &ElementTableDesc,
    (method, columns): (&str, Vec<String>),
    options: Vec<TableOptionValue>,
) -> IndexStatement {
    let table = TableName { auth: Some(element.table.auth.clone()), ident: element.table.ident.clone(), at: name.1 };
    let columns = columns.into_iter().map(|c| (c, 0)).collect();
    let storage = TableStorage { engine: Some((method.to_owned(), 0)), options };
    IndexStatement::Create { name, table, columns, expressions: Vec::new(), storage }
}

impl Session {
    /// The graph `graph` names.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 (GRF-0001) if there is none.
    fn indexed_graph<'s>(&self, st: &'s State, graph: &TableName) -> Result<(QualifiedName, &'s GraphDesc)> {
        let qn = qualify(graph, &self.default_schema());
        let desc = st
            .catalog
            .graph(&qn)
            .ok_or_else(|| graph_rule(Some("GRF-0001"), format!("there is no property graph {qn}")))
            .or_at(graph.at)?;
        Ok((qn, desc))
    }

    /// CREATE LABEL INDEX or CREATE REACHABILITY INDEX `stmt` as the
    /// CREATE INDEX statements of their methods; another as it is.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 (GRF-0001) if there is no such graph, (GRF-0012) if it
    /// has no such element table.
    pub(in crate::db) fn graph_index_create(&self, st: &State, stmt: &IndexStatement) -> Result<Vec<IndexStatement>> {
        match stmt {
            IndexStatement::Labels(l) => self.labels_create(st, l),
            IndexStatement::Reachability(r) => Ok(vec![self.reachability_create(st, r)?]),
            IndexStatement::Create { .. }
            | IndexStatement::Drop { .. }
            | IndexStatement::Adjacency(_)
            | IndexStatement::Alter { .. }
            | IndexStatement::Maintain { .. } => Ok(vec![stmt.clone()]),
        }
    }

    /// Runs CREATE LABEL INDEX or CREATE REACHABILITY INDEX `stmt` as
    /// the CREATE INDEX statements it is, each of the catalog the one
    /// before left.
    ///
    /// # Errors
    ///
    /// As `graph_index_create`, and those of CREATE INDEX.
    pub(in crate::db) fn graph_indexes_made(&self, st: &State, stmt: &IndexStatement) -> Result<()> {
        for made in self.graph_index_create(st, stmt)? {
            self.index_statement(&self.db.snapshot()?, &made)?;
        }
        Ok(())
    }

    /// \[EXTENSION\] CREATE LABEL INDEX `l`: of the element table it names,
    /// named as written; else of each base element table, each named its
    /// graph's name, or the name written, then its alias.
    fn labels_create(&self, st: &State, l: &LabelIndex) -> Result<Vec<IndexStatement>> {
        let (qn, desc) = self.indexed_graph(st, &l.graph)?;
        let elements: Vec<&ElementTableDesc> = match &l.table {
            Some((alias, edge, at)) => {
                let kind = if *edge { ElementKind::Edge } else { ElementKind::Vertex };
                let found = desc.element(alias).filter(|e| e.kind == kind);
                vec![
                    found
                        .ok_or_else(|| graph_rule(Some("GRF-0012"), format!("graph {qn} has no such element table {alias}")))
                        .or_at(*at)?,
                ]
            }
            None => desc.elements.iter().filter(|e| st.catalog.table(&e.table).is_some()).collect(),
        };
        let at = l.graph.at;
        Ok(elements
            .into_iter()
            .map(|e| {
                let name = match (&l.name, &l.table) {
                    (Some(name), Some(_)) => name.clone(),
                    (Some((prefix, _)), None) => (format!("{prefix}_{}", e.alias), at),
                    (None, _) => (format!("{}_{}_{LABELS_SUFFIX}", qn.ident, e.alias), at),
                };
                created(name, e, (LABELS_METHOD, e.key.clone()), Vec::new())
            })
            .collect())
    }

    /// \[EXTENSION\] CREATE REACHABILITY INDEX `r`, of its edge table's
    /// source's columns, then its destination's.
    fn reachability_create(&self, st: &State, r: &ReachabilityIndex) -> Result<IndexStatement> {
        let (qn, desc) = self.indexed_graph(st, &r.graph)?;
        let (alias, alias_at) = &r.edge;
        let element = desc.element(alias).filter(|e| e.kind == ElementKind::Edge);
        let element =
            element.ok_or_else(|| graph_rule(Some("GRF-0012"), format!("graph {qn} has no edge table {alias}"))).or_at(*alias_at)?;
        let Some(ends) = &element.endpoints else { return Err(graph_rule(Some("GRF-0012"), format!("edge table {alias} has no ends"))) };
        let columns = ends.source.columns.iter().chain(&ends.destination.columns).cloned().collect();
        let source = Literal::Exact(Decimal::new(i128::try_from(ends.source.columns.len()).unwrap_or(i128::MAX), 0));
        let options = vec![TableOptionValue { name: SOURCE_COLUMNS_OPTION.to_owned(), at: 0, value: Some(OptionValue::Literal(source)) }];
        let name = r.name.clone().unwrap_or_else(|| (format!("{}_{}_{REACHABILITY_SUFFIX}", qn.ident, element.alias), r.graph.at));
        Ok(created(name, element, (REACHABILITY_METHOD, columns), options))
    }
}
