//! \[EXTENSION\] A key part of an index that is an expression, bound over
//! the columns of its table's row (RULES.md D92): as a generation
//! expression is [ISO/IEC 9075-2:2023 §11.4 SR10], so it contains no
//! `<query expression>`, no set function and nothing possibly
//! non-deterministic, and reads its table's columns alone; then admitted
//! only if it raises no exception condition of any row
//! (`index/expression.rs`).

use std::collections::BTreeSet;
use std::sync::Arc;

use super::views::Source;
use super::{Binder, Context, FromItem, Purpose, Scope, key_operand};
use crate::catalog::{Horizon, TableDesc, owner_of};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::index::expression::BoundKey;
use crate::params::Params;
use crate::parser::Parser;

/// The key part `text` of index `index` on table `desc`, bound over its
/// row as the owner of its schema binds it, names without a schema of
/// that schema, and admitted.
///
/// # Errors
///
/// SQLCODE -103 if it does not parse, contains a `<query expression>`, a
/// set function or a potential source of non-determinism, references a
/// column its table lacks, is of a type no key groups [ISO/IEC 9075-2:2023
/// §9.12 SR3], or can raise an exception condition, each said to be of the
/// index's key part.
pub fn bind_key(db: &State, desc: &TableDesc, (index, text): (&str, &str)) -> Result<Arc<BoundKey>> {
    let about = |e: SqlError| SqlError { msg: format!("the key part ({text}) of index {index}: {}", e.msg), pos: None, ..e };
    bound(db, desc, text).map_err(about)
}

/// `bind_key`, its errors not yet said to be of the key part.
fn bound(db: &State, desc: &TableDesc, text: &str) -> Result<Arc<BoundKey>> {
    let value = Parser::for_stored(text)?.generation_expression()?;
    let mut queried = false;
    value.for_each_subquery(&mut |_| queried = true);
    if queried {
        return Err(SqlError::new(Code::SyntaxRule, "[EXTENSION] it contains no <query expression> (RULES.md D92)"));
    }
    let (params, horizon) = (Params::new(), Horizon::default());
    let owner = owner_of(&*db.catalog, &desc.name.auth);
    let cx = Context { purpose: Purpose::Key, schema: &desc.name.auth, horizon: &horizon, ..Context::new(db, owner, &params) };
    let item = FromItem::alone(desc.name.clone(), 0, (desc.clone(), Source::Table));
    let visible = (0..desc.columns.len()).map(|k| (0, k)).collect();
    let names = item.row_names();
    let binder = Binder::new(cx, None, (vec![item], names, visible), BTreeSet::new());
    let scope = Scope::Rows("[EXTENSION] a key part is over the columns of one row, so it contains no set function (RULES.md D92)");
    let (e, t) = binder.value_expression(&value, scope)?;
    key_operand(&t, "an index")?;
    Ok(Arc::new(BoundKey::new(e, t, desc)?))
}
