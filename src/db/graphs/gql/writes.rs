//! \[EXTENSION\] The writes of GQL's data-modifying statements to a
//! graph's element tables (graph standard §12, RULES.md D144), each an
//! SQL statement of the session's transaction with its values as
//! parameters, so R12.3 holds: they are logged and undone as SQL's are.
//!
//! In a graph over tables, of Mode A or a closed GQL graph, an element's
//! labels choose the one base table it is written to (R12.1, else
//! GRF-0051); its properties are that table's columns. A closed graph's
//! element takes its key from the graph's sequence, and its type is the
//! table's (GRF-0053 of a label or property it has not). An open graph
//! writes its two tables, the labels as text and the properties as a
//! JSON object built here.

use super::graphs::{EDGES, NODES, SEQUENCE, graph_table};
use super::identities::table_of;
use crate::binder::gql::{Graph, LABEL_BAR};
use crate::catalog::{ElementKind, ElementTableDesc, Endpoints, QualifiedName, id_type, identifier, qualified_identifier};
use crate::db::Session;
use crate::error::{Code, Result, SqlError};
use crate::json::{Item, Member, Scalar};
use crate::lexer::Tok;
use crate::params::Params;
use crate::storage::Row;
use crate::value::json::Json;
use crate::value::{DataType, Value};

/// A value to write, and its type.
pub(super) type Typed = (Value, DataType);

/// The error of a write the graph's tables cannot take: GRF-0051 of a
/// graph over tables, GRF-0053 of a closed GQL graph's type.
pub(super) fn refused(graph: &Graph, closed: bool, what: &str) -> SqlError {
    if closed {
        return SqlError::new(Code::Constraint, format!("[EXTENSION] GRF-0053 {what}, against the graph type of {}", graph.desc.name));
    }
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] GRF-0051 {what}, in graph {}", graph.desc.name))
}

/// The column property `p` of element table `e` is.
pub(super) fn property_column(graph: &Graph, closed: bool, e: &ElementTableDesc, p: &str) -> Result<String> {
    let Some(d) = e.property(p) else {
        if closed {
            return Err(refused(graph, true, &format!("type {} has no property {p}", e.alias)));
        }
        return Err(SqlError::new(Code::SyntaxRule, format!("[EXTENSION] GRF-0012 element table {} has no property {p}", e.alias)));
    };
    let toks: Vec<Tok> = crate::lexer::lex(&d.expression)?.into_iter().map(|t| t.tok).collect();
    if let [Tok::Identifier(column), Tok::End] = toks.as_slice() {
        return Ok(column.clone());
    }
    Err(refused(graph, closed, &format!("property {p} of {} is no column", e.alias)))
}

/// The element tables of kind `kind` over the one base table an element
/// of labels `labels` is written to (R12.1): more than one of an edge
/// table of node and edge tables, one for each pair of node tables it
/// joins (§4.2).
pub(super) fn written_tables<'g>(
    s: &Session,
    graph: &'g Graph,
    closed: bool,
    (kind, labels): (ElementKind, &[String]),
) -> Result<Vec<&'g ElementTableDesc>> {
    let has = |e: &&ElementTableDesc| e.kind == kind && labels.iter().all(|l| e.has_label(l));
    let all: Vec<&ElementTableDesc> = graph.desc.elements.iter().filter(has).collect();
    let exact: Vec<&ElementTableDesc> = all.iter().copied().filter(|e| e.labels.len() == labels.len()).collect();
    let chosen = if one_table(&all) { all } else { exact };
    let Some(e) = chosen.first().filter(|_| one_table(&chosen)) else {
        return Err(refused(graph, closed, &format!("the labels {} name no one element table", labels.join(" & "))));
    };
    let st = s.gql_state()?;
    if st.catalog.table(&e.table).is_none() {
        return Err(refused(graph, closed, &format!("element table {} is no base table", e.alias)));
    }
    Ok(chosen)
}

/// Whether `tables` are some, all over one base table.
fn one_table(tables: &[&ElementTableDesc]) -> bool {
    tables.first().is_some_and(|f| tables.iter().all(|e| e.table == f.table))
}

