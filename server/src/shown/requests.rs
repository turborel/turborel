//! \[DESIGN\] The attributes of the requests running (RULES.md D191): what
//! a Query or an Execute gave, read in SQL by the function
//! QUERY_ATTRIBUTE, which the server registers with the database it
//! serves. A request runs on one worker's thread from its start to its
//! reply, and a statement's expressions are evaluated on the thread that
//! runs it, so the attributes are kept by the thread running each
//! request, set as it begins and taken away as it ends; a cursor's
//! fetches read those of the Execute that opened it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::ThreadId;

use turborel::error::{Code, Result, SqlError};
use turborel::functions::ScalarFunction;
use turborel::value::{CharUnits, DataType, Value};
use turborel_protocol::{Attributes, MAX_ATTRIBUTE_VALUE};

/// \[DESIGN\] The function that reads a request's attribute by its name.
pub const QUERY_ATTRIBUTE: &str = "QUERY_ATTRIBUTE";
/// \[DESIGN\] The attribute whose value is the request's trace context,
/// which the record of a slow statement carries.
pub const TRACEPARENT: &str = "traceparent";

/// The trace context `attributes` give, if they give one.
pub fn trace(attributes: Option<&Attributes>) -> Option<String> {
    attributes?.iter().find(|(name, _)| name == TRACEPARENT).map(|(_, value)| value.clone())
}

/// The attributes of the request each worker's thread runs.
#[derive(Default)]
pub struct Requests(Mutex<HashMap<ThreadId, Arc<Attributes>>>);

/// The attributes of the request this thread runs, taken away as this is
/// dropped.
pub struct Running<'a>(&'a Requests);

impl Requests {
    /// `attributes`, those of the request this thread runs until what is
    /// returned is dropped; none set for a request of none.
    pub fn running(&self, attributes: Option<&Arc<Attributes>>) -> Option<Running<'_>> {
        let attributes = attributes?;
        self.locked().insert(std::thread::current().id(), Arc::clone(attributes));
        Some(Running(self))
    }

    /// The value of the attribute `name` of the request this thread runs.
    fn value(&self, name: &str) -> Option<String> {
        let all = self.locked().get(&std::thread::current().id()).cloned()?;
        all.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    }

    fn locked(&self) -> MutexGuard<'_, HashMap<ThreadId, Arc<Attributes>>> {
        // Each thread's attributes are put and taken whole.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.locked().remove(&std::thread::current().id());
    }
}

/// \[DESIGN\] QUERY_ATTRIBUTE(name): the value of the attribute `name` of
/// the request running the statement, a character string; null if it
/// gave none of that name, or the name is null.
pub struct QueryAttribute(pub Arc<Requests>);

impl ScalarFunction for QueryAttribute {
    fn name(&self) -> &str {
        QUERY_ATTRIBUTE
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        match args {
            [DataType::Character { .. } | DataType::Varying { .. }] => {
                let length = u32::try_from(MAX_ATTRIBUTE_VALUE).unwrap_or(u32::MAX);
                Ok(DataType::Varying { length, units: CharUnits::Characters })
            }
            _ => Err(SqlError::new(Code::SyntaxRule, "[DESIGN] QUERY_ATTRIBUTE takes one character string, the attribute's name")),
        }
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        let Some(Value::Char(name)) = args.first() else { return Ok(Value::Null) };
        Ok(self.0.value(name).map_or(Value::Null, |s| Value::Char(s.into())))
    }

    fn deterministic(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D191: a request's attributes are read on
    /// the thread that runs it while it runs, and gone once it ends; a
    /// request of none sets none.
    #[test]
    fn attributes_are_the_running_requests() {
        let requests = Arc::new(Requests::default());
        let function = QueryAttribute(Arc::clone(&requests));
        let name = [Value::Char("traceparent".into())];
        assert!(requests.running(None).is_none());
        let attributes = Arc::new(vec![("traceparent".to_owned(), "00-1-2-01".to_owned())]);
        assert_eq!(trace(Some(&attributes)).as_deref(), Some("00-1-2-01"));
        let running = requests.running(Some(&attributes));
        assert!(matches!(function.call(&name).unwrap(), Value::Char(v) if v == "00-1-2-01"));
        assert!(matches!(function.call(&[Value::Char("other".into())]).unwrap(), Value::Null));
        assert!(matches!(function.call(&[Value::Null]).unwrap(), Value::Null));
        drop(running);
        assert!(matches!(function.call(&name).unwrap(), Value::Null), "gone once it ends");
        assert!(function.return_type(&[DataType::Boolean]).is_err());
    }
}
