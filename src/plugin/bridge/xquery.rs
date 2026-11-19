//! \[EXTENSION\] A plug-in's XQuery function (RULES.md D222), an
//! `XQueryFunction` of atomic parameters and result, each of the XML
//! Schema type its manifest's SQL type maps to: a character string
//! `xs:string`, an exact number of no scale `xs:integer`, of a scale
//! `xs:decimal`, an approximate number `xs:double`, BOOLEAN `xs:boolean`;
//! each zero or one item. A call is one CALL of one row of arguments,
//! each the SQL value of its atomic value's lexical form, an empty
//! sequence null; the value answered is the result's lexical form.

use crate::error::Result;
use crate::plugin::errors::invalid_manifest;
use crate::plugin::payload::XQUERY_CALL;
use crate::plugin::pool::CallContext;
use crate::plugin::values::values;
use crate::plugin::wire::put_rows;
use crate::value::{CharUnits, DataType, Value, cast};
use crate::xml::{AtomicType, Item, QName, Sequence};
use crate::xquery::{Context, ItemKind, Occurrence, SchemaType, SeqType, XQueryFunction};

use super::{Link, answer, counted};

/// \[DESIGN\] The longest lexical form of a result read, in characters.
const LEXICAL_LENGTH: u32 = 1_048_576;

/// A plug-in's XQuery function.
#[derive(Debug)]
pub struct PluginXQuery {
    link: Link,
    name: QName,
    /// Each parameter's SQL type, its XQuery type.
    parameters: Vec<DataType>,
    types: Vec<SeqType>,
    result: (AtomicType, SeqType),
}

/// The atomic type of `t`, if it maps to one.
fn atomic_of(t: &DataType) -> Option<AtomicType> {
    Some(match t {
        DataType::Character { .. } | DataType::Varying { .. } => AtomicType::String,
        DataType::Exact { scale: 0, .. } => AtomicType::Integer,
        DataType::Exact { .. } => AtomicType::Decimal,
        DataType::Approximate { .. } => AtomicType::Double,
        DataType::Boolean => AtomicType::Boolean,
        DataType::DecFloat { .. }
        | DataType::Binary { .. }
        | DataType::VarBinary { .. }
        | DataType::Lob { .. }
        | DataType::Date
        | DataType::Time { .. }
        | DataType::Timestamp { .. }
        | DataType::Interval(_)
        | DataType::Array(_)
        | DataType::Multiset(_)
        | DataType::Row(_)
        | DataType::Distinct(_)
        | DataType::Structured(_)
        | DataType::Reference(_)
        | DataType::Json
        | DataType::Xml(_)
        | DataType::Extension(_) => return t.is_character().then_some(AtomicType::String),
    })
}

/// `xs:T?` of `t`.
fn optional(t: AtomicType) -> SeqType {
    SeqType::Of(ItemKind::Atomic(SchemaType::Atomic(t)), Occurrence::Optional)
}

/// `{uri}local`, split; none if it is not so written.
fn expanded(name: &str) -> Option<QName> {
    let (uri, local) = name.strip_prefix('{')?.split_once('}')?;
    let ok = !uri.is_empty() && !local.is_empty();
    ok.then(|| QName { prefix: None, uri: Some(uri.into()), local: local.into() })
}

impl PluginXQuery {
    /// The function `link` names, `{uri}local`, of `parameters` and `result`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 if its name is not so written, or a type maps to no
    /// atomic type.
    pub fn new(link: Link, parameters: Vec<DataType>, result: &DataType) -> Result<PluginXQuery> {
        let name = expanded(&link.object).ok_or_else(|| invalid_manifest("an XQuery function is named '{uri}local'"))?;
        let atomic = |t: &DataType| atomic_of(t).ok_or_else(|| invalid_manifest(&format!("{t} is of no atomic type of XQuery")));
        let types = parameters.iter().map(|t| atomic(t).map(optional)).collect::<Result<_>>()?;
        let r = atomic(result)?;
        Ok(PluginXQuery { link, name, parameters, types, result: (r, optional(r)) })
    }

    /// The SQL value of an argument, of the type of its parameter.
    fn argument(s: &Sequence, t: &DataType) -> Result<Value> {
        match s.items().first() {
            Some(Item::Atomic(a)) => cast(Value::Char(a.lexical.to_string()), t),
            Some(Item::Node(_)) | None => Ok(Value::Null),
        }
    }
}

impl XQueryFunction for PluginXQuery {
    fn name(&self) -> &QName {
        &self.name
    }
    fn parameters(&self) -> &[SeqType] {
        &self.types
    }
    fn result(&self) -> &SeqType {
        &self.result.1
    }
    fn call(&self, context: &Context<'_>, arguments: Vec<Sequence>) -> Result<Sequence> {
        let row = arguments.iter().zip(&self.parameters).map(|(s, t)| Self::argument(s, t)).collect::<Result<Vec<_>>>()?;
        let mut payload = Vec::new();
        put_rows(&mut payload, &[row])?;
        let stop = || context.check();
        let got = self.link.call(XQUERY_CALL, payload, &mut CallContext { interrupt: Some(&stop), sql: None })?;
        let v = counted(answer(&got, values)?, 1)?.pop().unwrap_or(Value::Null);
        if v.is_null() {
            return Ok(Sequence::new(Vec::new()));
        }
        let text = cast(v, &DataType::Varying { length: LEXICAL_LENGTH, units: CharUnits::Characters })?;
        let Value::Char(text) = text else { return Ok(Sequence::new(Vec::new())) };
        let made = crate::xml::from_lexical(&text, self.result.0)?;
        Ok(Sequence::new(made.map(Item::Atomic).into_iter().collect()))
    }
}
