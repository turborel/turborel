//! [ISO/IEC 9075-2:2023 §11.4 SR10, SR13] Generation expressions, bound
//! over the columns of their table's row: as a column is defined, to check
//! them and find a column's type, and as a statement that writes the table
//! runs, to give each generated column its value [§15.11 GR6a, §15.14 GR9]
//! (RULES.md D63).

use std::collections::BTreeSet;

use super::views::Source;
use super::{Binder, Context, FromItem, Purpose, Scope};
use crate::catalog::{Generation, TableDesc, owner_of};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::logical::{Expr, expr_columns};
use crate::params::Params;
use crate::parser::Parser;
use crate::value::DataType;

/// [ISO/IEC 9075-2:2023 §11.4 SR10] The generation expression `g` of the
/// column at `at` of table `desc`, bound over its row, as the owner of the
/// table's schema binds it, and its declared type.
///
/// # Errors
///
/// SQLCODE -103 if it does not parse, contains a `<query expression>`
/// \[SR10f\] or a potential source of non-determinism \[SR10d\], or
/// references a column other than a base column of the table \[SR10c\].
pub fn bind_generation(db: &State, desc: &TableDesc, (at, g): (usize, &Generation)) -> Result<(Expr, DataType)> {
    let column = desc.columns.get(at).map_or("?", |c| c.name.as_str());
    let about =
        |e: SqlError| SqlError { msg: format!("the generation expression of column {column} of {}: {}", desc.name, e.msg), pos: None, ..e };
    bound(db, desc, g).map_err(about)
}

/// `bind_generation`, its errors not yet said to be of the column.
fn bound(db: &State, desc: &TableDesc, g: &Generation) -> Result<(Expr, DataType)> {
    let value = Parser::new(&g.text)?.of_definition().generation_expression()?;
    let mut queried = false;
    value.for_each_subquery(&mut |_| queried = true);
    if queried {
        return Err(SqlError::new(Code::SyntaxRule, "[ISO/IEC 9075-2:2023 §11.4 SR10f] it contains no <query expression>"));
    }
    let params = Params::new();
    let owner = owner_of(&*db.catalog, &desc.name.auth);
    let cx = Context { purpose: Purpose::Generation, schema: &g.names, ..Context::new(db, owner, &params) };
    let item = FromItem::alone(desc.name.clone(), 0, (desc.clone(), Source::Table));
    let visible = (0..desc.columns.len()).map(|k| (0, k)).collect();
    let names = item.row_names();
    let binder = Binder::new(cx, None, (vec![item], names, visible), BTreeSet::new());
    let scope = Scope::Rows("[ISO/IEC 9075-2:2023 §11.4 SR10] it is over the columns of one row, so it contains no set function");
    let (e, t) = binder.value_expression(&value, scope)?;
    let mut used = Vec::new();
    expr_columns(&e, &mut used);
    if let Some(other) = used.iter().filter_map(|&u| desc.columns.get(u)).find(|c| c.generation().is_some()) {
        return Err(SqlError::new(
            Code::SyntaxRule,
            format!("[ISO/IEC 9075-2:2023 §11.4 SR10c] it references base columns alone, and {} is a generated column", other.name),
        ));
    }
    Ok((e, t))
}

/// [ISO/IEC 9075-2:2023 §15.11 GR6a, §15.14 GR9] Each generated column of
/// table `desc`, by its position, with its generation expression bound
/// over the table's row.
///
/// # Errors
///
/// As `bind_generation`: one binds no more.
pub fn generations(db: &State, desc: &TableDesc) -> Result<Vec<(usize, Expr)>> {
    let mut bound = Vec::new();
    for (at, c) in desc.columns.iter().enumerate() {
        if let Some(g) = c.generation() {
            bound.push((at, bind_generation(db, desc, (at, g))?.0));
        }
    }
    Ok(bound)
}
