//! \[EXTENSION\] One element table of a property graph described (§4.1):
//! the table or view it is (R4.1, GRF-0002), its key (R4.2, R4.3,
//! GRF-0003), an edge's endpoints (GRF-0003, GRF-0004), and its labels
//! and their properties (R4.4 to R4.6), each property's expression bound
//! as the graph's owner binds it.

use super::graph_rule;
use crate::ast::{ElementDefinition, EndpointDefinition, LabelDefinition, PropertiesDefinition, TemporaryKind};
use crate::binder::{PropertyBinding, element_columns, property_bound, property_type, qualify};
use crate::catalog::{
    ConstraintKind, ElementKind, ElementTableDesc, EndpointDesc, Endpoints, GraphMode, LabelDesc, PropertyDesc, QualifiedName, TableDesc,
    applicable, identifier, sole,
};
use crate::db::State;
use crate::error::{At, Code, Result, SqlError};

/// What an element table is described with: the graph's owner and the
/// names it writes, its mode, the kind of element table, and the vertex
/// tables described before it.
#[derive(Clone, Copy)]
pub(super) struct Element<'a> {
    pub(super) binding: PropertyBinding<'a>,
    pub(super) mode: GraphMode,
    pub(super) kind: ElementKind,
    pub(super) vertices: &'a [ElementTableDesc],
}

impl State {
    /// The descriptor of element table `e`, and the tables and views it
    /// reads: its own, and those its properties' subqueries name.
    ///
    /// # Errors
    ///
    /// As `describe_graph` says.
    pub(super) fn element_described(&self, cx: Element<'_>, e: &ElementDefinition) -> Result<(ElementTableDesc, Vec<QualifiedName>)> {
        let table = qualify(&e.table, cx.binding.names);
        let desc = self.element_table(cx.binding.owner, &table).or_at(e.table.at)?;
        let key = element_key(&desc, cx.mode, e.key.as_deref()).or_at(e.table.at)?;
        let endpoints = match cx.kind {
            ElementKind::Vertex => None,
            ElementKind::Edge => Some(self.endpoints(cx, &desc, e)?),
        };
        let mut reads = vec![table.clone()];
        let mut labels = Vec::new();
        let written =
            if e.labels.is_empty() { vec![LabelDefinition { name: None, at: e.table.at, properties: None }] } else { e.labels.clone() };
        for l in &written {
            let name = l.name.clone().unwrap_or_else(|| e.alias_name().to_owned());
            if labels.iter().any(|d: &LabelDesc| d.name == name) {
                return Err(graph_rule(None, format!("element table {} has label {name} twice", e.alias_name()))).or_at(l.at);
            }
            let properties = self.label_properties(cx.binding, &desc, l, &mut reads)?;
            labels.push(LabelDesc { name, properties });
        }
        one_expression(e.alias_name(), &labels)?;
        Ok((ElementTableDesc { alias: e.alias_name().to_owned(), table, kind: cx.kind, key, labels, endpoints }, reads))
    }

    /// The table or view `table` as an element table of a graph `owner`
    /// owns: its columns and constraints.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 (GRF-0002) if it is no base table or view of a schema,
    /// or is a declared local temporary table; -104 if `owner` holds no
    /// SELECT on it.
    fn element_table(&self, owner: &str, table: &QualifiedName) -> Result<TableDesc> {
        let base = self.catalog.table(table);
        let declared = base.is_some_and(|t| t.temporary.as_ref().is_some_and(|t| t.kind == TemporaryKind::Declared));
        if (base.is_none() && self.catalog.view(table).is_none()) || declared {
            let msg = format!("{table} is no base table or view an element table may be (R4.1)");
            return Err(graph_rule(Some("GRF-0002"), msg));
        }
        if !applicable(&*self.catalog, sole(&*self.catalog, owner), table).0.select {
            let msg = format!("[EXTENSION] {owner}, the graph's owner, holds no SELECT on element table {table} (§20)");
            return Err(SqlError::new(Code::Privilege, msg));
        }
        let mut desc = element_columns(self, owner, table)?;
        // A view declares no key, whatever its tables do.
        if base.is_none() {
            desc.constraints.clear();
        }
        Ok(desc)
    }

    /// The source and destination of edge table `e`, of columns of `desc`.
    fn endpoints(&self, cx: Element<'_>, desc: &TableDesc, e: &ElementDefinition) -> Result<Endpoints> {
        let missing = || graph_rule(None, format!("edge table {} names no SOURCE and DESTINATION", e.alias_name())).or_at(e.table.at);
        let (Some(source), Some(destination)) = (&e.source, &e.destination) else { return Err(missing()) };
        Ok(Endpoints {
            source: self.endpoint(cx, desc, source, "SOURCE")?,
            destination: self.endpoint(cx, desc, destination, "DESTINATION")?,
            undirected: e.undirected,
        })
    }

