//! \[DESIGN\] Parameter modes (RULES.md D978): with `parameter-modes`, a
//! Prepared of a CALL of an SQL-invoked procedure carries, in its tail,
//! the procedure's specific name and each of the statement's `?`s, in the
//! order written: the mode and ordinal position of the parameter whose
//! argument holds it, and its type [ISO/IEC 9075-2:2023 §20.10 GR7e]. Its
//! head's parameters are the input ones alone, those an Execute gives
//! values for, as ever; the output ones' values come back as the row its
//! columns describe. A Prepared of any other statement has no such field.
//!
//! ```text
//! 20 called     the specific name's schema and name, strings; count u32,
//!               then of each `?` its mode u8 (1 IN, 2 INOUT, 4 OUT, as
//!               Table 33 codes them), its parameter's ordinal position
//!               u32, from 1, and its type, as a head's parameter is
//! ```

use turborel::bytes::{Put, Reader};
use turborel::catalog::{ParameterMode, QualifiedName};
use turborel::error::Result;
use turborel::params::DynamicParameter;

use crate::Column;
use crate::declared::read_declared;
use crate::fields::{counted, parameter, put_count, put_parameter};
use crate::tail::{CALLED, DECLARED, Tail};

/// \[DESIGN\] RULES.md D978: what a Prepared of a CALL tells of the
/// procedure it invokes and of its `?`s.
#[derive(Debug, Clone, PartialEq)]
pub struct Called {
    /// [ISO/IEC 9075-2:2023 §20.10 GR7e vi] The procedure's specific name.
    pub procedure: QualifiedName,
    /// Each `?` of the statement, in the order written, with its
    /// parameter's mode and ordinal position [GR7e iv, v].
    pub parameters: Vec<DynamicParameter>,
}

/// Writes `called`, if there is one.
pub(crate) fn put_called(b: &mut Vec<u8>, called: Option<&Called>) {
    let Some(called) = called else { return };
    b.put_u8(CALLED);
    b.put_str(&called.procedure.auth);
    b.put_str(&called.procedure.ident);
    put_count(b, called.parameters.len());
    for p in &called.parameters {
        b.put_u8(p.mode.code());
        b.put_u32(p.ordinal.unwrap_or(0));
        put_parameter(b, &p.data_type);
    }
}

/// One `?` as `put_called` wrote it.
fn called_parameter(r: &mut Reader<'_>) -> Result<DynamicParameter> {
    let code = r.u8()?;
    let mode = ParameterMode::of_code(code).ok_or_else(|| r.corrupt(&format!("{code} is no parameter mode's code")))?;
    let ordinal = r.u32()?;
    if ordinal == 0 {
        return Err(r.corrupt("a parameter's ordinal position is from 1"));
    }
    Ok(DynamicParameter { mode, ordinal: Some(ordinal), data_type: parameter(r)? })
}

/// The tail of a Prepared: its declared types given to `columns`, one for
/// each or none, and what it tells of a CALL, if it does.
pub(crate) fn prepared_tail(r: &mut Reader<'_>, columns: &mut [Column]) -> Result<Option<Called>> {
    let mut tail = Tail::new();
    let mut called = None;
    while let Some(tag) = tail.next(r, &[DECLARED, CALLED])? {
        if tag == DECLARED {
            read_declared(r, columns)?;
            continue;
        }
        let procedure = QualifiedName { auth: r.str()?, ident: r.str()? };
        called = Some(Called { procedure, parameters: counted(r, called_parameter)? });
    }
    Ok(called)
}
