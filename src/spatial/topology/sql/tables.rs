//! [ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §11.3.5, §11.3.16, §11.3.19]
//! The routines that read a topology and change nothing: ST_GetFaceEdges
//! and ST_ValidateTopoGeo, which return tables, and ST_GetFaceGeometry,
//! which returns a value. Each is a table function written in Rust that
//! reads the topology's three views, named by the topology's name, which
//! is so known as the invocation is bound: a literal or a parameter
//! (RULES.md D125). ST_GetFaceGeometry is invoked as a
//! function, its value the one column of its one row.

use std::sync::Arc;

use super::{INTEGER, from_rows, int_value, name_type, read_failed, reads, required, topology_name};
use crate::error::{Code, Result, SqlError};
use crate::functions::builtin::TableFunction;
use crate::geometry::data_type_of;
use crate::geometry::model::Kind;
use crate::spatial::topology::{face_edges, face_geometry, validate};
use crate::value::{DataType, Value};

/// A routine of §11.3 that reads a topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopoReader {
    /// [§11.3.5] ST_GetFaceEdges.
    GetFaceEdges,
    /// [§11.3.16] ST_GetFaceGeometry.
    GetFaceGeometry,
    /// [§11.3.19] ST_ValidateTopoGeo.
    ValidateTopoGeo,
}

/// The routines, built in.
pub fn tables() -> Vec<Arc<dyn TableFunction>> {
    [TopoReader::GetFaceEdges, TopoReader::GetFaceGeometry, TopoReader::ValidateTopoGeo]
        .into_iter()
        .map(|r| -> Arc<dyn TableFunction> { Arc::new(r) })
        .collect()
}

/// [§11.3.19] The length of the column ERROR.
const ERROR_LENGTH: u32 = 30;

impl TopoReader {
    /// The subclause of its Description.
    fn rule(self) -> &'static str {
        match self {
            Self::GetFaceEdges => "§11.3.5 Description",
            Self::GetFaceGeometry => "§11.3.16 Description",
            Self::ValidateTopoGeo => "§11.3.19 Description",
        }
    }
}

impl TableFunction for TopoReader {
    fn name(&self) -> &str {
        match self {
            Self::GetFaceEdges => "ST_GETFACEEDGES",
            Self::GetFaceGeometry => "ST_GETFACEGEOMETRY",
            Self::ValidateTopoGeo => "ST_VALIDATETOPOGEO",
        }
    }

    fn parameters(&self, count: usize) -> Option<Vec<DataType>> {
        let types = match self {
            Self::GetFaceEdges | Self::GetFaceGeometry => vec![name_type(), INTEGER],
            Self::ValidateTopoGeo => vec![name_type()],
        };
        (types.len() == count).then_some(types)
    }

    fn columns(&self) -> Vec<(String, DataType)> {
        match self {
            Self::GetFaceEdges => vec![("SEQUENCE".to_owned(), INTEGER), ("EDGE".to_owned(), INTEGER)],
            Self::GetFaceGeometry => vec![("ST_GETFACEGEOMETRY".to_owned(), data_type_of(Kind::Surface))],
            Self::ValidateTopoGeo => vec![
                ("ERROR".to_owned(), DataType::varying(ERROR_LENGTH)),
                ("PRIMITIVE1".to_owned(), INTEGER),
                ("PRIMITIVE2".to_owned(), INTEGER),
            ],
        }
    }

    /// The three views of the topology named. Reading: the name is known
    /// as the invocation is bound, as the tables a query reads are.
    fn reads(&self, known: &[Option<&Value>]) -> Result<Vec<String>> {
        let Some(given) = known.first() else { return Ok(Vec::new()) };
        let Some(v) = given else {
            let msg = format!(
                "[ISO/IEC 13249-3:2016 (DIS, SC32 N2593) {}] the topology's name is not known as the invocation is bound: give a literal or a parameter",
                self.rule()
            );
            return Err(SqlError::new(Code::SyntaxRule, msg));
        };
        Ok(reads(&topology_name(Some(v), self.rule())?))
    }

    fn read_failed(&self, e: SqlError) -> SqlError {
        read_failed(e)
    }

    fn scalar(&self) -> bool {
        matches!(self, Self::GetFaceGeometry)
    }

    fn rows(&self, args: &[Value], read: &[Vec<Vec<Value>>]) -> Result<Vec<Vec<Value>>> {
        let topo = from_rows(read)?;
        let rule = format!("{} 2d", self.rule());
        match self {
            Self::GetFaceEdges => {
                let edges = face_edges(&topo, required(args.get(1), &rule)?)?;
                let numbered = (1_i32..).zip(edges);
                Ok(numbered.map(|(k, e)| vec![int_value(k), int_value(e)]).collect())
            }
            Self::GetFaceGeometry => Ok(vec![vec![face_geometry(&topo, required(args.get(1), &rule)?)?.to_value()]]),
            Self::ValidateTopoGeo => {
                let problems = validate(&topo)?;
                let optional = |n: Option<i32>| n.map_or(Value::Null, int_value);
                Ok(problems
                    .into_iter()
                    .map(|p| vec![Value::Char(p.error.to_owned()), optional(p.primitive1), optional(p.primitive2)])
                    .collect())
            }
        }
    }
}
