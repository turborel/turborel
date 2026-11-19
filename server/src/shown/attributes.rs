//! \[DESIGN\] INFORMATION_SCHEMA.CONNECTION_ATTRIBUTES (RULES.md D191):
//! each attribute each connection's client gave in Hello, by the
//! connection's number, its rows the database's owner's alone, as
//! CONNECTIONS' are. The attribute `application` is shown in CONNECTIONS
//! too.

use std::sync::Arc;

use turborel::catalog::ColumnDesc;
use turborel::error::Result;
use turborel::storage::Row;
use turborel::system::{Seen, SystemTable};
use turborel::value::Value;
use turborel_protocol::{Attributes, MAX_ATTRIBUTE_NAME, MAX_ATTRIBUTE_VALUE};

use super::{Shown, count, counted, text};

/// \[DESIGN\] The attribute whose value CONNECTIONS shows as APPLICATION:
/// the program its client names itself.
pub(crate) const APPLICATION: &str = "application";
/// The characters of a column of attribute values: as many as the bytes
/// of the longest value taken, which no value's characters pass.
pub(crate) fn value_length() -> u32 {
    u32::try_from(MAX_ATTRIBUTE_VALUE).unwrap_or(u32::MAX)
}

/// The characters of a column of attribute names, as `value_length`.
fn name_length() -> u32 {
    u32::try_from(MAX_ATTRIBUTE_NAME).unwrap_or(u32::MAX)
}

/// The value of `attributes`' `application`, or null.
pub(crate) fn application(attributes: &Attributes) -> Value {
    attributes.iter().find(|(name, _)| name == APPLICATION).map_or(Value::Null, |(_, value)| Value::Char(value.clone()))
}

/// \[DESIGN\] INFORMATION_SCHEMA.CONNECTION_ATTRIBUTES.
pub(crate) struct ConnectionAttributes(pub(crate) Arc<Shown>);

impl SystemTable for ConnectionAttributes {
    fn name(&self) -> &'static str {
        "CONNECTION_ATTRIBUTES"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        vec![counted("CONNECTION_ID"), text("ATTRIBUTE_NAME", name_length(), true), text("ATTRIBUTE_VALUE", value_length(), true)]
    }

    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        if !seen.is_owner() {
            return Ok(Vec::new());
        }
        let connections: Vec<(u64, Arc<Attributes>)> = self.0.locked().iter().map(|(id, e)| (*id, Arc::clone(&e.attributes))).collect();
        Ok(connections
            .into_iter()
            .flat_map(|(id, attributes)| {
                attributes
                    .iter()
                    .map(|(name, value)| vec![count(id), Value::Char(name.clone()), Value::Char(value.clone())])
                    .collect::<Vec<_>>()
            })
            .collect())
    }
}
