//! \[EXTENSION\] The functions that make UUIDs: UUID_V4(), a random UUID
//! of RFC 9562 version 4 (RULES.md D182), and UUID_V7(), a time-ordered
//! UUID of version 7 from the database's clock (RULES.md D183).
//!
//! Random bits come from the keys of the standard library's `RandomState`,
//! which it draws from the system once a thread and then steps by one for
//! each state made. A fresh state's SipHash of a word index is a random
//! word, so the core needs no dependency, no unsafe code and no code of
//! one operating system's. The words are unpredictable to one who does not
//! know the keys, though no cryptographic generator's.
//!
//! Each call makes its own value, so each row gets one, and none is folded
//! into a constant. A version 7 UUID never sorts before the last one its
//! database made (`Ordered`), so those of a session sort as they were made.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, Mutex};

use super::{UUID_OCTETS, UuidType, uuid_value};
use crate::error::{Code, Result, SqlError};
use crate::functions::{CallContext, ScalarFunction};
use crate::value::{DataType, Value};

/// The name of UUID_V4.
pub const UUID_V4: &str = "UUID_V4";
/// The name of UUID_V7.
pub const UUID_V7: &str = "UUID_V7";

/// [RFC 9562 §4.2] The version of a random UUID.
const VERSION_RANDOM: u128 = 4;
/// [RFC 9562 §4.2] The version of a time-ordered UUID.
const VERSION_ORDERED: u128 = 7;
/// [RFC 9562 §4.1] The variant bits of RFC 9562, `10`.
const VARIANT: u128 = 0b10;
/// [RFC 9562 §5.7] Where the version sits, from the low bit.
const VERSION_SHIFT: u32 = 76;
/// [RFC 9562 §4.1] Where the variant sits, from the low bit.
const VARIANT_SHIFT: u32 = 62;
/// [RFC 9562 §4.2] The bits of the version.
const VERSION_MASK: u128 = 0xf;
/// [RFC 9562 §4.1] The bits of the variant.
const VARIANT_MASK: u128 = 0b11;
/// [RFC 9562 §5.7] The bits of `rand_b`, below the variant.
const RAND_B_BITS: u32 = 62;
/// [RFC 9562 §5.7] The bits of `rand_a`, between the version and the
/// variant.
const RAND_A_BITS: u32 = 12;
/// [RFC 9562 §5.7] Where `rand_a` sits, from the low bit: above `rand_b`
/// and the two bits of the variant.
const RAND_A_SHIFT: u32 = 64;
/// The random bits of a version 7 UUID, `rand_a` and `rand_b`.
const RANDOM_BITS: u32 = RAND_A_BITS + RAND_B_BITS;
/// [RFC 9562 §5.7] Where `unix_ts_ms` sits, from the low bit.
const TIME_SHIFT: u32 = 80;
/// [RFC 9562 §5.7] The bits of `unix_ts_ms`.
const TIME_BITS: u32 = 48;
/// Microseconds in a millisecond, of the clock's microseconds.
const MICROS_PER_MILLI: i64 = 1_000;
/// The octets of a random word.
const WORD: usize = 8;

/// \[DESIGN\] The last version 7 UUID a database made, as the number of its
/// time and random bits (RULES.md D183), shared by every snapshot of its
/// state, so the next one made exceeds it.
#[derive(Debug, Default)]
pub struct Ordered {
    last: Mutex<u128>,
}

/// The two functions.
pub(crate) fn functions() -> Vec<Arc<dyn ScalarFunction>> {
    vec![Arc::new(UuidV4), Arc::new(UuidV7)]
}

/// `bits` masked to its low `n`.
fn low(bits: u128, n: u32) -> u128 {
    bits & 1_u128.checked_shl(n).map_or(u128::MAX, |b| b.wrapping_sub(1))
}

/// `bits` moved `by` places up, what passes the top dropped.
fn shifted(bits: u128, by: u32) -> u128 {
    bits.checked_shl(by).unwrap_or(0)
}

/// [RFC 9562 §4.1, §4.2] `bits` with its version set to `version` and
/// its variant to `10`.
fn marked(bits: u128, version: u128) -> u128 {
    let cleared = bits & !shifted(VERSION_MASK, VERSION_SHIFT) & !shifted(VARIANT_MASK, VARIANT_SHIFT);
    cleared | shifted(version, VERSION_SHIFT) | shifted(VARIANT, VARIANT_SHIFT)
}