/// The value `v` written to property `p` of `e`: a null of the
/// property's type.
pub(super) fn typed_for(e: &ElementTableDesc, p: &str, v: &Typed) -> Typed {
    match e.property(p).filter(|_| v.0.is_null()) {
        Some(d) => (Value::Null, d.data_type.clone()),
        None => v.clone(),
    }
}

/// Parameters `GQL_P0`, `GQL_P1`, ... of `values`, and their names
/// written: a null value's, a null of its type.
pub(super) fn params(values: &[Typed]) -> (Params, Vec<String>) {
    let mut p = Params::new();
    let mut names = Vec::new();
    for (i, (v, t)) in values.iter().enumerate() {
        if v.is_null() {
            names.push(format!("CAST(NULL AS {t})"));
            continue;
        }
        let name = format!("GQL_P{i}");
        p = p.declare(&name, t.clone(), v.clone());
        names.push(name);
    }
    (p, names)
}

/// The columns and values of a row to insert, and, of an edge, the
/// identities of the vertices it leaves and reaches.
pub(super) type Written<'a> = (Vec<(String, Typed)>, Option<(&'a str, &'a str)>);

/// Of an edge of `e` and its endpoints `ep` that joins the vertices
/// `src` and `dst` identify: its select list and its FROM and WHERE, the
/// endpoint columns added to `names` and the identities to `values`.
///
/// # Errors
///
/// GRF-0051, or GRF-0053 if `closed`, of a vertex of a table the edge
/// does not join.
fn edge_source(
    s: &Session,
    (graph, e, ep): (&Graph, &ElementTableDesc, &Endpoints),
    (names, values): (&mut Vec<String>, &mut Vec<Typed>),
    ((src, dst), closed): ((&str, &str), bool),
) -> Result<(String, String)> {
    let n = values.len();
    values.extend([id_value(src), id_value(dst)]);
    let (_, written) = params(values);
    let mut selected: Vec<String> = written.iter().take(n).cloned().collect();
    let (mut from, mut wheres) = (Vec::new(), Vec::new());
    for (q, end, id) in [("\"$s\"", &ep.source, src), ("\"$d\"", &ep.destination, dst)] {
        let v = table_of(graph, id)?;
        if v.alias != end.vertex {
            return Err(refused(graph, closed, &format!("an edge of {} does not connect {}", e.alias, v.alias)));
        }
        names.extend(end.columns.iter().map(|c| identifier(c)));
        selected.extend(end.references.iter().map(|r| format!("{q}.{}", identifier(r))));
        from.push(format!("{} AS {q}", qualified_identifier(&v.table)));
        let param = written.get(n.saturating_add(wheres.len())).cloned().unwrap_or_default();
        wheres.push(s.keyed(v, q, &param)?);
    }
    Ok((selected.join(", "), format!("{} WHERE {}", from.join(", "), wheres.join(" AND "))))
}

/// Whether `e` is over a node or edge table (§4.2).
fn native_table(s: &Session, e: &ElementTableDesc) -> Result<bool> {
    Ok(s.gql_state()?.catalog.table(&e.table).is_some_and(|t| t.native().is_some()))
}

/// An identity as a value to write.
pub(super) fn id_value(id: &str) -> Typed {
    (Value::Char(id.into()), DataType::varying(crate::logical::IDENTITY_LENGTH))
}

impl Session {
    /// Runs `sql` with the parameters of `values`.
    pub(super) fn write(&self, sql: &str, values: &[Typed]) -> Result<()> {
        let (mut p, _) = params(values);
        self.execute(sql, &mut p).map(|_| ())
    }

    /// The rows of the query `sql` with the parameters `params`: its
    /// columns' types, and its rows.
    pub(super) fn read_all(&self, sql: &str, params: &Params) -> Result<(Vec<DataType>, Vec<Row>)> {
        let commands = (crate::db::area::commands::SELECT_ROWS, crate::db::area::commands::UNRECOGNIZED);
        let mut cursor = self.open_with(commands, |call| self.open_in(call, sql, params))?;
        let types = cursor.columns.iter().map(|c| c.data_type.clone()).collect();
        let mut rows = Vec::new();
        while let Some(row) = cursor.fetch()? {
            rows.push(row);
        }
        Ok((types, rows))
    }

