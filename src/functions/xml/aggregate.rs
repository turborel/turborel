//! [ISO/IEC 9075-14:2006 draft §11.2] XMLAGG: the concatenation of the
//! XML values of a group, in the order of its sort keys; RETURNING
//! CONTENT a document node of it, as XMLDOCUMENT makes one \[SR5\]. The
//! aggregate makes the document node itself, so it is one aggregate,
//! over a window too.

use std::sync::Arc;

use super::super::arrays::{by_keys, keyed, order_suffix, ordered_keys};
use super::super::{Accumulator, AggregateFunction};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::value::collation::CollationRef;
use crate::value::xml::{Typing, XmlType, XmlValue};
use crate::value::{DataType, SortOrder, Value};
use crate::xml::{MAX_STORED, Sequence, concatenate, document};

/// XMLAGG's name.
pub const XMLAGG: &str = "XMLAGG";

/// [§11.2 GR2] XMLAGG: its argument a row of the XML value and its sort
/// keys, each key in its direction, its nulls placed by its null
/// ordering, by its collation if it has one, as ARRAY_AGG's are.
#[derive(Debug)]
pub struct XmlAggregate {
    /// Per sort key, its direction and null ordering.
    pub orders: Vec<SortOrder>,
    /// Per sort key, the collation it orders by, if not the default.
    pub collations: Vec<Option<CollationRef>>,
    /// Whether it is RETURNING CONTENT.
    pub content: bool,
}

fn no_argument() -> SqlError {
    SqlError::new(Code::Api, "[DESIGN] XMLAGG was bound without its row of an XML value and sort keys")
}

impl AggregateFunction for XmlAggregate {
    fn name(&self) -> &str {
        XMLAGG
    }

    /// [§11.2 SR2] Of an XML value [§6.9 SR1], ordered by keys an
    /// ordering operation takes: RETURNING SEQUENCE, XML(SEQUENCE)
    /// \[SR2b\]; RETURNING CONTENT, XML(CONTENT(UNTYPED)) of an UNTYPED
    /// value \[SR2a i\], XML(CONTENT(XMLSCHEMA)) of what the value's type
    /// of XMLSCHEMA indicates \[SR2a ii\], else XML(CONTENT(ANY)) \[SR2a
    /// iii\]; its document node made of that type, its nodes' types kept.
    fn result_type(&self, arg: Option<DataType>) -> Result<DataType> {
        let Some(DataType::Row(row)) = arg else { return Err(no_argument()) };
        let value = row.fields.first().map(|f| f.data_type.clone()).ok_or_else(no_argument)?;
        let DataType::Xml(t) = value else {
            return Err(SqlError::new(
                Code::SyntaxRule,
                format!("[ISO/IEC 9075-14:2006 draft §6.9 SR1] XMLAGG takes XML values, not values of {value}"),
            ));
        };
        ordered_keys(&row, XMLAGG)?;
        let typing = match (t.is_untyped(), t.schema()) {
            (true, _) => Typing::Untyped,
            (false, Some(named)) => Typing::Schema(Arc::new(named.clone())),
            (false, None) => Typing::Any,
        };
        Ok(DataType::Xml(if self.content { XmlType::Content(typing) } else { XmlType::Sequence }))
    }

    fn accumulator(&self, _: DataType) -> Box<dyn Accumulator> {
        let (orders, collations) = (self.orders.clone(), self.collations.clone());
        Box::new(Gathered { rows: Vec::new(), held: 0, orders, collations, content: self.content })
    }

    fn label_suffix(&self) -> String {
        order_suffix(&self.orders, &self.collations)
    }
}

/// The rows of an XML value not null and its keys so far, and the octets
/// their values hold.
struct Gathered {
    rows: Vec<Arc<[Value]>>,
    held: usize,
    orders: Vec<SortOrder>,
    collations: Vec<Option<CollationRef>>,
    content: bool,
}

impl Accumulator for Gathered {
    /// [§11.2 GR2c] A row whose XML value is null is removed; [GR1] data
    /// exception, XML value overflow, once the values held pass
    /// `MAX_STORED` octets, as no value longer is kept (RULES.md D106).
    fn update(&mut self, v: &Value) -> Result<()> {
        let Value::Row(row) = v else { return Err(no_argument()) };
        let Some(Value::Xml(x)) = row.first() else { return Ok(()) };
        self.held = self.held.saturating_add(x.held_bytes());
        if self.held > MAX_STORED {
            return Err(SqlError::new(
                Code::Data,
                format!("[ISO/IEC 9075-14:2006 draft §11.2 GR1] XMLAGG's values hold more than {MAX_STORED} octets"),
            )
            .with_state(SqlState::XML_VALUE_OVERFLOW));
        }
        self.rows.push(keyed(row, &self.collations)?);
        Ok(())
    }

    /// [§11.2 GR2d, GR2e] The null value of no rows; else the values
    /// concatenated [§10.14] in the order of their keys, stably, and of
    /// no keys in the order they came; RETURNING CONTENT, `document {
    /// $EXPR }` of that [§6.13 GR8] \[SR5\].
    fn finish(&self) -> Result<Value> {
        let mut rows: Vec<&Arc<[Value]>> = self.rows.iter().collect();
        rows.sort_by(|a, b| by_keys(a, b, &self.orders));
        let mut values: Vec<Arc<Sequence>> = Vec::with_capacity(rows.len());
        for row in rows {
            if let Some(Value::Xml(x)) = row.first() {
                values.push(x.sequence()?);
            }
        }
        let Some(made) = concatenate(values.iter().map(|s| Some(&**s))) else { return Ok(Value::Null) };
        Ok(Value::Xml(XmlValue::of(if self.content { document(&made)? } else { made })))
    }
}

#[cfg(test)]
mod tests;
