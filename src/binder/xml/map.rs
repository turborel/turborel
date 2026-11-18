//! \[EXTENSION\] XMLMAP bound (RULES.md D86): a table, a schema or the
//! catalog mapped to XML and to an XML Schema document [ISO/IEC
//! 9075-14:2006 draft §9.11, §9.14, §9.17]. Each table it maps is read by
//! a subquery, `SELECT * FROM` it, bound as a `<table reference>` naming
//! it is, whose rows a set function writes as §9.10 writes them
//! (`functions/xml/map/rows.rs`); the mapping is a call of XMLMAP's
//! function of those subqueries (`functions/xml/map.rs`).
//!
//! \[§4.10.8\] A column is visible if the statement's authorization
//! identifier holds SELECT on its table, and, of a distinct type, EXECUTE
//! on its cast to its source type; XML visible if its type is mappable
//! (`functions/xml/map/types.rs`). A schema's and the catalog's mappings
//! hold their XML visible tables alone, those of an XML visible column,
//! each schema's in order of name, and the catalog's XML visible schemas,
//! in order of name: the implementation-dependent repeatable orderings
//! [§9.12 GR5a, §9.16 GR5a].

use std::sync::Arc;

use super::super::{Binder, Candidate, OutColumn, Scope, nondeterministic, qualify, rule};
use crate::ast::{MapResults, MapSource, Subquery, TableName, XmlMapping, rows_of};
use crate::catalog::{QualifiedName, Routine, applicable};
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::functions::xml_map::{Choices, Column, Declared, Nulls, RowShape, Source, Table, XmlMap, XmlMapRows, mappable, rows_type};
use crate::functions::{AggregateRef, IMPLICIT_BINARY_ENCODING, RowOf, ScalarRef};
use crate::logical::Expr;
use crate::system::{DEFINITION_SCHEMA, INFORMATION_SCHEMA, information_views};
use crate::value::DataType;
use crate::value::types::Field;
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

/// The tables a mapping maps, each's subquery of its rows' text, and
/// whether a visible column of them is an XML unmappable column.
type Mapped = (Vec<Table>, Vec<Expr>, bool);

/// The XML visible schemas of the catalog, each with its XML visible
/// tables, the subquery of each table's rows' text, in order, and whether
/// a visible column of them is an XML unmappable column.
type Schemas = (Vec<(String, Vec<Table>)>, Vec<Expr>, bool);

/// [§9.11 SR1, §9.14 SR1, §9.17 SR1] The parameters `m` gives, each
/// written or implicit: TARGETNS the zero-length string if none is
/// written, no namespace [§9.10 GR1].
///
/// # Errors
///
/// SQLCODE -103 if the target namespace is the namespace of `xml` or of
/// `xmlns`, as no namespace XMLNAMESPACES declares is [§11.3 SR6], or
/// holds a character no XML character [XML 1.1 §2.2].
fn choices(m: &XmlMapping) -> Result<Choices> {
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
        encoding: m.encoding.unwrap_or(IMPLICIT_BINARY_ENCODING),
    })
}

/// The query of the rows of the table `name`: `SELECT * FROM` it, named
/// in full, so no query name a `<with clause>` gives names it.
fn rows_query(name: &QualifiedName, at: usize) -> Subquery {
    rows_of(TableName { auth: Some(name.auth.clone()), ident: name.ident.clone(), at }, at)
}

