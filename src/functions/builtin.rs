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
//! D-SPATIAL-ROUTINES).
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
}

/// \[EXTENSION\] What a procedure written in Rust runs in: the SQL-session
/// and transaction of the CALL that invokes it.
pub trait ProcedureContext {
    /// Runs the SQL statement `sql`, its `<dynamic parameter
    /// specification>`s taking `args` in order, as a statement of the
    /// procedure.
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
}

/// \[EXTENSION\] A procedure written in Rust.
pub trait Procedure: Send + Sync + Debug {
    /// Its name, as an identifier folds to upper case.
    fn name(&self) -> &str;

    /// The declared types of its parameters, all of mode IN, when it is
    /// invoked with `count` arguments; none if it takes no such count.
    fn parameters(&self, count: usize) -> Option<Vec<DataType>>;

    /// Runs it of `args` in `cx`.
    ///
    /// # Errors
    ///
    /// The procedure's own; it is undone with the CALL.
    fn call(&self, args: &[Value], cx: &mut dyn ProcedureContext) -> Result<()>;
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
