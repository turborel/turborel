//! The mappings of SQL to XML Schema [ISO/IEC 9075-14:2006 draft §9.4 to
//! §9.7]: SQL data types and domains, each to its XML Name and to the XML
//! Schema data type an XML Schema document defines of it (`map/types.rs`,
//! `map/durations.rs`).

mod durations;
mod types;

pub use types::{Declared, XDT_NAMESPACE, XDT_PREFIX, declared_name, definitions, mappable, named_type, type_name};
