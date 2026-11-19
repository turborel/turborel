//! \[EXTENSION\] A plug-in's scalar function (RULES.md D169), a
//! `ScalarFunction` whose call is a CALL of kind 1: rows of arguments,
//! answered by one value a row. A statement's batch of rows is one CALL of
//! up to BATCH rows (`call_many_in`), and each call ends as the statement
//! is cancelled (D44).

use crate::error::Result;
use crate::exec::Subqueries;
use crate::functions::ScalarFunction;
use crate::plugin::manifest::Declared;
use crate::plugin::payload::SCALAR_CALL;
use crate::plugin::pool::CallContext;
use crate::plugin::values::{stored, values};
use crate::plugin::wire::put_rows;
use crate::value::{DataType, Value};

use super::{Link, Resolve, answer, counted, resolved};

/// The rows a CALL carries when the manifest says no BATCH.
pub const DEFAULT_BATCH: usize = 1024;

/// A plug-in's scalar function.
#[derive(Debug)]
pub struct PluginScalar {
    link: Link,
    parameters: Vec<DataType>,
    returns: DataType,
    deterministic: bool,
    batch: usize,
}

impl PluginScalar {
    /// The function `d` declares.
    ///
    /// # Errors
    ///
    /// A type `resolve` refuses.
    pub fn new(d: &Declared, link: Link, resolve: Resolve<'_>) -> Result<PluginScalar> {
        let parameters = resolved(&d.parameters, resolve)?;
        let returns = resolved(d.returns.as_slice(), resolve)?.pop().unwrap_or(DataType::Boolean);
        let batch = d.batch.and_then(|b| usize::try_from(b).ok()).unwrap_or(DEFAULT_BATCH);
        Ok(PluginScalar { link, parameters, returns, deterministic: d.deterministic, batch })
    }

    /// The values of `rows`, a CALL each `batch` of them, `interrupt`
    /// asked between and during.
    fn rows(&self, rows: &[Vec<Value>], interrupt: Option<&dyn Fn() -> Result<()>>) -> Result<Vec<Value>> {
        let mut all = Vec::with_capacity(rows.len());
        for chunk in rows.chunks(self.batch.max(1)) {
            if let Some(stop) = interrupt {
                stop()?;
            }
            let mut payload = Vec::new();
            put_rows(&mut payload, chunk)?;
            let got = self.link.call(SCALAR_CALL, payload, &mut CallContext { interrupt, sql: None })?;
            let got = counted(answer(&got, values)?, chunk.len())?;
            for v in got {
                all.push(stored(v, &self.returns)?);
            }
        }
        Ok(all)
    }

    /// Its call's name, in EXPLAIN.
    pub fn named(&self) -> String {
        self.link.named()
    }
}

impl ScalarFunction for PluginScalar {
    fn name(&self) -> &str {
        &self.link.object
    }
    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(self.returns.clone())
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let mut got = self.rows(&[args.to_vec()], None)?;
        Ok(got.pop().unwrap_or(Value::Null))
    }
    fn in_statement(&self) -> bool {
        true
    }
    fn call_in_statement(&self, args: &[Value], statement: &dyn Subqueries) -> Result<Value> {
        let stop = || statement.interrupted();
        let mut got = self.rows(&[args.to_vec()], Some(&stop))?;
        Ok(got.pop().unwrap_or(Value::Null))
    }
    fn deterministic(&self) -> bool {
        self.deterministic
    }
    fn parameter_types(&self) -> Option<Vec<DataType>> {
        Some(self.parameters.clone())
    }
    fn batches(&self) -> bool {
        true
    }
    fn call_many(&self, rows: &[Vec<Value>]) -> Result<Vec<Value>> {
        self.rows(rows, None)
    }
    fn call_many_in(&self, rows: &[Vec<Value>], statement: &dyn Subqueries) -> Result<Vec<Value>> {
        let stop = || statement.interrupted();
        self.rows(rows, Some(&stop))
    }
}
