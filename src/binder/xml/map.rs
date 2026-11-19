//! \[EXTENSION\] XMLMAP bound (RULES.md D106): a table, a schema or the
//! catalog mapped to XML and to an XML Schema document [ISO/IEC
//! 9075-14:2006 draft §9.11, §9.14, §9.17]. Each table it maps is read by
//! a query, `SELECT * FROM` it, bound as a `<table reference>` naming it
//! is, each row's text written by a function of its values; the rows of
//! all of them, one table's after another's, are a subquery whose set
//! function writes the mapping's data as they come, so one bound holds
//! it whole (`functions/xml/map/rows.rs`); the mapping is a call of
//! XMLMAP's function of that subquery (`functions/xml/map.rs`).
//!
//! \[§4.10.8\] A column is visible if the statement's authorization
//! identifier holds SELECT on its table, and, of a distinct type, EXECUTE
//! on its cast to its source type; XML visible if its type is mappable
//! (`functions/xml/map/types.rs`). A schema's and the catalog's mappings
//! hold their XML visible tables alone, those of an XML visible column,
//! each schema's in order of name, and the catalog's XML visible schemas,
//! in order of name: the implementation-dependent repeatable orderings
//! [§9.12 GR5a, §9.16 GR5a].

use std::cell::RefCell;
use std::sync::Arc;

use super::super::{Binder, Candidate, OutColumn, Purpose, Scope, nondeterministic, qualify, rule};
use crate::ast::{MapResults, MapSource, Subquery, TableName, XmlMapping, rows_of};
use crate::catalog::{QualifiedName, Routine, applicable};
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::functions::xml_map::{
    Choices, Column, DataShape, Declared, Nulls, RowShape, Source, Table, XMLMAP, XmlMap, XmlMapRow, XmlMapRows, declared_name, rows_type,
};
use crate::functions::{AggregateRef, BinaryEncoding, RowOf, ScalarRef};
use crate::logical::Expr;
use crate::parser::mapped_sources;
use crate::system::{DEFINITION_SCHEMA, ST_DEFINITION_SCHEMA, information_view_name, information_views};
use crate::value::{DataType, Decimal, Value};
use crate::xml::{Version, XML_NAMESPACE, XMLNS_NAMESPACE, is_char};

/// \[EXTENSION\] implementation-defined: NULLS of an XMLMAP that writes
/// none, ABSENT, as XMLFOREST leaves a null value out [§6.15 SR3].
pub const IMPLICIT_NULLS: Nulls = Nulls::Absent;
/// \[EXTENSION\] implementation-defined: TABLEFOREST of one that writes no
/// AS, False, AS ELEMENT, so a table's mapping is a document, as a
/// schema's and the catalog's always are.
pub const IMPLICIT_FOREST: bool = false;
/// \[EXTENSION\] implementation-defined: what one that writes none of DATA
/// and METADATA gives, DATA.
pub const IMPLICIT_RESULTS: MapResults = MapResults::Data;

/// A table a mapping reads: its name, where it is written, and whether
/// it is mapped though it has no XML visible column, as TABLE's is.
struct Read {
    name: QualifiedName,
    at: usize,
    alone: bool,
}

/// What binding a mapping's tables found: the XML visible tables, in
/// order, and whether a visible column of them is an XML unmappable
/// column.
#[derive(Default)]
struct Found {
    tables: Vec<Table>,
    unmappable: bool,
}

/// What `kind` maps, of its XML visible tables `tables`: the catalog's
/// grouped by schema, in order.
fn source_of(kind: &MapSource, mut tables: Vec<Table>) -> Result<Source> {
    Ok(match kind {
        MapSource::Table(_) => Source::Table(tables.pop().ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] XMLMAP mapped no table"))?),
        MapSource::Schema(name, _) => Source::Schema(name.clone(), tables),
        MapSource::Catalog => {
            let mut schemas: Vec<(String, Vec<Table>)> = Vec::new();
            for t in tables {
                let last = schemas.last_mut().filter(|(schema, _)| *schema == t.schema);
                if let Some((_, of)) = last {
                    of.push(t);
                } else {
                    schemas.push((t.schema.clone(), vec![t]));
                }
            }
            Source::Catalog(schemas)
        }
    })
}

