//! GREATEST and LEAST [ISO/IEC 9075-2:2023 §6.15]: of their arguments,
//! each cast to the result type as it is bound, the greatest or the least,
//! compared under the collation they are ordered by.

use std::cmp::Ordering;

use super::ScalarFunction;
use crate::error::Result;
use crate::value::collation::CollationRef;
use crate::value::types::rows_hold;
use crate::value::{CompOp, DataType, Value, compare};

/// [ISO/IEC 9075-2:2023 §6.15] GREATEST's name, no key word of SQL-92.
pub const GREATEST: &str = "GREATEST";
/// [ISO/IEC 9075-2:2023 §6.15] LEAST's name, no key word of SQL-92.
pub const LEAST: &str = "LEAST";

/// GREATEST or LEAST of values of `result`, ordered under `collation` if
/// one is not the default.
pub struct GreatestOrLeast {
    /// Whether it is GREATEST.
    pub greatest: bool,
    /// The type of its arguments and its result [SR6, SR7].
    pub result: DataType,
    /// The collation its arguments are ordered by [§9.14 SR1h, §9.15], if
    /// not the default.
    pub collation: Option<CollationRef>,
}

impl GreatestOrLeast {
    /// What `v` is ordered by: its sort key under the collation, else
    /// itself.
    fn key(&self, v: &Value) -> Value {
        self.collation.as_ref().map_or_else(|| v.clone(), |c| c.sort_key(v))
    }
}

impl ScalarFunction for GreatestOrLeast {
    fn name(&self) -> &str {
        if self.greatest { GREATEST } else { LEAST }
    }

    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(self.result.clone())
    }

    /// \[GR1\] Null if any argument is; else \[GR2, GR3\] the first argument
    /// R that is >= (of GREATEST) or <= (of LEAST) every other, so of two
    /// equal the first; null if none is, which of values each comparable
    /// with each happens of rows alone, whose fields may be null.
    fn call(&self, args: &[Value]) -> Result<Value> {
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        // [GR2, GR3] of rows, the one that is >= (or <=) each as the
        // comparison of rows has it [§8.2 GR7], else null.
        if let Some(Value::Row(_)) = args.first() {
            let op = if self.greatest { CompOp::Ge } else { CompOp::Le };
            let holds = |x: &Value, y: &Value| if let (Value::Row(a), Value::Row(b)) = (x, y) { rows_hold(a, op, b) } else { None };
            let best = args.iter().find(|x| args.iter().all(|y| holds(x, y) == Some(true)));
            return Ok(best.cloned().unwrap_or(Value::Null));
        }
        let wanted = if self.greatest { Ordering::Greater } else { Ordering::Less };
        let mut best: Option<(&Value, Value)> = None;
        for v in args {
            let key = self.key(v);
            best = match best {
                None => Some((v, key)),
                Some((was, was_key)) => match compare(&key, &was_key) {
                    Some(o) => Some(if o == wanted { (v, key) } else { (was, was_key) }),
                    None => return Ok(Value::Null),
                },
            };
        }
        Ok(best.map_or(Value::Null, |(v, _)| v.clone()))
    }
}
