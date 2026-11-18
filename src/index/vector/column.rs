//! \[EXTENSION\] The column a vector index holds and its keys: one dense
//! VECTOR column of a fixed dimension count and format (the vector design
//! document R3.6, R10.3), its values read as `f32`s for the engines, a
//! null not indexed (R10.6).
//!
//! Keys are projected (RULES.md D85) to one value, the same of every
//! vector: the engines keep no copy of the column's value to give back,
//! the quantized ones not even its `f32`s, and an approximate search's
//! candidates are measured again on the row anyway (D94). So `rows`
//! of a vector gives the rows of that vector exactly, and a search's
//! entry stands for its row, its reader taking the row as it sees it.

use std::sync::Arc;

use crate::catalog::{IndexDesc, QualifiedName};
use crate::error::{Code, Result, SqlError};
use crate::index::{Key, KeyProjection, NearestQuery, NearestSupport};
use crate::value::{Binary, DataType, Value};
use crate::vector::{Element, fixed_shape, shape_of, to_f32, vector};

/// The SQLCODE -900 refusal of a vector index method `method`.
pub(crate) fn refused(method: &str, detail: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[EXTENSION] the {method} index method {detail}"))
}

/// What a vector index's column fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Column {
    /// Its dimension count.
    pub(crate) dims: u32,
    /// Its element format.
    pub(crate) element: Element,
}

impl Column {
    /// Its dimension count, as the engines count.
    pub(crate) fn width(self) -> usize {
        usize::try_from(self.dims).unwrap_or(usize::MAX)
    }

    /// The `f32`s of `value`, a vector of this column's shape, or none for
    /// a null.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 for a value that is no vector of this shape.
    pub(crate) fn floats(self, method: &str, value: &Value) -> Result<Option<Vec<f32>>> {
        if value.is_null() {
            return Ok(None);
        }
        let v = vector(value)?;
        if v.dims != self.dims || v.element != self.element || v.indices.is_some() {
            return Err(refused(method, "was given a vector not of its column's dimensions and format"));
        }
        let mut out = Vec::with_capacity(self.width());
        to_f32(v, &mut out);
        Ok(Some(out))
    }

    /// The `f32`s of a query value, if it is a vector of this shape: else
    /// none, the search declined, for the scan to raise what the call
    /// raises (D15).
    pub(crate) fn query(self, value: &Value) -> Option<Vec<f32>> {
        let v = vector(value).ok()?;
        if v.dims != self.dims || v.element != self.element || v.indices.is_some() {
            return None;
        }
        let mut out = Vec::with_capacity(self.width());
        to_f32(v, &mut out);
        Some(out)
    }
}

/// The column `columns` describes, if a vector index of `method` takes
/// it: one dense VECTOR column of a fixed dimension count and format, not
/// FLOAT64, whose values the engines' binary32 nodes could not hold.
///
/// # Errors
///
/// SQLCODE -900 saying what is wrong.
pub(crate) fn column_of(method: &str, columns: &[DataType]) -> Result<Column> {
    let [t] = columns else { return Err(refused(method, "indexes one column")) };
    if shape_of(t).is_none() {
        return Err(refused(method, "indexes a VECTOR column"));
    }
    let Some((dims, element)) = fixed_shape(t) else {
        return Err(refused(method, "indexes a DENSE VECTOR column of a fixed dimension count and format (R3.6, R10.3)"));
    };
    if element == Element::Float64 {
        return Err(refused(method, "keeps vectors in binary32, so indexes no FLOAT64 column"));
    }
    Ok(Column { dims, element })
}

/// Refuses an index `desc` describes that a vector index method `method`
/// does not make: a UNIQUE one, as vectors are not compared (R3.11), or
/// one holding nulls, as nulls are not indexed (R10.6).
///
/// # Errors
///
/// SQLCODE -900.
pub(crate) fn plain(method: &str, desc: &IndexDesc) -> Result<()> {
    if desc.unique || desc.nulls {
        return Err(refused(method, "makes no UNIQUE or NULLS NOT DISTINCT index"));
    }
    Ok(())
}

/// How a vector index method serves `query` on `columns`: approximately,
/// of a call of a vector metric on its one fixed-shape column, asked
/// approximately and ascending; else not. Whether the call's metric is
/// the index's is the planner's to ask (`IndexMethod::metric`).
pub(crate) fn approximate_support(method: &str, query: &NearestQuery, columns: &[DataType]) -> NearestSupport {
    let served = query.approximate.is_some() && query.metric.is_some() && !query.descending && query.column == 0;
    if served && column_of(method, columns).is_ok() { NearestSupport::Approximate } else { NearestSupport::No }
}

/// A descriptor of an index of `method` with every option its default,
/// for `create` and `open`, which are given none.
pub(crate) fn default_desc(method: &str) -> IndexDesc {
    IndexDesc {
        name: String::new(),
        table: QualifiedName::of_schema(""),
        columns: vec![0],
        method: method.to_owned(),
        unique: false,
        options: Vec::new(),
        collations: Vec::new(),
        nulls: false,
        expressions: Vec::new(),
    }
}

/// The projection of every vector: one value, a binary string of none.
fn projected() -> Key {
    Key(vec![Value::Binary(Binary::new(Vec::new()))])
}

/// \[EXTENSION\] Projects every vector to one value, and a null to a null
/// (D85): see the module's documentation.
pub(crate) struct Projection;

impl KeyProjection for Projection {
    fn project(&self, key: &Key) -> Result<Key> {
        let [v] = key.0.as_slice() else { return Err(SqlError::new(Code::Api, "[DESIGN] a vector index's key has one column")) };
        Ok(if v.is_null() { key.clone() } else { projected() })
    }
}

/// The projection a vector index declares.
pub(crate) fn projection() -> Arc<dyn KeyProjection> {
    Arc::new(Projection)
}

/// The projected key of every entry.
pub(crate) fn entry_key() -> Key {
    projected()
}
