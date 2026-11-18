//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §21] What linear referencing
//! reads of the catalogs, and the procedures that change them.
//!
//! \[EXTENSION\] The standard gives no statement to change ST_LRMS or
//! ST_LR_COLUMNS ("How these views are updated is implementation-defined"
//! [§21.1.1]). Each procedure is invoked by CALL and changes the catalog
//! in the caller's transaction, as a definition does:
//!
//! - `ST_DEFINELRM(name, id, organization, organization id, definition,
//!   description)`, `definition` the well-known text of the ST_LRM value
//!   whose ST_LRMID is `id` [§21.2.3 Description 8, 9];
//! - `ST_DROPLRM(name)`;
//! - `ST_REGISTERLRCOLUMN(schema, table, column, lrm id)`;
//! - `ST_UNREGISTERLRCOLUMN(schema, table, column)`.
//!
//! A routine that finds an LRM by its ST_PrivateLRMID, or a linear
//! element by its ST_PrivateLinearElementID or feature ID, reads them as
//! its statement is bound: it is given, after its arguments, the ARRAY of
//! a query of the LRMs of ST_LRMS and of the linear elements of each
//! column ST_LR_COLUMNS registers whose declared type is ST_LinearElement
//! or a subtype (`query`). Implementation-defined reading: those are the
//! linear elements the identifiers name [§15.6.1 Description 13, §15.9.1
//! Description 23], read with the privileges of the statement's user.

use std::fmt::Write as _;
use std::sync::Arc;

use super::super::{Catalog, LinearElement, LrKind, LrValue, Lrm, from_text, linear_units};
use super::args::{INTEGER, P, text_of};
use super::kind::{of_data_type, of_value};
use super::{DESCRIPTION_LENGTH, LRM_NAME_LENGTH};
use crate::catalog::{LrColumnDesc, LrmDesc, QualifiedName, SpatialChange};
use crate::error::{Code, Result, SqlError};
use crate::functions::AppendedQuery;
use crate::functions::builtin::{Procedure, ProcedureContext};
use crate::lexer::MAX_IDENTIFIER_LENGTH;
use crate::value::{DataType, Value};

/// An identifier quoted, as a `<delimited identifier>`.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The query of the LRMs and linear elements a routine reads: rows of
/// ROW(ST_LRM, ST_LINEARELEMENT), one of the two null.
pub fn query(catalog: &dyn crate::catalog::Catalog) -> AppendedQuery {
    let mut query = String::from("SELECT ROW(LRM, CAST(NULL AS ST_LINEARELEMENT)) FROM ST_INFORMTN_SCHEMA.ST_LRMS");
    for c in catalog.spatial().lr_columns() {
        let column = catalog.table(&c.table).and_then(|t| t.columns.iter().find(|col| col.name == c.column));
        let element = column.and_then(|col| of_data_type(&col.data_type)).is_some_and(|k| k.is_subtype_of(LrKind::LinearElement));
        if element {
            let name = quoted(&c.column);
            let _ = write!(
                query,
                " UNION ALL SELECT ROW(CAST(NULL AS ST_LRM), CAST({name} AS ST_LINEARELEMENT)) FROM {}.{} WHERE {name} IS NOT NULL",
                quoted(&c.table.auth),
                quoted(&c.table.ident)
            );
        }
    }
    AppendedQuery { query, tables: Vec::new() }
}

/// What a routine read of the catalogs.
#[derive(Debug, Default)]
pub struct Read {
    /// The LRMs of ST_LRMS.
    pub lrms: Vec<Lrm>,
    /// The linear elements of the columns registered.
    pub elements: Vec<LinearElement>,
}

impl Read {
    /// What `array`, the ARRAY `query` gave, holds; nothing of none.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 of a value not of its column's type.
    pub fn of(array: Option<&Value>) -> Result<Read> {
        let mut read = Read::default();
        let Some(Value::Array(rows)) = array else { return Ok(read) };
        for row in rows.iter() {
            let Value::Row(fields) = row else { continue };
            for field in fields.iter().filter(|f| !f.is_null()) {
                read.take(of_value(field)?);
            }
        }
        Ok(read)
    }

    /// Takes `v`, an LRM or a linear element.
    fn take(&mut self, v: LrValue) {
        if let LrValue::Lrm(l) = v {
            self.lrms.push(l);
        } else if let LrValue::LinearElement(e) = v {
            self.elements.push(e);
        }
    }
}

/// A procedure of the §21 catalog.
#[derive(Debug, Clone, Copy)]
pub enum LrProcedure {
    /// Defines a linear referencing method.
    DefineLrm,
    /// Drops one.
    DropLrm,
    /// Registers a column, with a method or none.
    Register,
    /// Removes a column's registration.
    Unregister,
}

/// The procedures, built in.
pub fn procedures() -> Vec<Arc<dyn Procedure>> {
    [LrProcedure::DefineLrm, LrProcedure::DropLrm, LrProcedure::Register, LrProcedure::Unregister]
        .into_iter()
        .map(|p| -> Arc<dyn Procedure> { Arc::new(p) })
        .collect()
}

