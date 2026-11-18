//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §12.3.17, §12.3.18] The
//! routines that read a network and change nothing: ST_ValidLogicalNet
//! and ST_ValidSpatialNet, which return tables. Each is a table function
//! written in Rust that reads the network's two views, named by the
//! network's name, which is so known as the invocation is bound: a literal
//! or a parameter (RULES.md D125).

use std::sync::Arc;

use super::{INTEGER, from_rows, int_value, name_type, network_name, read_failed, reads};
use crate::error::{Code, Result, SqlError};
use crate::functions::builtin::TableFunction;
use crate::spatial::network::{Problem, logical_problems, spatial_problems};
use crate::value::{DataType, Value};

/// A routine of §12.3 that reads a network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetReader {
    /// [§12.3.17] ST_ValidLogicalNet.
    ValidLogicalNet,
    /// [§12.3.18] ST_ValidSpatialNet.
    ValidSpatialNet,
}

/// The routines, built in.
pub fn tables() -> Vec<Arc<dyn TableFunction>> {
    [NetReader::ValidLogicalNet, NetReader::ValidSpatialNet].into_iter().map(|r| -> Arc<dyn TableFunction> { Arc::new(r) }).collect()
}

/// [§12.3.17] The length of the column ERROR.
const ERROR_LENGTH: u32 = 30;

impl NetReader {
    /// The subclause of its Description.
    fn rule(self) -> &'static str {
        match self {
            Self::ValidLogicalNet => "§12.3.17 Description",
            Self::ValidSpatialNet => "§12.3.18 Description",
        }
    }
}

/// The row of `p`.
fn row(p: &Problem) -> Vec<Value> {
    let optional = |n: Option<i32>| n.map_or(Value::Null, int_value);
    vec![Value::Char(p.error.to_owned()), optional(p.primitive1), optional(p.primitive2)]
}

impl TableFunction for NetReader {
    fn name(&self) -> &str {
        match self {
            Self::ValidLogicalNet => "ST_VALIDLOGICALNET",
            Self::ValidSpatialNet => "ST_VALIDSPATIALNET",
        }
    }

    fn parameters(&self, count: usize) -> Option<Vec<DataType>> {
        (count == 1).then(|| vec![name_type()])
    }

    fn columns(&self) -> Vec<(String, DataType)> {
        vec![("ERROR".to_owned(), DataType::varying(ERROR_LENGTH)), ("PRIMITIVE1".to_owned(), INTEGER), ("PRIMITIVE2".to_owned(), INTEGER)]
    }

    /// The two views of the network named. Reading: the name is known as
    /// the invocation is bound, as the tables a query reads are.
    fn reads(&self, known: &[Option<&Value>]) -> Result<Vec<String>> {
        let Some(given) = known.first() else { return Ok(Vec::new()) };
        let Some(v) = given else {
            let msg = format!(
                "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) {}] the network's name is not known as the invocation is bound: give a literal or a parameter",
                self.rule()
            );
            return Err(SqlError::new(Code::SyntaxRule, msg));
        };
        Ok(reads(&network_name(Some(v), self.rule())?))
    }

    fn read_failed(&self, e: SqlError) -> SqlError {
        read_failed(e)
    }

    fn rows(&self, _: &[Value], read: &[Vec<Vec<Value>>]) -> Result<Vec<Vec<Value>>> {
        let net = from_rows(read)?;
        let problems = match self {
            Self::ValidLogicalNet => logical_problems(&net)?,
            Self::ValidSpatialNet => spatial_problems(&net)?,
        };
        Ok(problems.iter().map(row).collect())
    }
}
