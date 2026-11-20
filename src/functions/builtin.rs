//! \[EXTENSION\] Routines built into the database or registered with it
//! that are written in Rust and are no scalar or aggregate function: table
//! functions, invoked as `TABLE(f(args))` in a FROM clause [ISO/IEC
//! 9075-2:2023 §7.6], and procedures, invoked by CALL [§15.1]. They are
//! how SQL/MM Spatial's routines that return a table or that change a
//! topology's tables are made [ISO/IEC 13249-3:2016 (DIS, SC32 N2593)
//! §8.6.5, §11].
//!
//! A table function reads the tables it needs through queries it names
//! from the arguments known as its invocation is bound
//! (`TableFunction::reads`): each is bound with the invoker's privileges
//! and run in the statement's transaction, its rows given to the
//! function as a multiset of rows, so the tables it reads are locked and
//! shown in the plan as those of any query are (RULES.md
//! D125).
//!
//! A procedure runs in the caller's SQL-session and transaction
//! (`ProcedureContext`): its statements are the session's, as the
//! session's current user, and are undone with the CALL that runs them
//! if it fails.

use std::fmt::Debug;
use std::sync::Arc;

use super::{ScalarFunction, ScalarRef};
use crate::error::{Code, Result, SqlError};
use crate::value::types::{Field, MAX_ARRAY_CARDINALITY};
use crate::value::{DataType, Value};

/// \[EXTENSION\] A table function written in Rust.
pub trait TableFunction: Send + Sync + Debug {
    /// Its name, as an identifier folds to upper case.
    fn name(&self) -> &str;

    /// The declared types of its parameters when it is invoked with
    /// `count` arguments; none if it takes no such count.
    fn parameters(&self, count: usize) -> Option<Vec<DataType>>;

    /// The columns of the table it returns, of their names and types.
    fn columns(&self) -> Vec<(String, DataType)>;

    /// The query expressions whose rows it reads, of the arguments known
    /// as its invocation is bound, `known`, each a constant or none: by
    /// default none.
    ///
    /// # Errors
    ///
    /// The function's own, as of an argument that must be known and is
    /// not, or that names nothing.
    fn reads(&self, known: &[Option<&Value>]) -> Result<Vec<String>> {
        let _ = known;
        Ok(Vec::new())
    }

    /// Its rows, of the arguments `args` and the rows of each query
    /// `reads` named, `read`, in that order.
    ///
    /// # Errors
    ///
    /// The function's own.
    fn rows(&self, args: &[Value], read: &[Vec<Vec<Value>>]) -> Result<Vec<Vec<Value>>>;

    /// The error of binding a query it reads that failed with `e`: by
    /// default `e`.
    fn read_failed(&self, e: SqlError) -> SqlError {
        e
    }

    /// Whether it is invoked as a function too, `F(args)` in a value
    /// expression, its value the first column of its first row, the null
    /// value of none: by default not.
    fn scalar(&self) -> bool {
        false
    }
}

/// \[EXTENSION\] What a procedure written in Rust runs in: the SQL-session
/// and transaction of the CALL that invokes it.
pub trait ProcedureContext {
    /// Runs the SQL statement `sql`, its `<dynamic parameter
    /// specification>`s taking `args` in order, as a statement of the
    /// procedure; or the `<schema definition>` `sql`, owned by the
    /// session's user, made in the transaction.
    ///
    /// # Errors
    ///
    /// The statement's.
    fn run(&mut self, sql: &str, args: &[Value]) -> Result<()>;

    /// The rows of the query `sql`, its `<dynamic parameter
    /// specification>`s taking `args` in order.
    ///
    /// # Errors
    ///
    /// The query's.
    fn rows(&mut self, sql: &str, args: &[Value]) -> Result<Vec<Vec<Value>>>;

    /// Makes `change` to the spatial reference systems and geometry
    /// columns the catalog describes [ISO/IEC 13249-3:2016 (DIS, SC32
    /// N2593) §20.2, §20.3], in the transaction.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if the change breaks a constraint of the tables that
    /// describe them.
    fn define_spatial(&mut self, change: crate::catalog::SpatialChange) -> Result<()>;

    /// Runs `sql` as `run` does, with the privileges of the owner of
    /// schema `schema` [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.1.1]:
    /// how a routine writes tables the user may only read.
    ///
    /// # Errors
    ///
    /// The statement's.
    fn run_owned(&mut self, schema: &str, sql: &str, args: &[Value]) -> Result<()>;
}

/// \[EXTENSION\] A procedure written in Rust.
pub trait Procedure: Send + Sync + Debug {
    /// Its name, as an identifier folds to upper case.
    fn name(&self) -> &str;

    /// The declared types of its parameters, all of mode IN, when it is
    /// invoked with `count` arguments; none if it takes no such count.
    fn parameters(&self, count: usize) -> Option<Vec<DataType>>;

