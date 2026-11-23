//! The values of a typed table's self-referencing column [ISO/IEC
//! 9075-2:2023 §4.11.1, §14.11 GR7d], and what an INSERT or UPDATE of a
//! typed table may not do [§14.11 SR5, §14.15 SR3].
//!
//! \[DESIGN\] A system-generated reference's octets are the value its
//! subtable family's sequence generator gives the row, as sixteen octets,
//! most significant first. A derived reference's are the values of the
//! derivational attributes as `value::codec` writes a row's (UA063): equal
//! for values written alike, and different for values that differ.

use crate::catalog::TableDesc;
use crate::db::{State, api, rule};
use crate::error::{Code, Result, SqlError};
use crate::storage::Row;
use crate::value::codec::put_row;
use crate::value::types::REF_OCTETS;
use crate::value::{Binary, Value};

/// [ISO/IEC 9075-2:2023 §14.11 GR7d i] The REF value of a candidate row
/// whose subtable family's sequence generator gave it `next`.
///
/// # Errors
///
/// SQLCODE -910 if `next` is no integer.
pub(in crate::db) fn system_reference(next: &Value) -> Result<Value> {
    let Value::Exact(d) = next else { return Err(api(format!("[DESIGN] a sequence generator gave {next}, no integer"))) };
    Ok(Value::Reference(Binary::of(&d.units.to_be_bytes())))
}

/// [ISO/IEC 9075-2:2023 §14.11 GR7d ii] The value of a derived
/// self-referencing column of `row`, derived from its columns at
/// `columns`, those of the derivational attributes.
///
/// # Errors
///
/// SQLCODE -910 if `row` lacks one; -900 if their values take more than
/// `REF_OCTETS` octets.
pub(in crate::db) fn derived_reference(row: &Row, columns: &[usize]) -> Result<Value> {
    let values = columns.iter().map(|&c| row.get(c).cloned().ok_or_else(|| api(format!("[DESIGN] a row has no column {c}"))));
    let mut octets = Vec::new();
    put_row(&mut octets, &values.collect::<Result<Row>>()?);
    if octets.len() > REF_OCTETS {
        let msg = format!(
            "[DESIGN] a reference has at most {REF_OCTETS} octets, and the derivational attributes' values take {} (RULES.md D81)",
            octets.len()
        );
        return Err(SqlError::new(Code::Unsupported, msg));
    }
    Ok(Value::Reference(Binary::new(octets)))
}

/// [ISO/IEC 9075-2:2023 §14.11 SR3, SR5, §11.3 GR4n] That the base table
/// `desc` is insertable-into: of a typed table, its structured type is
/// instantiable.
///
/// # Errors
///
/// SQLCODE -103 if it is not.
pub(in crate::db) fn insertable_into(st: &State, desc: &TableDesc) -> Result<()> {
    let Some(typed) = &desc.typed else { return Ok(()) };
    if st.catalog.structured_type(&typed.udt).is_some_and(|t| !t.instantiable) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §14.11 SR5] {} is a typed table of {}, which is NOT INSTANTIABLE, so no row is inserted into it",
            desc.name, typed.udt
        )));
    }
    Ok(())
}

// Unit tests: the octets of REF values (RULES.md R-TEST-1, private logic).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Decimal;

    /// The octets of the reference `v`.
    fn octets(v: Result<Value>) -> Vec<u8> {
        let Value::Reference(b) = v.unwrap() else { panic!("no reference") };
        b.bytes().to_vec()
    }

    /// Verifies: [ISO/IEC 9075-2:2023 §14.11 GR7d i] [DESIGN] a
    /// system-generated reference is the generator's value as sixteen
    /// octets, distinct values distinct; a value no integer is SQLCODE
    /// -910.
    #[test]
    fn system_references_are_the_generators_values() {
        let one = octets(system_reference(&Value::Exact(Decimal::new(1, 0))));
        let two = octets(system_reference(&Value::Exact(Decimal::new(2, 0))));
        assert_eq!(one.len(), 16);
        assert_eq!(one.last(), Some(&1));
        assert_ne!(one, two);
        assert_eq!(system_reference(&Value::Null).unwrap_err().code, Code::Api);
    }

    /// Verifies: [ISO/IEC 9075-2:2023 §14.11 GR7d ii] [DESIGN] (UA063) a
    /// derived reference is equal for equal derivational values and
    /// different for different ones; one of more than REF_OCTETS octets is
    /// SQLCODE -900, and a column the row lacks -910.
    #[test]
    fn derived_references_follow_their_values() {
        let row: Row = vec![Value::Char("A".into()), Value::Exact(Decimal::new(7, 0)), Value::Char("B".into())];
        let same: Row = vec![Value::Char("Z".into()), Value::Exact(Decimal::new(7, 0)), Value::Char("B".into())];
        assert_eq!(octets(derived_reference(&row, &[1, 2])), octets(derived_reference(&same, &[1, 2])));
        assert_ne!(octets(derived_reference(&row, &[0, 1])), octets(derived_reference(&same, &[0, 1])));
        let long: Row = vec![Value::Char("X".repeat(REF_OCTETS).into())];
        assert_eq!(derived_reference(&long, &[0]).unwrap_err().code, Code::Unsupported);
        assert_eq!(derived_reference(&row, &[3]).unwrap_err().code, Code::Api);
    }
}