    /// The properties label `l` of element table `desc` gives, their
    /// expressions bound, the tables their subqueries name added to `reads`.
    fn label_properties(
        &self,
        binding: PropertyBinding<'_>,
        desc: &TableDesc,
        l: &LabelDefinition,
        reads: &mut Vec<QualifiedName>,
    ) -> Result<Vec<PropertyDesc>> {
        let mut written: Vec<(String, String, usize)> = Vec::new();
        match &l.properties {
            Some(PropertiesDefinition::None) => {}
            None => written.extend(desc.columns.iter().map(|c| (c.name.clone(), identifier(&c.name), l.at))),
            Some(PropertiesDefinition::AllColumns(except)) => {
                if let Some(other) = except.iter().find(|c| desc.column(c).is_none()) {
                    return Err(graph_rule(None, format!("EXCEPT names {other}, no column of {}", desc.name))).or_at(l.at);
                }
                let kept = desc.columns.iter().filter(|c| !except.contains(&c.name));
                written.extend(kept.map(|c| (c.name.clone(), identifier(&c.name), l.at)));
            }
            Some(PropertiesDefinition::Listed(list)) => {
                for p in list {
                    let named = "a property that is no column alone is named with AS (R4.5)";
                    let name = p.name.clone().ok_or_else(|| graph_rule(None, named.to_owned())).or_at(p.at)?;
                    written.push((name, p.text.clone(), p.at));
                }
            }
        }
        let mut properties: Vec<PropertyDesc> = Vec::new();
        for (name, expression, at) in written {
            if properties.iter().any(|p| p.name == name) {
                return Err(graph_rule(None, format!("a label of {} has property {name} twice", desc.name))).or_at(at);
            }
            let (data_type, read) = property_bound(self, binding, desc, &expression).or_at(at)?;
            reads.extend(read.into_iter().filter(|t| !reads.contains(t)).collect::<Vec<_>>());
            properties.push(PropertyDesc { name, expression, data_type });
        }
        Ok(properties)
    }
}

/// The key of an element table of `desc`: `written`, else its PRIMARY KEY,
/// else its one UNIQUE constraint of NOT NULL columns (R4.2). Of ENFORCED
/// MODE, a key written is the columns of such a constraint (R4.3).
///
/// # Errors
///
/// SQLCODE -103 if a column written is not of the table, or written
/// twice, or, ENFORCED, the columns are of no such constraint; GRF-0003 if
/// none is written and none is found.
fn element_key(desc: &TableDesc, mode: GraphMode, written: Option<&[String]>) -> Result<Vec<String>> {
    let keys = declared_keys(desc);
    let Some(written) = written else {
        let primary = desc.primary_key().map(<[usize]>::to_vec);
        let found = primary.or_else(|| if keys.len() == 1 { keys.first().cloned() } else { None });
        let found = found.ok_or_else(|| {
            let msg = format!("{} has no PRIMARY KEY nor one UNIQUE key of NOT NULL columns, so write its KEY (R4.2)", desc.name);
            graph_rule(Some("GRF-0003"), msg)
        })?;
        return Ok(found.iter().filter_map(|k| desc.columns.get(*k)).map(|c| c.name.clone()).collect());
    };
    let at = columns_of(desc, written, "KEY")?;
    let mut sorted = at.clone();
    sorted.sort_unstable();
    let declared = keys.iter().any(|k| {
        let mut k = k.clone();
        k.sort_unstable();
        k == sorted
    });
    if mode == GraphMode::Enforced && !declared {
        let msg = format!(
            "KEY ({}) of {} is no PRIMARY KEY or UNIQUE key of NOT NULL columns, as ENFORCED MODE needs (R4.3)",
            written.join(", "),
            desc.name
        );
        return Err(graph_rule(None, msg));
    }
    Ok(written.to_vec())
}

/// The columns of each PRIMARY KEY and UNIQUE constraint of `desc` whose
/// columns are all NOT NULL and that is WITHOUT OVERLAPS of no period.
fn declared_keys(desc: &TableDesc) -> Vec<Vec<usize>> {
    let not_null = |k: &[usize]| k.iter().all(|c| desc.columns.get(*c).is_some_and(|c| c.not_null));
    desc.constraints
        .iter()
        .filter_map(|c| match &c.kind {
            ConstraintKind::Unique { columns, period: None, .. } => Some(columns.clone()),
            ConstraintKind::Unique { period: Some(_), .. }
            | ConstraintKind::NotNull(_)
            | ConstraintKind::ForeignKey(_)
            | ConstraintKind::Check { .. } => None,
        })
        .filter(|k| not_null(k))
        .collect()
}