/// The text `v` holds, none if it is null.
fn text(v: Option<&Value>) -> Result<Option<String>> {
    v.map_or(Ok(None), text_of)
}

/// The integer `v` holds, none if it is null.
fn integer(v: Option<&Value>) -> Option<i32> {
    let Some(Value::Exact(d)) = v else { return None };
    if d.scale != 0 {
        return None;
    }
    i32::try_from(d.units).ok()
}

/// [§21.2] A null where a NOT NULL column of `table` takes it.
fn required(table: &str, what: &str) -> SqlError {
    SqlError::new(Code::Constraint, format!("[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §21.2] {table}: {what} is not null"))
}

/// An identifier's type.
fn identifier() -> DataType {
    DataType::varying(u32::try_from(MAX_IDENTIFIER_LENGTH).unwrap_or(u32::MAX))
}

impl LrProcedure {
    /// [§21.2.3] The method `args` define: its LRM the value of its
    /// definition, whose ST_LRMID is its identifier [Description 8, 9].
    fn method(args: &[Value]) -> Result<LrmDesc> {
        let table = "ST_LRMS";
        let name = text(args.first())?.ok_or_else(|| required(table, "LRM_NAME"))?;
        let id = integer(args.get(1)).ok_or_else(|| required(table, "LRM_ID"))?;
        let organization = match (text(args.get(2))?, integer(args.get(3))) {
            (Some(org), Some(org_id)) => Some((org, org_id)),
            (None, None) => None,
            (Some(_), None) | (None, Some(_)) => {
                let msg = "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §21.2.3] ORGANIZATION_NULL: ORGANIZATION and ORGANIZATION_LRM_ID are both null or neither";
                return Err(SqlError::new(Code::Constraint, msg));
            }
        };
        let definition = text(args.get(4))?.ok_or_else(|| required(table, "DEFINITION"))?;
        let units = linear_units();
        let LrValue::Lrm(lrm) = from_text(&definition, LrKind::Lrm, &Catalog::of_units(&units))? else {
            return Err(SqlError::new(Code::Corrupt, "[DESIGN] the text of an ST_LRM read as another type"));
        };
        if lrm.id != id {
            let msg = format!(
                "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §21.2.3 Description 8] {table}: LRM_ID {id} is not the LRM's ST_LRMID {}",
                lrm.id
            );
            return Err(SqlError::new(Code::Constraint, msg));
        }
        let lrm = LrValue::Lrm(lrm).encode()?;
        Ok(LrmDesc { name, id, organization, definition, description: text(args.get(5))?, lrm })
    }

    /// The table and column `args` name.
    fn column(args: &[Value]) -> Result<(QualifiedName, String)> {
        let part = |at: usize, what: &str| -> Result<String> { text(args.get(at))?.ok_or_else(|| required("ST_LR_COLUMNS", what)) };
        Ok((QualifiedName { auth: part(0, "TABLE_SCHEMA")?, ident: part(1, "TABLE_NAME")? }, part(2, "COLUMN_NAME")?))
    }
}

impl Procedure for LrProcedure {
    fn name(&self) -> &str {
        match self {
            Self::DefineLrm => "ST_DEFINELRM",
            Self::DropLrm => "ST_DROPLRM",
            Self::Register => "ST_REGISTERLRCOLUMN",
            Self::Unregister => "ST_UNREGISTERLRCOLUMN",
        }
    }

    fn parameters(&self, count: usize) -> Option<Vec<DataType>> {
        let name = DataType::varying(LRM_NAME_LENGTH);
        let types = match self {
            Self::DefineLrm => vec![
                name,
                INTEGER,
                DataType::varying(crate::system::ORGANIZATION_LENGTH),
                INTEGER,
                P::Clob.data_type(None),
                DataType::varying(DESCRIPTION_LENGTH),
            ],
            Self::DropLrm => vec![name],
            Self::Register => vec![identifier(), identifier(), identifier(), INTEGER],
            Self::Unregister => vec![identifier(), identifier(), identifier()],
        };
        (types.len() == count).then_some(types)
    }

    fn call(&self, args: &[Value], cx: &mut dyn ProcedureContext) -> Result<()> {
        let change = match self {
            Self::DefineLrm => SpatialChange::DefineLrm(Self::method(args)?),
            Self::DropLrm => SpatialChange::DropLrm(text(args.first())?.ok_or_else(|| required("ST_LRMS", "LRM_NAME"))?),
            Self::Register => {
                let (table, column) = Self::column(args)?;
                SpatialChange::RegisterLr(LrColumnDesc { table, column, lrm: integer(args.get(3)) })
            }
            Self::Unregister => {
                let (table, column) = Self::column(args)?;
                SpatialChange::UnregisterLr(table, column)
            }
        };
        cx.define_spatial(change)
    }
}