/// 128 random bits, as the module says.
fn random_bits() -> u128 {
    let mut octets = [0_u8; UUID_OCTETS];
    for (index, chunk) in (0_u64..).zip(octets.chunks_mut(WORD)) {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(index);
        chunk.iter_mut().zip(hasher.finish().to_be_bytes()).for_each(|(o, b)| *o = b);
    }
    u128::from_be_bytes(octets)
}

/// [RFC 9562 §5.4] The version 4 UUID of the random `bits`: its version
/// and variant set, its other 122 bits those of `bits`.
pub fn random_uuid(bits: u128) -> [u8; UUID_OCTETS] {
    marked(bits, VERSION_RANDOM).to_be_bytes()
}

/// [RFC 9562 §5.7] The version 7 UUID of `payload`: its 48 bits of time,
/// then its 74 random bits, laid out with the version and variant
/// between them.
pub fn ordered_uuid(payload: u128) -> [u8; UUID_OCTETS] {
    let millis = low(payload.checked_shr(RANDOM_BITS).unwrap_or(0), TIME_BITS);
    let rand_a = low(payload.checked_shr(RAND_B_BITS).unwrap_or(0), RAND_A_BITS);
    let rand_b = low(payload, RAND_B_BITS);
    marked(shifted(millis, TIME_SHIFT) | shifted(rand_a, RAND_A_SHIFT) | rand_b, VERSION_ORDERED).to_be_bytes()
}

/// [RFC 9562 §6.2] The payload after `last` at `unix_micros`, of the random
/// `bits`: the milliseconds of that instant, then the low 74 of `bits`;
/// but if that does not exceed `last`, `last` plus one, its random bits
/// then a counter (§6.2 method 2), so each exceeds the one before.
pub fn next_payload(last: u128, unix_micros: i64, bits: u128) -> u128 {
    let millis = u128::try_from(unix_micros.checked_div(MICROS_PER_MILLI).unwrap_or(0)).unwrap_or(0);
    let fresh = shifted(low(millis, TIME_BITS), RANDOM_BITS) | low(bits, RANDOM_BITS);
    if fresh > last { fresh } else { last.saturating_add(1) }
}

impl Ordered {
    /// The next version 7 UUID at `unix_micros`, after the last made.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the last one made is poisoned.
    pub fn next(&self, unix_micros: i64) -> Result<[u8; UUID_OCTETS]> {
        let bits = random_bits();
        let mut last = crate::sync::lock(&self.last)?;
        let next = next_payload(*last, unix_micros, bits);
        *last = next;
        Ok(ordered_uuid(next))
    }
}

/// That a function of no arguments was given some.
fn no_arguments(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {name} takes no arguments"))
}

/// UUID_V4.
#[derive(Debug)]
struct UuidV4;

impl ScalarFunction for UuidV4 {
    fn name(&self) -> &str {
        UUID_V4
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if args.is_empty() { Ok(UuidType::data_type()) } else { Err(no_arguments(UUID_V4)) }
    }
    fn call(&self, _: &[Value]) -> Result<Value> {
        Ok(uuid_value(random_uuid(random_bits())))
    }
    fn deterministic(&self) -> bool {
        false
    }
}

/// UUID_V7.
#[derive(Debug)]
struct UuidV7;

impl ScalarFunction for UuidV7 {
    fn name(&self) -> &str {
        UUID_V7
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        if args.is_empty() { Ok(UuidType::data_type()) } else { Err(no_arguments(UUID_V7)) }
    }
    /// Where no database runs the call: the system's clock, and an order
    /// of its own.
    fn call(&self, _: &[Value]) -> Result<Value> {
        use crate::clock::Clock;
        Ordered::default().next(crate::clock::SystemClock.now()).map(uuid_value)
    }
    fn uses_context(&self, _: usize) -> bool {
        true
    }
    /// The database's clock, and its order.
    fn call_in(&self, args: &[Value], cx: &dyn CallContext) -> Result<Value> {
        match cx.uuid_clock() {
            Some((clock, ordered)) => ordered.next(clock.now()).map(uuid_value),
            None => self.call(args),
        }
    }
    fn deterministic(&self) -> bool {
        false
    }
}

// Unit tests: the UUIDs made, their bits and order (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
