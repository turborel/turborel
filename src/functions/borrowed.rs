//! Compatibility of the optional borrowed scalar entry point.

use std::sync::Mutex;

use super::*;
use crate::error::{Code, fail};

struct Recorded(Mutex<Vec<Vec<String>>>);

impl ScalarFunction for Recorded {
    fn name(&self) -> &'static str {
        "BORROWED_ARGUMENTS"
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Boolean)
    }

    fn call(&self, args: &[Value]) -> Result<Value> {
        self.0
            .lock()
            .map_err(|_| crate::error::SqlError::new(Code::Api, "[DESIGN] the test log is poisoned"))?
            .push(args.iter().map(ToString::to_string).collect());
        if args.iter().any(Value::is_null) {
            return fail(Code::Data, "[DESIGN] the test function refuses NULL");
        }
        Ok(Value::Bool(true))
    }
}

/// Verifies: [DESIGN] borrowed scalar calls preserve argument order and
/// invoke an existing implementation exactly once, for every list storage
/// path, including failing calls. Existing plug-ins need no new method.
#[test]
fn default_borrowed_calls_preserve_arguments_side_effects_and_errors() {
    let function = Recorded(Mutex::new(Vec::new()));
    for count in 0_usize..=5 {
        let values: Vec<_> = (0..count).map(|n| Value::Char(n.to_string())).collect();
        let refs: Vec<_> = values.iter().collect();
        assert!(matches!(function.call_borrowed(&refs, None).unwrap(), Value::Bool(true)));
        assert_eq!(function.0.lock().unwrap().pop().unwrap(), values.iter().map(ToString::to_string).collect::<Vec<_>>());
        for failed in 0..count {
            let mut refs = refs.clone();
            refs[failed] = &Value::Null;
            assert_eq!(function.call_borrowed(&refs, None).unwrap_err().code, Code::Data);
            assert_eq!(function.0.lock().unwrap().pop().unwrap(), refs.iter().map(ToString::to_string).collect::<Vec<_>>());
        }
        assert!(function.0.lock().unwrap().is_empty());
    }
}
