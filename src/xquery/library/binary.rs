//! The operators on binary values and NOTATION [XQueryFO §12, §13]: the
//! equality of two `xs:hexBinary` or two `xs:base64Binary` values, by
//! their octets, and of two `xs:NOTATION` values, code point by code
//! point.

use super::args::wrong_type;
use crate::error::Result;
use crate::xml::{Atomic, AtomicType, base64, equal, hex};

/// The octets of `a`, of `kind`.
fn octets(a: &Atomic, kind: AtomicType) -> Result<Vec<u8>> {
    let decoded = if a.kind != kind {
        None
    } else if kind == AtomicType::HexBinary {
        hex(&a.lexical)
    } else {
        base64(&a.lexical)
    };
    decoded.ok_or_else(|| wrong_type(&format!("an {}", kind.name()), &format!("it is an {}", a.kind.name())))
}

/// [XQueryFO §12.1.1] `op:hexBinary-equal`: whether `a` and `b` are of the
/// same octets in the same order.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no `xs:hexBinary`.
pub fn hex_binary_equal(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(octets(a, AtomicType::HexBinary)? == octets(b, AtomicType::HexBinary)?)
}

/// [XQueryFO §12.1.2] `op:base64Binary-equal`: whether `a` and `b` are of
/// the same octets in the same order.
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no `xs:base64Binary`.
pub fn base64_binary_equal(a: &Atomic, b: &Atomic) -> Result<bool> {
    Ok(octets(a, AtomicType::Base64Binary)? == octets(b, AtomicType::Base64Binary)?)
}

/// [XQueryFO §13.1.1] `op:NOTATION-equal`: whether `a` and `b` are equal
/// code point by code point, as expanded QNames [XQueryDM §3.3.3].
///
/// # Errors
///
/// err:XPTY0004 of an operand that is no `xs:NOTATION`.
pub fn notation_equal(a: &Atomic, b: &Atomic) -> Result<bool> {
    for x in [a, b] {
        if x.kind != AtomicType::Notation {
            return Err(wrong_type("an xs:NOTATION", &format!("it is an {}", x.kind.name())));
        }
    }
    Ok(equal(a, b))
}

#[cfg(test)]
mod tests;