    /// The declared types of its parameters when its arguments are of the
    /// types `args`, bound as `parameters` types them, if they choose
    /// others: of a procedure of one name and count of parameters of
    /// several types [ISO/IEC 9075-2:2023 §9.6]. By default none.
    fn overload(&self, _args: &[DataType]) -> Option<Vec<DataType>> {
        None
    }

    /// Runs it of `args` in `cx`.
    ///
    /// # Errors
    ///
    /// The procedure's own; it is undone with the CALL.
    fn call(&self, args: &[Value], cx: &mut dyn ProcedureContext) -> Result<()>;

    /// Of one invoked as a function too, the type of its value when it is
    /// invoked with `count` arguments: none, by default, of one that is
    /// no function. Such a function possibly modifies SQL-data, so it is
    /// invoked where an SQL-session runs it [ISO/IEC 9075-2:2023 §9.18
    /// GR6]; a CALL does not invoke it.
    fn returns(&self, count: usize) -> Option<DataType> {
        let _ = count;
        None
    }

    /// Its value, invoked as a function of `args` in `cx`: by default the
    /// null value, once `call` has run.
    ///
    /// # Errors
    ///
    /// The function's own; it is undone with the statement.
    fn evaluate(&self, args: &[Value], cx: &mut dyn ProcedureContext) -> Result<Value> {
        self.call(args, cx).map(|()| Value::Null)
    }
}

/// The row type of a table function's rows.
pub fn row_type(f: &dyn TableFunction) -> DataType {
    let fields = f.columns().into_iter().map(|(name, data_type)| Field { name, data_type, unnamed: false }).collect();
    DataType::row(fields)
}

/// The invocation of a table function, as the scalar function whose
/// value is the multiset of its rows: of its arguments, then the
/// multisets of the rows of the queries it reads.
#[derive(Debug)]
pub struct TableCall {
    /// The table function.
    pub function: Arc<dyn TableFunction>,
    /// The count of its own arguments, before the queries' rows.
    pub arguments: usize,
}

impl TableCall {
    /// It as a scalar function.
    pub fn scalar(function: Arc<dyn TableFunction>, arguments: usize) -> ScalarRef {
        ScalarRef(Arc::new(TableCall { function, arguments }))
    }
}

impl ScalarFunction for TableCall {
    fn name(&self) -> &str {
        self.function.name()
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::multiset(row_type(&*self.function)))
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let (own, queried) = args.split_at(self.arguments.min(args.len()));
        let read = queried.iter().map(rows_of).collect::<Result<Vec<_>>>()?;
        let made = self.function.rows(own, &read)?;
        if made.len() > usize::try_from(MAX_ARRAY_CARDINALITY).unwrap_or(usize::MAX).saturating_mul(1024) {
            return Err(SqlError::new(Code::Unsupported, format!("{} returns more rows than a table function may", self.name())));
        }
        let width = self.function.columns().len();
        let rows = made.into_iter().map(|row| check_width(row, width, self.name())).collect::<Result<Vec<_>>>()?;
        Ok(Value::Multiset(rows.into()))
    }
    fn deterministic(&self) -> bool {
        false
    }
}

/// \[EXTENSION\] The invocation of a table function as a function
/// (`TableFunction::scalar`): the first column of its first row, the null
/// value of none.
#[derive(Debug)]
pub struct ValueCall(pub TableCall);

impl ScalarFunction for ValueCall {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        let first = self.0.function.columns().into_iter().next();
        first.map(|(_, t)| t).ok_or_else(|| SqlError::new(Code::Api, format!("{} returns no column", self.name())))
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        let Value::Multiset(rows) = self.0.call(args)? else { return Ok(Value::Null) };
        let Some(Value::Row(fields)) = rows.iter().next() else { return Ok(Value::Null) };
        Ok(fields.first().cloned().unwrap_or(Value::Null))
    }
    fn deterministic(&self) -> bool {
        false
    }
}

/// A row a table function made, of `width` columns, as a row value.
fn check_width(row: Vec<Value>, width: usize, name: &str) -> Result<Value> {
    if row.len() != width {
        return Err(SqlError::new(Code::Api, format!("{name} made a row of {} columns, not {width}", row.len())));
    }
    Ok(Value::Row(row.into()))
}

/// The rows of a query a table function reads, given as a multiset of
/// its rows, each a row value or, of one column, the value.
fn rows_of(v: &Value) -> Result<Vec<Vec<Value>>> {
    if v.is_null() {
        return Ok(Vec::new());
    }
    let Value::Multiset(items) = v else { return Err(SqlError::new(Code::Corrupt, format!("a table function read {v:?}, no multiset"))) };
    Ok(items.iter().map(|item| if let Value::Row(fields) = item { fields.to_vec() } else { vec![item.clone()] }).collect())
}

// Unit tests: a table function invoked, its rows and its value.
#[cfg(test)]
mod tests;