/// [§9.11 SR1, §9.14 SR1, §9.17 SR1] The parameters `m` gives, each
/// written or implicit: TARGETNS the zero-length string if none is
/// written, no namespace [§9.10 GR1]; ENCODING, if none is written,
/// `scoped`, the `<XML binary encoding>` in scope [§11.3] (RULES.md D106);
/// and the SQL-session's time zone displacement `zone` [§9.8 GR7].
///
/// # Errors
///
/// SQLCODE -103 if the target namespace is the namespace of `xml` or of
/// `xmlns`, as no namespace XMLNAMESPACES declares is [§11.3 SR6], or
/// holds a character no XML character [XML 1.1 §2.2].
fn choices(m: &XmlMapping, scoped: BinaryEncoding, zone: i16) -> Result<Choices> {
    let target = m.target.clone().unwrap_or_default();
    if let Some(bad) = target.chars().find(|&c| !is_char(Version::V11, c)) {
        return Err(rule(format!("[EXTENSION] [XML 1.1 §2.2] the target namespace holds U+{:04X}, no XML character", u32::from(bad))));
    }
    if target == XML_NAMESPACE || target == XMLNS_NAMESPACE {
        return Err(rule(format!("[EXTENSION] [ISO/IEC 9075-14:2006 draft §11.3 SR6] the target namespace is not {target}")));
    }
    let (data, metadata) = match m.results.unwrap_or(IMPLICIT_RESULTS) {
        MapResults::Data => (true, false),
        MapResults::Metadata => (false, true),
        MapResults::DataAndMetadata => (true, true),
    };
    Ok(Choices {
        nulls: m.nulls.unwrap_or(IMPLICIT_NULLS),
        forest: m.forest.unwrap_or(IMPLICIT_FOREST),
        target,
        data,
        metadata,
        encoding: m.encoding.unwrap_or(scoped),
        zone,
    })
}

/// The query of the rows of the table `name`: `SELECT * FROM` it, named
/// in full, so no query name a `<with clause>` gives names it.
fn rows_query(name: &QualifiedName, at: usize) -> Subquery {
    rows_of(TableName { auth: Some(name.auth.clone()), ident: name.ident.clone(), at }, at)
}

