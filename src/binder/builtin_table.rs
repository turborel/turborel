//! \[EXTENSION\] `TABLE(f(args))` of a table function written in Rust
//! (`functions/builtin.rs`), bound as a lateral collection derived table
//! [ISO/IEC 9075-2:2023 §7.6 SR8]: its arguments bound over the table
//! references before it, each stored in its parameter's type, and the
//! queries it reads, named of the arguments known here, bound with the
//! privileges of the statement's user as `MULTISET (query)` arguments
//! after them. Its rows are the elements of the multiset its invocation
//! returns, each row's fields its columns (RULES.md D-SPATIAL-ROUTINES).

use std::sync::Arc;

use super::from::{Bounds, From, FromItem, renamed};
use super::rule;
use super::unnest::{Unnested, lateral_binder, lateral_item};
use crate::ast::{PtfArgument, PtfTable};
use crate::catalog::ColumnDesc;
use crate::error::{At, Result};
use crate::functions::builtin::{TableCall, TableFunction};
use crate::functions::{ScalarFunction, ScalarRef};
use crate::logical::Expr;
use crate::parser::Parser;
use crate::value::{DataType, Value};

/// The rule a set function in an argument breaks.
const IN_ARGUMENT: &str = "[ISO/IEC 9075-2:2023 §6.9 SR7] a set function in a table function's argument is over an outer reference";

/// The parameters of a table function, as a scalar function declares
/// them, so its arguments are bound as a function's are.
#[derive(Debug)]
struct Parameters(Vec<DataType>);

impl ScalarFunction for Parameters {
    fn name(&self) -> &'static str {
        "TABLE"
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }
    fn call(&self, _: &[Value]) -> Result<Value> {
        Ok(Value::Null)
    }
    fn argument_type(&self, position: usize, _: &[Option<DataType>]) -> Option<DataType> {
        self.0.get(position).cloned()
    }
    fn parameter_type(&self, position: usize, _: &[Option<DataType>]) -> Option<crate::params::ParameterType> {
        self.0.get(position).cloned().map(crate::params::ParameterType::Of)
    }
}

/// The value of `e`, if it is known as it is bound: a constant, or a
/// call or cast of known values.
fn known(e: &Expr) -> Option<Value> {
    if let Expr::Const(v) = e {
        return Some(v.clone());
    }
    if let Expr::Call(f, args) = e {
        let values = args.iter().map(known).collect::<Option<Vec<_>>>()?;
        return f.0.call(&values).ok();
    }
    if let Expr::Cast(inner, t) = e {
        return crate::value::cast(known(inner)?, t).ok();
    }
    None
}

/// The FROM item of `p` if it invokes a table function written in Rust,
/// unqualified; none else.
///
/// # Errors
///
/// SQLCODE -103 if an argument is no value expression, the function
/// takes no such count of arguments, or an argument is not assignable to
/// its parameter; the function's, naming what it reads; a query's.
pub(super) fn builtin_item(b: Bounds<'_>, p: &PtfTable, offset: usize, from: &mut From) -> Result<Option<FromItem>> {
    let inv = &p.invocation;
    let Some(f) = inv.schema.is_none().then(|| b.cx.db.functions.table(&inv.name)).flatten() else { return Ok(None) };
    if !inv.copartition.is_empty() {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.6 SR7] table function {} takes no table argument to co-partition", inv.name)));
    }
    let mut args = Vec::with_capacity(inv.arguments.len());
    for a in &inv.arguments {
        let PtfArgument::Value(v) = a else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §10.4 SR] table function {} takes value expressions alone", inv.name)));
        };
        args.push(v.clone());
    }
    let types = f
        .parameters(args.len())
        .ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §9.18 SR10] table function {} takes no {} arguments", inv.name, args.len())))?;
    let binder = lateral_binder(b, from);
    let scope = super::Scope::Rows(IN_ARGUMENT);
    let (mut exprs, _) = binder.arguments(&Parameters(types), &args, scope)?;
    let values: Vec<Option<Value>> = exprs.iter().map(known).collect();
    let refs: Vec<Option<&Value>> = values.iter().map(Option::as_ref).collect();
    for query in f.reads(&refs)? {
        let read = Parser::new(&format!("MULTISET ({query})"))?.generation_expression()?;
        exprs.push(binder.value_expression(&read, scope)?.0);
    }
    let reads = binder.used.into_inner();
    from.used.extend(reads.iter().copied());
    let columns = columns(&*f, p)?;
    let call = Expr::Call(ScalarRef(TableCall::scalar(Arc::clone(&f), args.len()).0), exprs);
    let correlation = p.correlation.clone().unwrap_or_else(|| inv.name.clone());
    Ok(Some(lateral_item(&correlation, p.at, columns, offset, Unnested::rows_of(call, f.columns().len(), reads))))
}

/// The columns of `f`'s table, renamed by `p`'s derived column list.
fn columns(f: &dyn TableFunction, p: &PtfTable) -> Result<Vec<ColumnDesc>> {
    let mut columns: Vec<ColumnDesc> = f
        .columns()
        .into_iter()
        .map(|(name, data_type)| ColumnDesc {
            name,
            data_type,
            not_null: false,
            default: None,
            domain: None,
            collation: None,
            generated: None,
        })
        .collect();
    if let Some(list) = &p.columns {
        if list.len() != columns.len() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.6 SR7d ii] {} column names for {} columns", list.len(), columns.len())))
                .or_at(p.at);
        }
        renamed(&mut columns, list)?;
    }
    Ok(columns)
}