impl Binder<'_> {
    /// \[EXTENSION\] XMLMAP: its parameters, the tables it maps, each's
    /// rows a subquery, and the call of its function of them, if DATA.
    /// A mapping reads its tables in an order not determined, so a
    /// constraint holds none [ISO/IEC 9075:1992 §11.9 SR6].
    pub(super) fn xml_map(&self, m: &XmlMapping, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        if self.purpose.constrains() {
            return Err(nondeterministic("ISO/IEC 9075-14:2006 draft §9.10 GR6a", "XMLMAP"));
        }
        let choices = choices(m)?;
        let (source, rows, unmappable) = match &m.source {
            MapSource::Table(query) => {
                let (mut tables, rows, unmappable) = self.map_table(query, scope, &choices)?;
                (tables.pop().map(Source::Table), rows, unmappable)
            }
            MapSource::Schema(name, at) => {
                let (tables, rows, unmappable) = self.map_schema(name, scope, &choices).or_at(*at)?;
                (Some(Source::Schema(name.clone(), tables)), rows, unmappable)
            }
            MapSource::Catalog => {
                let (schemas, rows, unmappable) = self.map_catalog(scope, &choices)?;
                (Some(Source::Catalog(schemas)), rows, unmappable)
            }
        };
        let source = source.ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] XMLMAP mapped no table"))?;
        let map = XmlMap { catalog: self.db.catalog_name(), source, choices };
        let result = map.result_type();
        let args = if map.choices.data { rows } else { Vec::new() };
        let call = Expr::Call(ScalarRef(Arc::new(map)), args);
        // [§9.11 GR3, §9.14 GR3, §9.17 GR3] Of a visible column that is an
        // XML unmappable column, the warning column cannot be mapped.
        let e = if unmappable { Expr::Warned(Box::new(call), SqlState::COLUMN_CANNOT_BE_MAPPED) } else { call };
        Ok((e, result))
    }

    /// [§9.11] TABLE: the table `query` reads, `SELECT * FROM` it, a base
    /// table or a viewed table [§4.10.9], which needs SELECT.
    fn map_table(&self, query: &Subquery, scope: Scope<'_>, choices: &Choices) -> Result<Mapped> {
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
        let name = qualify(t, self.schema);
        let (table, rows, unmappable) = self.mapped_table(&name, &rows_query(&name, t.at), scope, (choices, true))?;
        Ok((vec![table], vec![rows], unmappable))
    }

    /// [§9.14] SCHEMA: its XML visible tables, in order of name.
    ///
    /// # Errors
    ///
    /// SQLSTATE 3F000 if there is no schema `schema`.
    fn map_schema(&self, schema: &str, scope: Scope<'_>, choices: &Choices) -> Result<Mapped> {
        let db = self.db;
        let known = db.catalog.schema_exists(schema) || schema == INFORMATION_SCHEMA || db.system.keys().any(|t| t.auth == schema);
        if !known {
            let msg = format!("[EXTENSION] [ISO/IEC 9075:1992 §5.4 SR5] there is no schema {schema}");
            return Err(SqlError::new(Code::SyntaxRule, msg).with_state(SqlState::INVALID_SCHEMA_NAME));
        }
        let mut mapped: Mapped = (Vec::new(), Vec::new(), false);
        for name in self.tables_in(schema) {
            if !self.selects(&name) {
                continue;
            }
            let (table, rows, unmappable) = self.mapped_table(&name, &rows_query(&name, 0), scope, (choices, false))?;
            mapped.2 |= unmappable;
            if !table.columns.is_empty() {
                mapped.0.push(table);
                mapped.1.push(rows);
            }
        }
        Ok(mapped)
    }

    /// [§9.17] CATALOG: its XML visible schemas, those of an XML visible
    /// table, in order of name.
    fn map_catalog(&self, scope: Scope<'_>, choices: &Choices) -> Result<Schemas> {
        let db = self.db;
        let mut schemas: Vec<String> = db.catalog.schemas().into_iter().map(str::to_owned).collect();
        schemas.extend(db.system.keys().map(|t| t.auth.clone()));
        schemas.push(INFORMATION_SCHEMA.to_owned());
        schemas.sort_unstable();
        schemas.dedup();
        let (mut mapped, mut all, mut unmapped) = (Vec::new(), Vec::new(), false);
        for schema in schemas.into_iter().filter(|s| s != DEFINITION_SCHEMA) {
            let (tables, rows, unmappable) = self.map_schema(&schema, scope, choices)?;
            unmapped |= unmappable;
            if !tables.is_empty() {
                mapped.push((schema, tables));
                all.extend(rows);
            }
        }
        Ok((mapped, all, unmapped))
    }

    /// The tables, views and system tables of `schema`, in order of name;
    /// none of the Definition Schema, which no statement reads [ISO/IEC
    /// 9075:1992 §21.1].
    fn tables_in(&self, schema: &str) -> Vec<QualifiedName> {
        if schema == DEFINITION_SCHEMA {
            return Vec::new();
        }
        let db = self.db;
        let information =
            information_views().iter().map(|v| QualifiedName { auth: INFORMATION_SCHEMA.to_owned(), ident: v.name.to_owned() });
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

    /// [§4.10.8] Whether the table `name` is a visible table: whether its
    /// columns are visible columns, the statement's authorization
    /// identifier holding SELECT on it, as every one does on a system
    /// table [ISO/IEC 9075:1992 §21.1].
    fn selects(&self, name: &QualifiedName) -> bool {
        self.db.system.contains_key(name) || applicable(&*self.db.catalog, self.who(), name).0.select
    }

    /// The table `name` mapped: its rows read by `query`, written as
    /// `choices` say, as the mapping of the table itself if `alone`.
    fn mapped_table(
        &self,
        name: &QualifiedName,
        query: &Subquery,
        scope: Scope<'_>,
        (choices, alone): (&Choices, bool),
    ) -> Result<(Table, Expr, bool)> {
        let mut found = None;
        let (rows, _) = self.aggregated_rows(query, scope, &mut |columns| {
            let (table, positions, unmappable) = self.visible_columns(name, columns);
            let fields = positions.iter().filter_map(|&i| columns.get(i)).map(|c| Field {
                name: c.name.clone().unwrap_or_default(),
                data_type: c.data_type.clone(),
                unnamed: false,
            });
            let row =
                Expr::Call(ScalarRef(Arc::new(RowOf(DataType::row(fields.collect())))), positions.into_iter().map(Expr::Column).collect());
            let shape = RowShape::of(&table, choices, alone);
            found = Some((table, unmappable));
            Ok((AggregateRef(Arc::new(XmlMapRows(Arc::new(shape)))), row, format!("XMLMAP({name})"), rows_type()))
        })?;
        let (table, unmappable) = found.ok_or_else(|| SqlError::new(Code::Api, "[DESIGN] XMLMAP bound no table"))?;
        Ok((table, rows, unmappable))
    }

    /// [§4.10.8] The table `name` of the columns `columns`, as `SELECT *`
    /// gives them: its XML visible columns, their positions there, and
    /// whether a visible column is an XML unmappable column.
    fn visible_columns(&self, name: &QualifiedName, columns: &[OutColumn]) -> (Table, Vec<usize>, bool) {
        let mut table = Table { schema: name.auth.clone(), name: name.ident.clone(), columns: Vec::new() };
        let (mut positions, mut unmappable) = (Vec::new(), false);
        for (i, c) in columns.iter().enumerate() {
            if !self.cast_visible(&c.data_type) {
                continue;
            }
            if !mappable(&c.data_type) {
                unmappable = true;
                continue;
            }
            let column = c.name.clone().unwrap_or_default();
            let domain = self.domain_of(name, &column);
            let declared = Declared::of(&c.data_type, domain.as_ref().map(|(s, d)| (s.as_str(), d.as_str())));
            table.columns.push(Column { name: column, declared, nullable: !c.not_null });
            positions.push(i);
        }
        (table, positions, unmappable)
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
