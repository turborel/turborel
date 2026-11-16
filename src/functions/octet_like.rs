//! [ISO/IEC 9075-2:2023 §8.5 GR4] `<octet like predicate>` (Features T022,
//! T042): bound as this function of the match value, the pattern and the
//! escape octet, compared to 1, as LIKE under a collation of contractions
//! is (`binder/collation.rs`). A binary large object match value is read
//! a chunk at a time; a pattern or escape octet of one is held whole up
//! to a chunk (RULES.md D79).

use crate::error::{Code, Result, fail};
use crate::functions::ScalarFunction;
use crate::value::lob::{CHUNK_OCTETS, LobKind, TempLobs};
use crate::value::{DataType, Decimal, OctetPattern, Spelled, Value};

/// Its name, as plans show it.
const OCTET_LIKE: &str = "OCTET LIKE";

/// [§8.5 GR4] One if the match value, the pattern and the escape octet if
/// given, its arguments, match, zero if not, null if one is null \[GR4a\].
pub struct OctetLike;

/// A binary string's octets: of a large object, held whole up to a chunk,
/// `what` of LIKE.
///
/// # Errors
///
/// SQLCODE -900 if it is a large object longer than a chunk; -910 if it
/// is no binary string.
fn held(v: &Value, what: &str) -> Result<Vec<u8>> {
    if let Value::Binary(b) = v {
        return Ok(b.bytes().to_vec());
    }
    let Value::Lob(lob) = v else { return fail(Code::Api, "[DESIGN] an octet LIKE was bound of other than binary strings") };
    let most = u64::try_from(CHUNK_OCTETS).unwrap_or(u64::MAX);
    lob.bounded(most)?
        .map_or_else(|| fail(Code::Unsupported, format!("[DESIGN] {what} is a large object of more than {CHUNK_OCTETS} octets")), Ok)
}

/// Whether `value`, a binary string not null, matches `pattern`.
///
/// # Errors
///
/// As reading a large object fails.
fn matches(pattern: &OctetPattern, value: &Value) -> Result<bool> {
    if let Value::Lob(lob) = value
        && lob.kind() == LobKind::Binary
    {
        return pattern.matches_lob(lob);
    }
    if let Value::Binary(b) = value {
        return Ok(pattern.matches(b.bytes()));
    }
    fail(Code::Api, "[DESIGN] an octet LIKE was bound of other than binary strings")
}

impl ScalarFunction for OctetLike {
    fn name(&self) -> &str {
        OCTET_LIKE
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Ok(DataType::Exact { precision: 1, scale: 0, spelled: Spelled::Smallint })
    }
    fn call(&self, args: &[Value]) -> Result<Value> {
        self.call_with(args, None)
    }
    fn call_with(&self, args: &[Value], _: Option<&dyn TempLobs>) -> Result<Value> {
        if args.iter().any(Value::is_null) {
            return Ok(Value::Null);
        }
        let (Some(value), Some(pattern)) = (args.first(), args.get(1)) else {
            return fail(Code::Api, "[DESIGN] an octet LIKE has a match value and a pattern");
        };
        let escape = args.get(2).map(|e| held(e, "an escape octet")).transpose()?;
        let pattern = OctetPattern::new(&held(pattern, "a LIKE pattern")?, escape.as_deref())?;
        Ok(Value::Exact(Decimal::new(i128::from(matches(&pattern, value)?), 0)))
    }
}
