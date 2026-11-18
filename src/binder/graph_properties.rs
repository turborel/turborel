//! \[EXTENSION\] The columns of a property graph's element table and the
//! expressions of its properties (RULES.md D106), bound as the
//! graph's owner binds them, as a view's query is bound. Each property's
//! `<value expression>` is bound over the columns of one row of the table,
//! as a generation expression is [ISO/IEC 9075-2:2023 §11.4 SR10], so it
//! holds no set function; but it may read other tables in a subquery, and
//! may be possibly non-deterministic, as a view's column may. A property's
//! expression is kept as text and bound again from it
//! (`Parser::for_stored`), as a generated column's is.

use std::collections::BTreeSet;

use super::views::Source;
use super::{Binder, Context, FromItem, Invoked, Purpose, Scope, case::set_type, qualify};
use crate::ast::ValueExpression;
use crate::catalog::{Horizon, Principal, QualifiedName, TableDesc};
use crate::db::State;
use crate::error::{Result, SqlError};
use crate::params::Params;
use crate::parser::Parser;
use crate::value::DataType;

/// The rule a set function in a property's expression breaks.
const ONE_ROW: &str = "[EXTENSION] a property is of one row of its element table, so its expression holds no set function (§4.1 R4.5)";

/// Who binds an element table, and how: the graph's owner, the schema of
/// the names its expressions write without one, and what they see.
#[derive(Clone, Copy)]
pub struct PropertyBinding<'a> {
    /// The owner of the graph's schema.
    pub owner: &'a str,
    /// The schema of names written without one.
    pub names: &'a str,
    /// What the expressions see; their subject routines among it.
    pub horizon: &'a Horizon,
    /// Where to note the routines they invoke, as they are defined.
    pub invoked: Option<&'a Invoked>,
}

impl PropertyBinding<'_> {
    /// The context of a query it binds.
    fn context<'p>(&'p self, db: &'p State, params: &'p Params) -> Context<'p> {
        Context {
            schema: self.names,
            horizon: self.horizon,
            invoked: self.invoked,
            purpose: Purpose::Query,
            ..Context::new(db, self.owner, params)
        }
    }
}

/// The table or view `table` as an element table reads it, as `owner`
/// reads it: its columns, and of a base table its constraints.
///
/// # Errors
///
/// SQLCODE -103 if it is not defined.
pub fn element_columns(db: &State, owner: &str, table: &QualifiedName) -> Result<TableDesc> {
    super::table_columns(db, table, Principal::sole(db.catalog.roles(), owner))
}

/// The type of property expression `text` over a row of element table
/// `desc`, bound as `binding` says, and the tables and views its
/// subqueries name.
///
/// # Errors
///
/// SQLCODE -103 if it does not parse, does not bind over the row, or holds
/// a set function; -104 if the owner lacks a privilege it needs.
pub fn property_bound(db: &State, binding: PropertyBinding<'_>, desc: &TableDesc, text: &str) -> Result<(DataType, Vec<QualifiedName>)> {
    let about = |e: SqlError| SqlError { msg: format!("property expression {text} of {}: {}", desc.name, e.msg), pos: None, ..e };
    row_bound(db, binding, desc, text).map_err(about)
}

/// `property_bound`, its errors not yet said to be of the property.
fn row_bound(db: &State, binding: PropertyBinding<'_>, desc: &TableDesc, text: &str) -> Result<(DataType, Vec<QualifiedName>)> {
    let value = Parser::for_stored(text)?.generation_expression()?;
    let mut reads = Vec::new();
    value.for_each_subquery(&mut |s| {
        let mut names = Vec::new();
        s.query.table_names(&mut names);
        reads.extend(names.into_iter().map(|t| qualify(t, binding.names)));
    });
    let params = Params::new();
    let cx = binding.context(db, &params);
    let item = FromItem::alone(desc.name.clone(), 0, (desc.clone(), Source::Table));
    let visible = (0..desc.columns.len()).map(|k| (0, k)).collect();
    let names = item.row_names();
    let binder = Binder::new(cx, None, (vec![item], names, visible), BTreeSet::new());
    let (_, t) = binder.value_expression(&value, Scope::Rows(ONE_ROW))?;
    Ok((t, reads))
}

/// [ISO/IEC 9075:1992 §9.3] The type a property of `types`, one of each
/// element table of a label that has it, is read as: the type of their
/// union (§5 R5.6).
///
/// # Errors
///
/// SQLCODE -103 if they are not union-compatible.
pub fn property_type(types: &[DataType]) -> Result<DataType> {
    set_type(types, "§9.3")
}

/// The `<value expression>` of a property's expression text, over the
/// columns of its element table.
///
/// # Errors
///
/// SQLCODE -102 if it is not one.
pub fn property_expression(text: &str) -> Result<ValueExpression> {
    Parser::for_stored(text)?.generation_expression()
}