impl Binder<'_> {
    /// \[EXTENSION\] XMLMAP: its parameters, the tables it maps, the
    /// subquery of the data of their rows, and the call of its function of
    /// that, if DATA. A mapping reads its tables in an order not
    /// determined, so a constraint holds none [ISO/IEC 9075:1992 §11.9
    /// SR6].
    pub(super) fn xml_map(&self, m: &XmlMapping, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        if self.purpose.constrains() {
            return Err(nondeterministic("ISO/IEC 9075-14:2006 draft §9.10 GR6a", "XMLMAP"));
        }
        let alone = matches!(m.source, MapSource::Table(_));
        if !alone && self.purpose == Purpose::Mapped {
            return Err(rule(
                "[EXTENSION] RULES.md D106: a mapping of a schema or the catalog reads no table, view or routine that maps a schema or the catalog, as such a mapping could hold itself",
            ));
        }
        let choices = choices(m, self.binary_encoding(), self.now.zone())?;
        let reads = match &m.source {
            MapSource::Table(query) => vec![self.table_read(query)?],
            MapSource::Schema(name, at) => self.schema_reads(name).or_at(*at)?,
            MapSource::Catalog => self.catalog_reads(),
        };
        let catalog = self.db.catalog_name();
        let found = RefCell::new(Found::default());
        let queries: Vec<Subquery> = reads.iter().map(|r| rows_query(&r.name, r.at)).collect();
        let purpose = if alone { self.purpose } else { Purpose::Mapped };
        let data = self.aggregated_union(
            (&queries, purpose),
            scope,
            &mut |i, columns| reads.get(i).map_or(Ok(None), |read| self.branch(read, columns, &choices, &mut found.borrow_mut())),
            &mut || {
                let source = source_of(&m.source, found.borrow().tables.clone())?;
                let shape = DataShape::of(&source, &choices, &catalog);
                let arg = Expr::Call(ScalarRef(Arc::new(RowOf(DataType::row(Vec::new())))), vec![Expr::Column(0), Expr::Column(1)]);
                Ok((AggregateRef(Arc::new(XmlMapRows(Arc::new(shape)))), arg, XMLMAP.to_owned(), rows_type()))
            },
        )?;
        let Found { tables, unmappable } = found.into_inner();
        let map = XmlMap { catalog, source: source_of(&m.source, tables)?, choices };
        let result = map.result_type();
        let args = if map.choices.data { data.into_iter().collect() } else { Vec::new() };
        let call = Expr::Call(ScalarRef(Arc::new(map)), args);
        // [§9.11 GR3, §9.14 GR3, §9.17 GR3] Of a visible column that is an
        // XML unmappable column, the warning column cannot be mapped.
        let e = if unmappable { Expr::Warned(Box::new(call), SqlState::COLUMN_CANNOT_BE_MAPPED) } else { call };
        Ok((e, result))
    }

    /// [§9.10 GR6] The rows of the table `read`, of the columns `columns`
    /// `SELECT *` gives: if it is mapped, an XML visible table or TABLE's,
    /// its index among the tables mapped and the text of each row, written
    /// as `choices` say; and noted in `found`.
    fn branch(&self, read: &Read, columns: &[OutColumn], choices: &Choices, found: &mut Found) -> Result<Option<Vec<Expr>>> {
        let (table, positions, unmappable) = self.visible_columns(&read.name, columns)?;
        found.unmappable |= unmappable;
        if !read.alone && table.columns.is_empty() {
            return Ok(None);
        }
        let index =
            i128::try_from(found.tables.len()).map_err(|_| SqlError::new(Code::Unsupported, "[DESIGN] XMLMAP of too many tables"))?;
        let shape = RowShape::of(&table, choices, read.alone);
        let row = Expr::Call(ScalarRef(Arc::new(XmlMapRow(Arc::new(shape)))), positions.into_iter().map(Expr::Column).collect());
        found.tables.push(table);
        Ok(Some(vec![Expr::Const(Value::Exact(Decimal::new(index, 0))), row]))
    }

    /// [§9.11] TABLE: the table `query` reads, `SELECT * FROM` it, a base
    /// table or a viewed table [§4.10.9], which needs SELECT.
    fn table_read(&self, query: &Subquery) -> Result<Read> {
        let named = query.query.spec().and_then(|s| s.table_expression.from.first()).and_then(|r| r.named());
        let named = named.ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] XMLMAP of a table has no table"))?;
        let t = &named.table;
        if t.auth.is_none() && self.queries.is_some_and(|q| q.names(&t.ident)) {
            let msg = format!(
                "[EXTENSION] [ISO/IEC 9075-14:2006 draft §4.10.9] XMLMAP maps a base table or a viewed table, not the query {}",
                t.ident
            );
            return Err(rule(msg).or_at(t.at));
        }
        Ok(Read { name: qualify(t, self.schema), at: t.at, alone: true })
    }

    /// [§9.14] SCHEMA: its visible tables, in order of name.
    ///
    /// # Errors
    ///
    /// SQLSTATE 3F000 if there is no schema `schema`.
    fn schema_reads(&self, schema: &str) -> Result<Vec<Read>> {
        let db = self.db;
        let information = information_views().iter().any(|v| information_view_name(v).auth == schema);
        let known = db.catalog.schema_exists(schema) || information || db.system.keys().any(|t| t.auth == schema);
        if !known {
            let msg = format!("[EXTENSION] [ISO/IEC 9075:1992 §5.4 SR5] there is no schema {schema}");
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME));
        }
        let tables = self.tables_in(schema).into_iter().filter(|name| self.selects(name) && !self.view_maps(name));
        Ok(tables.map(|name| Read { name, at: 0, alone: false }).collect())
    }

    /// [§9.17] CATALOG: the visible tables of each schema, the schemas in
    /// order of name.
    fn catalog_reads(&self) -> Vec<Read> {
        let db = self.db;
        let mut schemas: Vec<String> = db.catalog.schemas().into_iter().map(str::to_owned).collect();
        schemas.extend(db.system.keys().map(|t| t.auth.clone()));
        schemas.extend(information_views().iter().map(|v| information_view_name(v).auth));
        schemas.sort_unstable();
        schemas.dedup();
        let tables = schemas.iter().flat_map(|s| self.tables_in(s));
        tables.filter(|name| self.selects(name) && !self.view_maps(name)).map(|name| Read { name, at: 0, alone: false }).collect()
    }

    /// The tables, views and system tables of `schema`, in order of name;
    /// none of the Definition Schema, or of SQL/MM Spatial's, which no
    /// statement reads [ISO/IEC 9075:1992 §21.1].
    fn tables_in(&self, schema: &str) -> Vec<QualifiedName> {
        if schema == DEFINITION_SCHEMA || schema == ST_DEFINITION_SCHEMA {
            return Vec::new();
        }
        let db = self.db;
        let information = information_views().iter().map(information_view_name);
        let mut names: Vec<QualifiedName> = db
            .catalog
            .tables()
            .into_iter()
            .map(|t| t.name.clone())
            .chain(db.catalog.views().into_iter().map(|v| v.name.clone()))
            .chain(db.system.keys().cloned())
            .chain(information)
            .filter(|n| n.auth == schema)
            .collect();
        names.sort_unstable_by(|a, b| a.ident.cmp(&b.ident));
        names.dedup();
        names
    }

    /// \[EXTENSION\] Whether `name` is a view whose query, or that of a view
    /// it reads, maps a schema or the catalog, which a mapping of a schema
    /// or the catalog leaves out, as its mapping could hold itself
    /// (RULES.md D106). Each view is read once, so the walk ends.
    fn view_maps(&self, name: &QualifiedName) -> bool {
        let catalog = &*self.db.catalog;
        let (mut next, mut seen) = (vec![name.clone()], Vec::new());
        while let Some(at) = next.pop() {
            let Some(view) = catalog.view(&at).filter(|_| !seen.contains(&at)) else { continue };
            if !mapped_sources(&view.query).is_empty() {
                return true;
            }
            next.extend(view.reads.iter().cloned());
            seen.push(at);
        }
        false
    }

    /// [§4.10.8] Whether the table `name` is a visible table: whether its
    /// columns are visible columns, the statement's authorization
    /// identifier holding SELECT on it, as every one does on a system
    /// table [ISO/IEC 9075:1992 §21.1].
    fn selects(&self, name: &QualifiedName) -> bool {
        self.db.system.contains_key(name) || applicable(&*self.db.catalog, self.who(), name).0.select
    }

    /// [§4.10.8] The table `name` of the columns `columns`, as `SELECT *`
    /// gives them: its XML visible columns, their positions there, and
    /// whether a visible column is an XML unmappable column; SQLCODE -900
    /// of a type nested too deep to map.
    fn visible_columns(&self, name: &QualifiedName, columns: &[OutColumn]) -> Result<(Table, Vec<usize>, bool)> {
        let mut table = Table { schema: name.auth.clone(), name: name.ident.clone(), columns: Vec::new() };
        let (mut positions, mut unmappable) = (Vec::new(), false);
        let catalog = self.db.catalog_name();
        for (i, c) in columns.iter().enumerate() {
            if !self.cast_visible(&c.data_type) {
                continue;
            }
            let column = c.name.clone().unwrap_or_default();
            let domain = self.domain_of(name, &column);
            let declared = Declared::of(&c.data_type, domain.as_ref().map(|(s, d)| (s.as_str(), d.as_str())));
            let Some(type_name) = declared_name(&declared, &catalog)? else {
                unmappable = true;
                continue;
            };
            table.columns.push(Column { name: column, declared, nullable: !c.not_null, type_name });
            positions.push(i);
        }
        Ok((table, positions, unmappable))
    }

    /// [§4.10.8] Whether a column of `t` may be visible: of a distinct
    /// type, if EXECUTE on its cast to its source type is held [ISO/IEC
    /// 9075-2:2023 §6.13 AR2].
    fn cast_visible(&self, t: &DataType) -> bool {
        let DataType::Distinct(d) = t else { return true };
        let name = QualifiedName { auth: d.schema.clone(), ident: d.name.clone() };
        self.db.catalog.user_type(&name).is_some_and(|u| Candidate::Cast(u, Routine::ToSource).executable(self.db, self.who()))
    }

    /// The schema and name of the domain the column `column` of the table
    /// or view `table` is of, if it is of one [§9.4 GR18].
    fn domain_of(&self, table: &QualifiedName, column: &str) -> Option<(String, String)> {
        let catalog = &*self.db.catalog;
        let columns = catalog.table(table).map(|t| &t.columns).or_else(|| catalog.view(table).map(|v| &v.columns))?;
        let domain = columns.iter().find(|c| c.name == column)?.domain.as_ref()?;
        Some((domain.name.auth.clone(), domain.name.ident.clone()))
    }
}