    /// The next number of GQL graph `g`'s sequence, and its type.
    pub(super) fn next_number(&self, g: &QualifiedName) -> Result<Typed> {
        let seq = qualified_identifier(&graph_table(g, SEQUENCE));
        let (types, rows) = self.read_all(&format!("SELECT NEXT VALUE FOR {seq} FROM (VALUES (0)) AS \"$z\"(\"$Z\")"), &Params::new())?;
        let value = rows.into_iter().next().and_then(|r| r.into_iter().next()).unwrap_or(Value::Null);
        Ok((value, types.into_iter().next().unwrap_or(DataType::varying(20))))
    }

    /// Inserts a row of `e` of the columns and values `cols`, its key from
    /// the sequence of closed graph `g` if given, and, of an edge, its
    /// endpoint columns selected from the vertices `ends` identifies: its
    /// key's values.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 (GRF-0051) if no value of its key is given; as the
    /// INSERT's.
    pub(super) fn insert_row(
        &self,
        graph: &Graph,
        e: &ElementTableDesc,
        (mut cols, ends): Written<'_>,
        g: Option<&QualifiedName>,
    ) -> Result<Vec<Typed>> {
        if let Some(g) = g {
            cols.insert(0, ("$ID".to_owned(), self.next_number(g)?));
        }
        let keyed = e.key.iter().all(|k| cols.iter().any(|(c, _)| c == k));
        if !keyed && !native_table(self, e)? {
            return Err(SqlError::new(
                Code::SyntaxRule,
                format!("[EXTENSION] GRF-0051 the element written to {} gives no value of its key", e.alias),
            ));
        }
        let mut names: Vec<String> = cols.iter().map(|(c, _)| identifier(c)).collect();
        let mut values: Vec<Typed> = cols.iter().map(|(_, v)| v.clone()).collect();
        let table = qualified_identifier(&e.table);
        let sql = if let (Some(ends), Some(ep)) = (ends, &e.endpoints) {
            let (selected, from) = edge_source(self, (graph, e, ep), (&mut names, &mut values), (ends, g.is_some()))?;
            format!("INSERT INTO {table} ({}) SELECT {selected} FROM {from}", names.join(", "))
        } else {
            let (_, written) = params(&values);
            format!("INSERT INTO {table} ({}) VALUES ({})", names.join(", "), written.join(", "))
        };
        self.write(&sql, &values)?;
        if keyed {
            return Ok(e.key.iter().filter_map(|k| cols.iter().find(|(c, _)| c == k).map(|(_, v)| v.clone())).collect());
        }
        // The node or edge table drew its key (§4.2): the INSERT tells
        // which, so no other row of the same values is taken for it.
        let drawn = self.drawn_identities().into_iter().next();
        drawn
            .map(|v| vec![(v, id_type())])
            .ok_or_else(|| SqlError::new(Code::Data, format!("[EXTENSION] the row inserted into {} drew no identity", e.alias)))
    }

    /// The table of open graph `g` of elements of `kind`.
    pub(super) fn open_table(g: &QualifiedName, kind: ElementKind) -> String {
        qualified_identifier(&graph_table(g, if kind == ElementKind::Edge { EDGES } else { NODES }))
    }
}

/// The JSON object of `props`, the members of `base` kept but those it
/// sets; a null value removes its member.
pub(super) fn props_json(base: &Value, props: &[(String, Value)]) -> Result<Value> {
    let mut members: Vec<Member> = Vec::new();
    if let Value::Json(j) = base {
        members = j.item()?.as_object().map(<[Member]>::to_vec).unwrap_or_default();
    }
    for (k, v) in props {
        members.retain(|m| m.key != *k);
        if let Some(s) = Scalar::new(v.clone()) {
            members.push(Member { key: k.clone(), value: Item::Scalar(s) });
        }
    }
    Ok(Value::Json(Json::of_item(&Item::Object(members))?))
}

/// `labels`, a label set held as text, with `label` added, or taken away.
pub(super) fn relabeled(labels: &str, label: &str, add: bool) -> String {
    let mut all: Vec<&str> = labels.split(LABEL_BAR).filter(|l| !l.is_empty() && *l != label).collect();
    if add {
        all.push(label);
    }
    if all.is_empty() {
        return LABEL_BAR.to_owned();
    }
    format!("{LABEL_BAR}{}{LABEL_BAR}", all.join(LABEL_BAR))
}