/// The positions in `desc` of the columns `names`, a `clause`'s.
///
/// # Errors
///
/// SQLCODE -103 if one is no column of it, or is named twice.
fn columns_of(desc: &TableDesc, names: &[String], clause: &str) -> Result<Vec<usize>> {
    let mut at = Vec::new();
    for name in names {
        let Some(k) = desc.column(name) else {
            return Err(graph_rule(None, format!("{clause} names {name}, no column of {}", desc.name)));
        };
        if at.contains(&k) {
            return Err(graph_rule(None, format!("{clause} names column {name} twice")));
        }
        at.push(k);
    }
    Ok(at)
}

impl State {
    /// The endpoint `end`, after `clause`, of an edge table of `desc`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103: GRF-0004 if it names no vertex table of the graph;
    /// GRF-0003 if its key is not written and no one FOREIGN KEY gives it; if
    /// its columns and the vertex table's are not as many, or of types no
    /// comparison takes, or, ENFORCED, are of no FOREIGN KEY.
    fn endpoint(&self, cx: Element<'_>, desc: &TableDesc, end: &EndpointDefinition, clause: &str) -> Result<EndpointDesc> {
        let (alias, at) = (&end.vertex.0, end.vertex.1);
        let Some(vertex) = cx.vertices.iter().find(|v| v.alias == *alias) else {
            let msg = format!("{clause} {alias} is no vertex table of the graph");
            return Err(graph_rule(Some("GRF-0004"), msg)).or_at(at);
        };
        let vertex_desc = element_columns(self, cx.binding.owner, &vertex.table).or_at(at)?;
        let found = match &end.key {
            Some(columns) => (columns.clone(), end.references.clone().unwrap_or_else(|| vertex.key.clone())),
            None => inferred(desc, (vertex, &vertex_desc), clause).or_at(at)?,
        };
        let (ours, theirs) = (columns_of(desc, &found.0, clause).or_at(at)?, columns_of(&vertex_desc, &found.1, "REFERENCES").or_at(at)?);
        if ours.len() != theirs.len() {
            return Err(graph_rule(None, format!("{clause} KEY names {} columns, and REFERENCES {}", ours.len(), theirs.len()))).or_at(at);
        }
        for (a, b) in ours.iter().zip(&theirs) {
            let types = [desc.columns.get(*a), vertex_desc.columns.get(*b)].map(|c| c.map(|c| c.data_type.clone()));
            let [Some(x), Some(y)] = types else { continue };
            property_type(&[x, y])
                .map_err(|e| graph_rule(None, format!("{clause} KEY and REFERENCES are of types no comparison takes, as {}", e.msg)))
                .or_at(at)?;
        }
        let declared = desc.foreign_keys().any(|(_, fk)| fk.table == vertex.table && fk.columns == ours && fk.referenced == theirs);
        if cx.mode == GraphMode::Enforced && !declared {
            let msg = format!("{clause} of {} is no FOREIGN KEY of it, as ENFORCED MODE needs (R4.3)", desc.name);
            return Err(graph_rule(None, msg)).or_at(at);
        }
        Ok(EndpointDesc { vertex: alias.clone(), columns: found.0, references: found.1 })
    }
}

/// The columns of edge table `desc`, and of `vertex`, whose table is
/// `vertex_table`, of its one FOREIGN KEY that references the table of
/// `vertex`, as an endpoint `clause` written without KEY takes (R4.2).
///
/// # Errors
///
/// SQLCODE -103 (GRF-0003) if it has none or more than one.
fn inferred(desc: &TableDesc, (vertex, vertex_table): (&ElementTableDesc, &TableDesc), clause: &str) -> Result<(Vec<String>, Vec<String>)> {
    let mut found = desc.foreign_keys().filter(|(_, fk)| fk.table == vertex.table);
    let (Some((_, fk)), None) = (found.next(), found.next()) else {
        let msg = format!(
            "{} has no one FOREIGN KEY to {}, so write {clause} KEY (...) REFERENCES {} (R4.2)",
            desc.name, vertex.table, vertex.alias
        );
        return Err(graph_rule(Some("GRF-0003"), msg));
    };
    let names = |of: &TableDesc, at: &[usize]| at.iter().filter_map(|k| of.columns.get(*k)).map(|c| c.name.clone()).collect::<Vec<_>>();
    Ok((names(desc, &fk.columns), names(vertex_table, &fk.referenced)))
}

/// That each property of element table `alias` has one expression, of
/// whichever of its labels `labels` it is (R4.4).
///
/// # Errors
///
/// SQLCODE -103 if two labels give one name two expressions.
fn one_expression(alias: &str, labels: &[LabelDesc]) -> Result<()> {
    let all: Vec<&PropertyDesc> = labels.iter().flat_map(|l| &l.properties).collect();
    for (k, p) in all.iter().enumerate() {
        if all.iter().skip(k.saturating_add(1)).any(|q| q.name == p.name && q.expression != p.expression) {
            let msg =
                format!("element table {alias} gives property {} two expressions, where it takes one on each of its labels (R4.4)", p.name);
            return Err(graph_rule(None, msg));
        }
    }
    Ok(())
}
